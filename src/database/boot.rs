use super::{conn, record, ts_text};
use rusqlite::params;

pub struct Boot<'a> {
    pub ts: i64,
    pub kind: &'a str,
    pub kernel_boot: &'a str,
    pub kernel: Option<&'a str>,
    pub note: Option<&'a str>,
}

pub fn boot_recorded(kernel_boot: &str, kind: &str) -> bool {
    conn()
        .query_row(
            "SELECT 1 FROM boot WHERE kernel_boot = ?1 AND kind = ?2",
            params![kernel_boot, kind],
            |_| Ok(()),
        )
        .is_ok()
}

pub fn add_boot_event(b: &Boot, severity: &str, summary: &str) {
    let ts = ts_text(b.ts);
    record("boot", &ts, Some((severity, summary)), |tx| {
        tx.execute(
            "INSERT INTO boot (ts, kind, kernel_boot, kernel, note) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, b.kind, b.kernel_boot, b.kernel, b.note],
        )
        .map(|_| ())
    });
}

/// Errors and warnings just before a clean shutdown are programs being killed as the session is
/// torn down (Hyprland and Spotify both aborted during one poweroff here), not failures: keep them, as info.
/// Returns the number of rows changed; running it again changes nothing.
pub fn mark_shutdown(from: i64, to: i64) -> usize {
    conn()
        .execute(
            "UPDATE messages SET severity = 'info', summary = summary || ' (during shutdown)'
             WHERE ts BETWEEN ?1 AND ?2 AND severity != 'info'",
            params![ts_text(from), ts_text(to)],
        )
        .unwrap_or(0)
}
