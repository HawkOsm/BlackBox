use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, params};

/// One audit record worth keeping, and its alert if it is one.
pub struct Audit {
    pub ts: i64,
    pub event_type: String,
    pub pid: Option<i32>,
    pub uid: Option<i32>,
    pub executable: Option<String>,
    /// Severity and summary.
    pub alert: Option<(Severity, String)>,
}

impl Record for Audit {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO auditd (ts, event_type, pid, uid, executable) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, self.event_type, self.pid, self.uid, self.executable],
        )?;
        let alert = self.alert.as_ref().map(|(sev, text)| (*sev, text.as_str()));
        index(c, "auditd", &ts, alert)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    #[test]
    fn a_failed_login_is_stored_with_its_alert() {
        let c = open(":memory:");
        Audit {
            ts: 1_700_000_100,
            event_type: "USER_AUTH".to_string(),
            pid: Some(1),
            uid: Some(1000),
            executable: Some("/bin/su".to_string()),
            alert: Some((Severity::Warning, "failed auth".to_string())),
        }
        .store(&c)
        .unwrap();
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM messages JOIN auditd ON ref_id = audit_id
                 WHERE source = 'auditd' AND severity = 'warning'"
            ),
            1
        );
    }
}
