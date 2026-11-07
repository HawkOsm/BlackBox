use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, params};

/// A daemon switched profile, or was seen for the first time (`from` is None).
pub struct ProfileChange {
    pub ts: i64,
    pub daemon: &'static str,
    pub from: Option<String>,
    pub to: String,
}

impl Record for ProfileChange {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO profiles (ts, daemon, profile) VALUES (?1, ?2, ?3)",
            params![ts, self.daemon, self.to],
        )?;
        let summary = match &self.from {
            Some(from) => format!("{} profile: {from} → {}", self.daemon, self.to),
            None => format!("{} profile: {}", self.daemon, self.to),
        };
        index(c, "profiles", &ts, Some((Severity::Info, &summary)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    #[test]
    fn a_switch_is_stored_and_listed_with_both_profiles() {
        let c = open(":memory:");
        ProfileChange {
            ts: 1,
            daemon: "tccd",
            from: Some("Balanced".into()),
            to: "Performance".into(),
        }
        .store(&c)
        .unwrap();
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM profiles WHERE profile = 'Performance'"
            ),
            1
        );
        let summary: String = c
            .query_row(
                "SELECT summary FROM messages WHERE source = 'profiles'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "tccd profile: Balanced → Performance");
    }
}
