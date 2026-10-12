use super::{conn, record, ts_epoch, ts_text};
use rusqlite::params;

pub fn add_package_change(ts: i64, command: Option<&str>, changes: &str, summary: &str) {
    let ts = ts_text(ts);
    record("packages", &ts, Some(("info", summary)), |tx| {
        tx.execute(
            "INSERT INTO packages (ts, command, changes) VALUES (?1, ?2, ?3)",
            params![ts, command, changes],
        )
        .map(|_| ())
    });
}

/// Whether this exact transaction is already stored.
pub fn package_change_exists(ts: i64, changes: &str) -> bool {
    conn()
        .query_row(
            "SELECT 1 FROM packages WHERE ts = ?1 AND changes = ?2",
            params![ts_text(ts), changes],
            |_| Ok(()),
        )
        .is_ok()
}

/// The newest transaction stored from one package manager. Its summary starts with the manager's
/// name (`apt: upgraded 3 · …`), which keeps two managers on one machine apart.
pub fn last_package_ts(manager: &str) -> Option<i64> {
    conn()
        .query_row(
            "SELECT MAX(ts) FROM messages WHERE source = 'packages' AND summary LIKE ?1 || ': %'",
            params![manager],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .as_deref()
        .and_then(ts_epoch)
}
