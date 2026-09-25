pub(super) const SCHEMA: &str = "
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

    CREATE TABLE IF NOT EXISTS boot (
        boot_id     INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        kind        TEXT    NOT NULL,  -- 'start' | 'unclean_end'
        kernel_boot TEXT    NOT NULL,  -- the kernel's boot id of the boot this row is about
        kernel      TEXT,              -- kernel release, for 'start'
        note        TEXT,              -- for 'unclean_end': the last thing that boot logged
        UNIQUE (kernel_boot, kind)
    );
    CREATE INDEX IF NOT EXISTS idx_boot_ts ON boot(ts);

    CREATE TABLE IF NOT EXISTS packages (
        package_id  INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        command     TEXT,              -- the pacman command line, when the log has it
        changes     TEXT    NOT NULL   -- one line per package: 'upgraded linux (6.9-1 -> 6.10-1)'
    );
    CREATE INDEX IF NOT EXISTS idx_packages_ts ON packages(ts);

    CREATE TABLE IF NOT EXISTS state (
        key         TEXT PRIMARY KEY,  -- e.g. 'journald.cursor.warnings'
        value       TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS procstat(
        procstat_id INTEGER PRIMARY KEY,
        ts          TEXT    NOT NULL,
        pid         INTEGER NOT NULL,
        comm        TEXT,
        rss_kb      INTEGER NOT NULL,
        pss_kb      INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_procstat_ts ON procstat(ts)

";

pub(super) const TIME_TABLES: [&str; 8] = [
    "messages", "custom", "journald", "auditd", "sysstat", "boot", "packages", "procstat",
];
