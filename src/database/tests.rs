use super::retention::{checkpoint, logical_size_bytes, trim_to_budget};
use super::*;
use rusqlite::Connection;

/// `n` crash rows with their alerts, one a second from `from`, stored as the collector stores them.
fn seed(c: &Connection, from: i64, n: i64, comm: &str) {
    c.execute_batch(&format!(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i < {last})
         INSERT INTO custom (ts, pid, ppid, exit_code, comm)
         SELECT datetime({from} + i, 'unixepoch'), i, 1, 11, '{comm}' FROM n;
         INSERT OR IGNORE INTO messages (source, ref_id, ts, severity, summary)
         SELECT 'custom', custom_id, ts, 'error', '{comm} {pad}' FROM custom
         WHERE ts >= datetime({from}, 'unixepoch') AND comm = '{comm}';",
        last = n - 1,
        pad = "x".repeat(100),
    ))
    .unwrap();
}

/// The trim works on the shared connection, so this is the one test that opens it.
#[test]
fn trim_keeps_the_newest_rows_under_budget() {
    let path = std::env::temp_dir().join(format!("bb_test_{}.sqlite", std::process::id()));
    // SAFETY: only this test touches the environment or the global connection.
    unsafe { std::env::set_var("BLACKBOX_DB", &path) };
    init_database();

    seed(&conn(), 1_700_000_000, 3000, "test");
    let before = logical_size_bytes();
    let deleted = trim_to_budget(before / 2);
    assert!(deleted > 0, "trim should delete rows when over budget");
    assert!(
        logical_size_bytes() <= before / 2,
        "size should drop under the budget"
    );
    let custom = count(&conn(), "SELECT COUNT(*) FROM custom");
    assert!(custom < 3000);
    assert_eq!(
        count(&conn(), "SELECT COUNT(*) FROM messages"),
        custom,
        "every row goes with its alert"
    );

    // one row stamped in the far future (a clock that was once wrong) must not make the trim
    // throw away every real row
    conn()
        .execute_batch("DELETE FROM custom; DELETE FROM messages;")
        .unwrap();
    // every table and index costs a few pages even when empty; the budget below is half of the
    // *data*, so adding a table to the schema must not change how many rows this test keeps
    let empty = logical_size_bytes();
    let now = now();
    seed(&conn(), now - 3000, 3000, "test");
    seed(&conn(), 4_102_444_800, 1, "future");
    let before = logical_size_bytes();
    trim_to_budget(empty + (before - empty) / 2);
    let recent = count(&conn(), "SELECT COUNT(*) FROM custom WHERE comm = 'test'");
    assert!(recent > 1000, "trim kept only {recent} of 3000 recent rows");

    // a source reads through `read()`, which cannot write
    assert!(read(|c| c.execute_batch("DELETE FROM custom")).is_err());
    assert_eq!(
        read(|c| count(c, "SELECT COUNT(*) FROM custom")),
        recent + 1
    );

    // through the writer thread: `drain` returns once what was queued is stored
    start_writer();
    write(SetState {
        key: "drained".to_string(),
        value: Some("yes".to_string()),
    });
    drain();
    assert_eq!(get_state("drained").as_deref(), Some("yes"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn exists_tells_an_error_from_a_missing_row() {
    let c = open(":memory:");
    c.execute_batch("INSERT INTO state (key, value) VALUES ('k', 'v')")
        .unwrap();
    assert_eq!(
        exists(&c, "SELECT 1 FROM state WHERE key = 'k'", []),
        Ok(true)
    );
    assert_eq!(
        exists(&c, "SELECT 1 FROM state WHERE key = 'x'", []),
        Ok(false)
    );
    // a broken query is an error, never "not stored": that would store a duplicate
    assert!(exists(&c, "SELECT 1 FROM no_such_table", []).is_err());
}

#[test]
fn a_state_value_is_set_and_forgotten() {
    let mut c = open(":memory:");
    let set = |value: Option<&str>| -> Box<dyn Record> {
        Box::new(SetState {
            key: "k".to_string(),
            value: value.map(str::to_string),
        })
    };
    commit(&mut c, vec![set(Some("a")), set(Some("b"))]);
    let value: String = c
        .query_row("SELECT value FROM state WHERE key = 'k'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(value, "b", "a second value replaces the first");
    commit(&mut c, vec![set(None)]);
    assert_eq!(count(&c, "SELECT COUNT(*) FROM state"), 0);
}

#[test]
#[ignore = "stress test: cargo test --release trim_does_not_stall_writers -- --ignored --nocapture"]
fn trim_does_not_stall_writers() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let path = std::env::temp_dir().join(format!("bb_stress_{}.sqlite", std::process::id()));
    // SAFETY: only this test touches the environment or the global connection.
    unsafe { std::env::set_var("BLACKBOX_DB", &path) };
    init_database();

    let rows: i64 = std::env::var("STRESS_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600_000);
    seed(&conn(), 1_700_000_000, rows, "seed");

    // seeding was one giant transaction; settle its WAL first, as a long-running service would have
    checkpoint();
    let before = logical_size_bytes();
    let done = Arc::new(AtomicBool::new(false));
    let writer_done = done.clone();
    let writer = std::thread::spawn(move || {
        let mut worst = Duration::ZERO;
        let mut writes = 0i64;
        while !writer_done.load(Ordering::Relaxed) {
            let started = Instant::now();
            seed(&conn(), 1_800_000_000 + writes, 1, "live");
            worst = worst.max(started.elapsed());
            writes += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        (worst, writes)
    });

    let started = Instant::now();
    let deleted = trim_to_budget(before / 2);
    let trim_time = started.elapsed();
    done.store(true, Ordering::Relaxed);
    let (worst, writes) = writer.join().unwrap();
    println!(
        "seeded {rows} rows ({} MB); trim deleted {deleted} rows in {trim_time:?}; \
         {writes} concurrent writes, worst single write {worst:?}",
        before / 1_000_000
    );
    let _ = std::fs::remove_file(&path);
    assert!(
        worst < Duration::from_millis(500),
        "a writer was stalled for {worst:?} during trim"
    );
}
