use super::retention::{checkpoint, logical_size_bytes, trim_to_budget};
use super::*;

#[test]
fn write_and_trim() {
    let path = std::env::temp_dir().join(format!("bb_test_{}.sqlite", std::process::id()));
    // SAFETY: only this test touches the environment or the global connection.
    unsafe { std::env::set_var("BLACKBOX_DB", &path) };
    init_database();

    for i in 0..3000 {
        add_custom_event(
            1_700_000_000 + i,
            100,
            Some(1),
            11,
            Some("test"),
            "error",
            "x".repeat(100).as_str(),
        );
    }
    add_journald(&Journal {
        ts: 1_700_000_100,
        priority: 3,
        unit: Some("unit"),
        pid: Some(5),
        message: "boom",
        severity: None,
        cursor: Some(("journald.cursor.test", "s=abc")),
        dedup: false,
    });
    assert_eq!(get_state("journald.cursor.test").as_deref(), Some("s=abc"));
    let journald_ref: i64 = conn()
        .query_row(
            "SELECT ref_id FROM messages WHERE source = 'journald'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let journald_id: i64 = conn()
        .query_row("SELECT journald_id FROM journald", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        journald_ref, journald_id,
        "the cursor write must not shift the row id"
    );

    // crashes in the last seconds before a clean shutdown are shutdown noise
    add_custom_event(
        1_700_050_000,
        7,
        Some(1),
        6,
        Some("start-hyprland"),
        "error",
        "start-hyprland (pid 7) killed by SIGABRT (6)",
    );
    assert_eq!(mark_shutdown(1_700_049_990, 1_700_050_010), 1);
    assert_eq!(
        mark_shutdown(1_700_049_990, 1_700_050_010),
        0,
        "a second run changes nothing"
    );
    let (sev, text): (String, String) = conn()
        .query_row(
            "SELECT severity, summary FROM messages WHERE summary LIKE 'start-hyprland%'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(sev, "info");
    assert!(text.ends_with("(during shutdown)"));
    add_auditd_event(
        1_700_000_100,
        "USER_AUTH",
        Some(1),
        Some(1000),
        Some("/bin/su"),
        Some(("warning", "failed auth")),
    );
    add_sysstat_event(
        1_700_000_100,
        &Sample {
            cpu_pct: 1.0,
            mem_pct: 2.0,
            disk_io_kb: 3.0,
            load_avg: 0.5,
            gpu_pct: Some(4.0),
            gpu_mem_pct: None,
            gpu_temp: None,
        },
        None,
    );

    let count = |table: &str| -> i64 {
        conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count("custom"), 3000 + 1); // + the shutdown-noise crash
    assert_eq!(count("messages"), 3000 + 1 + 1 + 1);
    assert_eq!(count("journald"), 1);
    assert_eq!(count("sysstat"), 1);

    let before = logical_size_bytes();
    let deleted = trim_to_budget(before / 2);
    assert!(deleted > 0, "trim should delete rows when over budget");
    assert!(
        logical_size_bytes() <= before / 2,
        "size should drop under the budget"
    );
    assert!(count("custom") < 3000);

    // a crash stored without a name gets one from the later coredump entry
    add_custom_event(
        1_700_100_000,
        4242,
        Some(1),
        139,
        None,
        "error",
        "? (pid 4242) crashed",
    );
    name_crash(1_700_100_003, 4242, Some("bash"), Some("/usr/bin/bash"));
    let (comm, exe, summary): (String, String, String) = conn()
        .query_row(
            "SELECT comm, exe, summary FROM custom JOIN messages
             ON source = 'custom' AND ref_id = custom_id WHERE pid = 4242",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((comm.as_str(), exe.as_str()), ("bash", "/usr/bin/bash"));
    assert_eq!(
        summary,
        "bash (pid 4242) killed by SIGSEGV (11), core dumped"
    );

    // one row stamped in the far future (a clock that was once wrong) must not make the trim
    // throw away every real row
    conn()
        .execute_batch("DELETE FROM custom; DELETE FROM messages; DELETE FROM journald;")
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for i in 0..3000 {
        add_custom_event(
            now - 3000 + i,
            100,
            Some(1),
            11,
            Some("test"),
            "error",
            &"x".repeat(100),
        );
    }
    add_custom_event(
        4_102_444_800,
        100,
        Some(1),
        11,
        Some("future"),
        "error",
        "from 2100",
    );
    let before = logical_size_bytes();
    trim_to_budget(before / 2);
    let recent: i64 = conn()
        .query_row("SELECT COUNT(*) FROM custom WHERE comm = 'test'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(recent > 1000, "trim kept only {recent} of 3000 recent rows");

    let _ = std::fs::remove_file(&path);
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

    let rows: u64 = std::env::var("STRESS_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600_000);
    conn()
        .execute_batch(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i < {})
             INSERT INTO custom (ts, pid, ppid, exit_code, comm)
             SELECT datetime(1700000000 + i, 'unixepoch'), i, 1, 256, 'seed' FROM n;
             INSERT INTO messages (source, ref_id, ts, severity, summary)
             SELECT 'custom', custom_id, ts, 'info', 'seed (pid ' || pid || ') exited with status 1' FROM custom;",
            rows - 1
        ))
        .unwrap();

    // seeding was one giant transaction; settle its WAL first, as a long-running service would have
    checkpoint();
    let before = logical_size_bytes();
    let done = Arc::new(AtomicBool::new(false));
    let writer_done = done.clone();
    let writer = std::thread::spawn(move || {
        let mut worst = Duration::ZERO;
        let mut writes = 0u64;
        while !writer_done.load(Ordering::Relaxed) {
            let started = Instant::now();
            add_custom_event(
                1_800_000_000 + writes as i64,
                1,
                None,
                11,
                Some("live"),
                "error",
                "live write",
            );
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
