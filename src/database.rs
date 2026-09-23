use crate::exit;
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::{Mutex, MutexGuard, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS messages (
        source   TEXT    NOT NULL,   -- 'journald' | 'auditd' | 'sysstat' | 'custom'
        ref_id   INTEGER NOT NULL,   -- id of the row in that source's own table
        ts       TEXT    NOT NULL,   -- UTC 'YYYY-MM-DD HH:MM:SS'
        severity TEXT    NOT NULL,   -- 'error' | 'warning' | 'info'
        summary  TEXT    NOT NULL,   -- one-line human text, no join needed for a UI list
        PRIMARY KEY (source, ref_id)
    );
    CREATE INDEX IF NOT EXISTS idx_messages_ts ON messages(ts);

    CREATE TABLE IF NOT EXISTS custom (
        custom_id   INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        pid         INTEGER NOT NULL,
        ppid        INTEGER,
        exit_code   INTEGER NOT NULL,  -- raw wait status: signal in low 7 bits, exit status << 8
        comm        TEXT,              -- process name, best-effort
        exe         TEXT               -- full executable path, from systemd-coredump for crashes
    );
    CREATE INDEX IF NOT EXISTS idx_custom_ts ON custom(ts);

    CREATE TABLE IF NOT EXISTS journald (
        journald_id INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        priority    INTEGER NOT NULL,  -- syslog level, 0-7
        unit        TEXT,
        pid         INTEGER,
        message     TEXT    NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_journald_ts ON journald(ts);

    CREATE TABLE IF NOT EXISTS auditd (
        audit_id   INTEGER PRIMARY KEY,
        ts         TEXT    NOT NULL,
        event_type TEXT    NOT NULL,
        pid        INTEGER,
        uid        INTEGER,
        executable TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_auditd_ts ON auditd(ts);

    CREATE TABLE IF NOT EXISTS sysstat (
        sysstat_id  INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        cpu_pct     REAL,
        mem_pct     REAL,
        disk_io_kb  REAL,
        load_avg    REAL,
        gpu_pct     REAL,
        gpu_mem_pct REAL,
        gpu_temp    REAL
    );
    CREATE INDEX IF NOT EXISTS idx_sysstat_ts ON sysstat(ts);
";

const TRIM_CHUNK: usize = 1000;
const TIME_TABLES: [&str; 5] = ["messages", "custom", "journald", "auditd", "sysstat"];

pub struct Sample {
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub disk_io_kb: f64,
    pub load_avg: f64,
    pub gpu_pct: Option<f64>,
    pub gpu_mem_pct: Option<f64>,
    pub gpu_temp: Option<f64>,
}

pub fn db_path() -> String {
    if let Ok(path) = std::env::var("BLACKBOX_DB") {
        return path;
    }
    // Never default to the working directory: that is how a database ends up in a git repo.
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = format!("{home}/.local/share/blackbox");
    let _ = std::fs::create_dir_all(&dir);
    // the database holds process names and log lines: keep it readable by this user only
    let _ = std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    format!("{dir}/blackbox.db")
}

fn conn() -> MutexGuard<'static, Connection> {
    DB.get()
        .expect("init_database() must run first")
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn init_database() {
    let conn = Connection::open(db_path()).expect("cannot open database");
    conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
        .expect("cannot enable WAL");
    // FULL: every commit reaches the disk before the next event. With NORMAL, the last ~30 s sit in
    // the page cache and die with the machine, and those seconds are what a flight recorder is for.
    // Writes are rare (a sample every 10 s plus the odd event), so this costs almost nothing.
    conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;")
        .unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    for column in ["gpu_pct", "gpu_mem_pct", "gpu_temp"] {
        ensure_column(&conn, "sysstat", column, "REAL");
    }
    ensure_column(&conn, "custom", "exe", "TEXT");
    let _ = DB.set(Mutex::new(conn));
}

fn ensure_column(conn: &Connection, table: &str, column: &str, kind: &str) {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut stmt| {
            let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(names.flatten().any(|n| n == column))
        })
        .unwrap_or(true);
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))
            .unwrap();
    }
}

