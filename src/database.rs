pub fn init_database() {
    // Initialize the database
    use rusqlite::Connection;
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        CREATE TABLE IF NOT EXISTS messages (
            source   TEXT    NOT NULL,   -- 'journald' | 'auditd' | 'sysstat' | 'custom'
            ref_id   INTEGER NOT NULL,   -- id of the row in that source's own table
            ts       TEXT    NOT NULL,   -- unix timestamp
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
            comm        TEXT               -- process name, best-effort
        );

        CREATE TABLE IF NOT EXISTS journald (
            journald_id INTEGER PRIMARY KEY,
            ts          TEXT    NOT NULL,
            priority    INTEGER NOT NULL,  -- syslog level, 0-7
            unit        TEXT,
            pid         INTEGER,
            message     TEXT    NOT NULL
        );

        CREATE TABLE IF NOT EXISTS auditd (
            audit_id   INTEGER PRIMARY KEY,
            ts         TEXT    NOT NULL,
            event_type TEXT    NOT NULL,
            pid        INTEGER,
            uid        INTEGER,
            executable TEXT
        );

        CREATE TABLE IF NOT EXISTS sysstat (
            sysstat_id INTEGER PRIMARY KEY,
            ts         TEXT    NOT NULL,
            cpu_pct    REAL,
            mem_pct    REAL,
            disk_io_kb REAL,
            load_avg   REAL
        );
    ";
    conn.execute_batch(query).unwrap();
}

fn convert_to_unix_timestamp(ts: i64) -> String {
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).unwrap();
    let readable = dt.format("%Y-%m-%d %H:%M:%S").to_string();
    readable
}

fn add_message(source: &str, ref_id: i64, ts: String, severity: &str, summary: &str) {
    use rusqlite::Connection;
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        INSERT INTO messages (source, ref_id, ts, severity, summary)
        VALUES (?1, ?2, ?3, ?4, ?5);
    ";
    conn.execute(
        query,
        rusqlite::params!(source, ref_id, ts, severity, summary),
    )
    .unwrap();
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
    use rusqlite::Connection;
    let ts = convert_to_unix_timestamp(ts);
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        INSERT INTO custom (ts, pid, ppid, exit_code, comm)
        VALUES (?1, ?2, ?3, ?4, ?5);
    ";
    conn.execute(query, rusqlite::params!(ts, pid, ppid, exit_code, comm))
        .unwrap();

    add_message("custom", conn.last_insert_rowid(), ts, severity, summary);
}

pub fn add_journald_event(
    ts: i64,
    priority: i32,
    unit: Option<&str>,
    pid: Option<i32>,
    message: &str,
) {
    use rusqlite::Connection;
    let ts = convert_to_unix_timestamp(ts);
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        INSERT INTO journald (ts, priority, unit, pid, message)
        VALUES (?1, ?2, ?3, ?4, ?5);
    ";
    conn.execute(query, rusqlite::params!(ts, priority, unit, pid, message))
        .unwrap();

    add_message(
        "journald",
        conn.last_insert_rowid(),
        ts,
        "info",
        &format!("Journald event: {}", message),
    );
}
pub fn add_auditd_event(
    ts: i64,
    event_type: &str,
    pid: Option<i32>,
    uid: Option<i32>,
    executable: Option<&str>,
) {
    use rusqlite::Connection;
    let ts = convert_to_unix_timestamp(ts);
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        INSERT INTO auditd (ts, event_type, pid, uid, executable)
        VALUES (?1, ?2, ?3, ?4, ?5);
    ";
    conn.execute(
        query,
        rusqlite::params!(ts, event_type, pid, uid, executable),
    )
    .unwrap();

    add_message(
        "auditd",
        conn.last_insert_rowid(),
        ts,
        "info",
        &format!("Auditd event: {} for PID {:?}", event_type, pid),
    );
}

pub fn add_sysstat_event(ts: i64, cpu_pct: f64, mem_pct: f64, disk_io_kb: f64, load_avg: f64) {
    use rusqlite::Connection;
    let ts = convert_to_unix_timestamp(ts);
    let conn = Connection::open("db.sqlite").unwrap();
    let query = "
        INSERT INTO sysstat (ts, cpu_pct, mem_pct, disk_io_kb, load_avg)
        VALUES (?1, ?2, ?3, ?4, ?5);
    ";
    conn.execute(
        query,
        rusqlite::params![ts, cpu_pct, mem_pct, disk_io_kb, load_avg],
    )
    .unwrap();

    add_message(
        "sysstat",
        conn.last_insert_rowid(),
        ts,
        "info",
        &format!(
            "Sysstat event: CPU {}%, MEM {}%, Disk IO {} KB/s, Load Avg {}",
            cpu_pct, mem_pct, disk_io_kb, load_avg
        ),
    );
}
