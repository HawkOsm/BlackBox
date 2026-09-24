//! What a source may ask of the database, and nothing more. Sources hold an `Arc<dyn Store>`
//! and never see SQL, so the store can move to its own process (docs/microservices.md) without
//! touching them: today the only implementation is `database::LocalStore`, in this process.

pub struct Sample {
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub disk_io_kb: f64,
    pub load_avg: f64,
    pub gpu_pct: Option<f64>,
    pub gpu_mem_pct: Option<f64>,
    pub gpu_temp: Option<f64>,
}

/// One journal entry to store.
pub struct Journal<'a> {
    pub ts: i64,
    pub priority: i32,
    pub unit: Option<&'a str>,
    pub pid: Option<i32>,
    pub message: &'a str,
    /// Overrides the severity the priority implies (a panic logged at info is still a crash).
    pub severity: Option<&'a str>,
    /// Where the follower is in the journal, saved with the row so a restart resumes exactly.
    pub cursor: Option<(&'a str, &'a str)>,
    /// Skip it if an identical row exists: only needed where a time-based resume overlaps.
    pub dedup: bool,
}

pub struct Boot<'a> {
    pub ts: i64,
    pub kind: &'a str,
    pub kernel_boot: &'a str,
    pub kernel: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// Writes are fire-and-forget: a failed write is logged by the store, and a source has nothing
/// better to do about it than carry on.
pub trait Store: Send + Sync {
    fn add_custom_event(
        &self,
        ts: i64,
        pid: i32,
        ppid: Option<i32>,
        exit_code: i32,
        comm: Option<&str>,
        severity: &str,
        summary: &str,
    );
    /// Names an earlier crash of `pid` from systemd-coredump's journal entry.
    fn name_crash(&self, ts: i64, pid: i32, comm: Option<&str>, exe: Option<&str>);
    fn add_journald(&self, j: &Journal);
    fn add_auditd_event(
        &self,
        ts: i64,
        event_type: &str,
        pid: Option<i32>,
        uid: Option<i32>,
        executable: Option<&str>,
        alert: Option<(&str, &str)>,
    );
    fn add_sysstat_event(&self, ts: i64, s: &Sample, alert: Option<(&str, &str)>);
    fn add_package_change(&self, ts: i64, command: Option<&str>, changes: &str, summary: &str);
    fn add_boot_event(&self, b: &Boot, severity: &str, summary: &str);
    /// Downgrades alerts between `from` and `to` to info; returns how many changed.
    fn mark_shutdown(&self, from: i64, to: i64) -> usize;
    fn boot_recorded(&self, kernel_boot: &str, kind: &str) -> bool;
    /// Newest timestamp stored in a source table, for resuming after a restart.
    fn last_ts(&self, table: &str) -> Option<i64>;
    fn get_state(&self, key: &str) -> Option<String>;
    fn set_state(&self, key: &str, value: Option<&str>);
}

/// The inverse of `ts_text`.
pub fn ts_epoch(ts: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// UTC 'YYYY-MM-DD HH:MM:SS', the format of every `ts` column.
pub fn ts_text(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