pub fn ts_text(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Inserts one source row and, when `alert` is set, its `messages` index row, atomically.
fn record<F>(source: &str, ts: &str, alert: Option<(&str, &str)>, insert: F)
where
    F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<()>,
{
    let mut guard = conn();
    let result = (|| -> rusqlite::Result<()> {
        let tx = guard.transaction()?;
        insert(&tx)?;
        if let Some((severity, summary)) = alert {
            let ref_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT OR REPLACE INTO messages (source, ref_id, ts, severity, summary)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![source, ref_id, ts, severity, summary],
            )?;
        }
        tx.commit()
    })();
    if let Err(e) = result {
        eprintln!("db write failed ({source}): {e}");
    }
}

pub fn add_custom_event(
    ts: i64,
    pid: i32,
    ppid: Option<i32>,
    exit_code: i32,
    comm: Option<&str>,
    severity: &str,
    summary: &str,
) {
    let ts = ts_text(ts);
    record("custom", &ts, Some((severity, summary)), |tx| {
        tx.execute(
            "INSERT INTO custom (ts, pid, ppid, exit_code, comm) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, pid, ppid, exit_code, comm],
        )
        .map(|_| ())
    });
}

/// Names a crash after the fact from systemd-coredump's journal entry, which carries the process
/// name and executable even when the process was gone before the collector could look it up.
/// Matches the newest crash of that pid in the ten minutes before the entry.
pub fn name_crash(ts: i64, pid: i32, comm: Option<&str>, exe: Option<&str>) {
    let ts = ts_text(ts);
    let mut guard = conn();
    let result = (|| -> rusqlite::Result<()> {
        let tx = guard.transaction()?;
        let crash: Option<(i64, i64, Option<String>)> = tx
            .query_row(
                "SELECT custom_id, exit_code, comm FROM custom
                 WHERE pid = ?1 AND exe IS NULL AND (exit_code & 127) != 0
                   AND ts BETWEEN datetime(?2, '-600 seconds') AND datetime(?2, '+60 seconds')
                 ORDER BY custom_id DESC LIMIT 1",
                params![pid, ts],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, exit_code, known)) = crash else {
            return Ok(());
        };
        let name = known.as_deref().or(comm);
        tx.execute(
            "UPDATE custom SET comm = ?1, exe = ?2 WHERE custom_id = ?3",
            params![name, exe, id],
        )?;
        if let Some(name) = name {
            let summary = format!("{name} (pid {pid}) {}", exit::describe(exit_code as u32));
            tx.execute(
                "UPDATE messages SET summary = ?1 WHERE source = 'custom' AND ref_id = ?2",
                params![summary, id],
            )?;
        }
        tx.commit()
    })();
    if let Err(e) = result {
        eprintln!("db write failed (crash name): {e}");
    }
}

/// Latest timestamp already stored for a source table (used to backfill after a restart).
pub fn last_ts(table: &str) -> Option<String> {
    if !TIME_TABLES.contains(&table) {
        return None;
    }
    conn()
        .query_row(&format!("SELECT MAX(ts) FROM {table}"), [], |r| r.get(0))
        .ok()
        .flatten()
}

fn journald_duplicate(ts: &str, unit: Option<&str>, message: &str) -> bool {
    conn()
        .query_row(
            "SELECT 1 FROM journald WHERE ts = ?1 AND message = ?2 AND IFNULL(unit, '') = IFNULL(?3, '') LIMIT 1",
            params![ts, message, unit],
            |_| Ok(()),
        )
        .is_ok()
}

pub fn add_journald_event(
    ts: i64,
    priority: i32,
    unit: Option<&str>,
    pid: Option<i32>,
    message: &str,
) {
    let ts = ts_text(ts);
    if journald_duplicate(&ts, unit, message) {
        return;
    }
    let severity = if priority <= 3 { "error" } else { "warning" };
    let summary = format!(
        "{}: {}",
        unit.unwrap_or("system"),
        message
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>()
    );
    let alert = (priority <= 4).then_some((severity, summary.as_str()));
    record("journald", &ts, alert, |tx| {
        tx.execute(
            "INSERT INTO journald (ts, priority, unit, pid, message) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, priority, unit, pid, message],
        )
        .map(|_| ())
    });
}

pub fn add_auditd_event(
    ts: i64,
    event_type: &str,
    pid: Option<i32>,
    uid: Option<i32>,
    executable: Option<&str>,
    alert: Option<(&str, &str)>,
) {
    let ts = ts_text(ts);
    record("auditd", &ts, alert, |tx| {
        tx.execute(
            "INSERT INTO auditd (ts, event_type, pid, uid, executable) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, event_type, pid, uid, executable],
        )
        .map(|_| ())
    });
}

