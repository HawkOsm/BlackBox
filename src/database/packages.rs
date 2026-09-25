use super::{record, ts_text};
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
