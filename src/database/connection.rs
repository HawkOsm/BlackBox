use super::scheme::SCHEMA;
use rusqlite::Connection;
use std::sync::{Mutex, MutexGuard, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// A read-only connection for `read()`: a source can look at what is stored, but cannot write past
/// the writer. WAL lets it read next to the writer.
static READER: OnceLock<Mutex<Connection>> = OnceLock::new();

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

pub(super) fn conn() -> MutexGuard<'static, Connection> {
    DB.get()
        .expect("init_database() must run first")
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn init_database() {
    let path = db_path();
    let _ = DB.set(Mutex::new(open(&path)));
    let reader = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("cannot open database for reading");
    reader
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let _ = READER.set(Mutex::new(reader));
}

/// Opens a database and brings its schema up to date. Tests open `:memory:` the same way.
pub(crate) fn open(path: &str) -> Connection {
    let conn = Connection::open(path).expect("cannot open database");
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
    conn
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

/// Runs a read, for a source checking what it already stored. The connection is read-only:
/// writes go through `write()`.
pub fn read<T>(f: impl FnOnce(&Connection) -> T) -> T {
    let reader = READER
        .get()
        .expect("init_database() must run first")
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&reader)
}