pub fn add_sysstat_event(ts: i64, s: &Sample, alert: Option<(&str, &str)>) {
    let ts = ts_text(ts);
    record("sysstat", &ts, alert, |tx| {
        tx.execute(
            "INSERT INTO sysstat (ts, cpu_pct, mem_pct, disk_io_kb, load_avg, gpu_pct, gpu_mem_pct, gpu_temp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ts, s.cpu_pct, s.mem_pct, s.disk_io_kb, s.load_avg, s.gpu_pct, s.gpu_mem_pct,
                s.gpu_temp
            ],
        )
        .map(|_| ())
    });
}

/// Bytes actually holding data (freed pages are reused, so the file itself never shrinks).
pub fn logical_size_bytes() -> u64 {
    let c = conn();
    let pragma = |name: &str| -> i64 {
        c.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap_or(0)
    };
    ((pragma("page_count") - pragma("freelist_count")).max(0) * pragma("page_size")) as u64
}

/// Ring buffer: once over `max_bytes`, drop the oldest slice of time from every table
/// until the data is back under 90% of the budget. Returns the number of rows deleted.
pub fn trim_to_budget(max_bytes: u64) -> usize {
    if logical_size_bytes() <= max_bytes {
        return 0;
    }
    let target = max_bytes / 10 * 9;
    // One aggregate per subquery: SQLite turns a lone MIN/MAX on an indexed column into an index
    // seek, but MIN and MAX together in one SELECT scan the whole index.
    let extreme = |agg: &str| -> String {
        let parts = TIME_TABLES
            .iter()
            .map(|t| format!("SELECT (SELECT {agg}(ts) FROM {t}) AS v"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        format!("SELECT {agg}(v) FROM ({parts})")
    };
    let (min_sql, max_sql) = (extreme("MIN"), extreme("MAX"));
    let mut deleted = 0;
    for _ in 0..40 {
        if logical_size_bytes() <= target {
            break;
        }
        // where the oldest tenth of the recorded time span ends
        let cutoff: Option<String> = {
            let c = conn();
            let lo = c.query_row(&min_sql, [], |r| r.get::<_, Option<String>>(0));
            let hi = c.query_row(&max_sql, [], |r| r.get::<_, Option<String>>(0));
            match (lo, hi) {
                (Ok(Some(lo)), Ok(Some(hi))) => c
                    .query_row(
                        "SELECT datetime(?1, '+' || max(60, (strftime('%s', ?2) - strftime('%s', ?1)) / 10) || ' seconds')",
                        params![lo, hi],
                        |r| r.get(0),
                    )
                    .ok(),
                _ => None,
            }
        };
        let Some(cutoff) = cutoff else { break };
        // Small chunks, lock released in between: a big single DELETE would block every writer
        // (the collector would start dropping events) for as long as it runs.
        for table in TIME_TABLES {
            loop {
                let n = {
                    let c = conn();
                    c.execute(
                        &format!(
                            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} WHERE ts < ?1 LIMIT {TRIM_CHUNK})"
                        ),
                        params![cutoff],
                    )
                    .unwrap_or(0)
                };
                deleted += n;
                if n < TRIM_CHUNK {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    checkpoint();
    deleted
}

pub fn checkpoint() {
    let _ = conn().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

pub fn start_trim_thread(max_bytes: u64) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            let deleted = trim_to_budget(max_bytes);
            if deleted > 0 {
                eprintln!(
                    "trim: deleted {deleted} old rows (budget {} MB)",
                    max_bytes / 1_000_000
                );
            } else {
                checkpoint();
            }
        }
    });
}

#[cfg(test)]
mod tests {
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
        add_journald_event(1_700_000_100, 3, Some("unit"), Some(5), "boom");
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
        assert_eq!(count("custom"), 3000);
        assert_eq!(count("messages"), 3000 + 1 + 1);
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

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[ignore = "stress test: cargo test --release trim_does_not_stall_writers -- --ignored --nocapture"]
    fn trim_does_not_stall_writers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let path = std::env::temp_dir().join(format!("bb_stress_{}.sqlite", std::process::id()));
        // SAFETY: only this test touches the environment or the global connection.
        unsafe { std::env::set_var("BLACKBOX_DB", &path) };
        init_database();

        let rows: u64 = std::env::var("STRESS_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(600_000);
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
                add_custom_event(1_800_000_000 + writes as i64, 1, None, 11, Some("live"), "error", "live write");
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
        assert!(worst < Duration::from_millis(500), "a writer was stalled for {worst:?} during trim");
    }
}
