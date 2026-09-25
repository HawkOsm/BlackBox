use super::{record, ts_text};
use rusqlite::params;

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
