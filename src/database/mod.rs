mod auditd;
mod boot;
mod custom;
mod journald;
mod packages;
mod procstat;
mod retention;
mod scheme;
mod state;
mod sysstat;

// Callers go through `database::…`; which file an item lives in is not their concern.
pub use auditd::add_auditd_event;
pub use boot::{Boot, add_boot_event, boot_recorded, mark_shutdown};
pub use custom::{add_custom_event, name_crash};
pub use journald::{Journal, add_journald};
pub use packages::add_package_change;
pub use procstat::{ProcSample, add_procstat};
pub use retention::start_trim_thread;
pub use state::{get_state, set_state};
pub use sysstat::{Sample, add_sysstat_event};

use rusqlite::{Connection, params};
use scheme::{SCHEMA, TIME_TABLES};
use std::sync::{Mutex, MutexGuard, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

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

/// The inverse of `ts_text`.
pub fn ts_epoch(ts: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp())
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

#[cfg(test)]
mod tests;
