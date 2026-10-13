use crate::database::{Record, Severity, exists, index, ts_text, upsert_state};
use rusqlite::{Connection, params};

/// One journal entry to store.
pub struct Journal {
    pub ts: i64,
    pub priority: i32,
    pub unit: Option<String>,
    pub pid: Option<i32>,
    pub message: String,
    /// Overrides the severity the priority implies (a panic logged at info is still a crash).
    pub severity: Option<Severity>,
    /// The follower's state key and its place in the journal, saved with the row so a restart
    /// resumes exactly.
    pub cursor: Option<(String, String)>,
    /// Skip it if an identical row exists: only needed where a time-based resume overlaps.
    pub dedup: bool,
}

fn journald_duplicate(
    c: &Connection,
    ts: &str,
    unit: Option<&str>,
    message: &str,
) -> rusqlite::Result<bool> {
    exists(
        c,
        "SELECT 1 FROM journald WHERE ts = ?1 AND message = ?2 AND IFNULL(unit, '') = IFNULL(?3, '') LIMIT 1",
        params![ts, message, unit],
    )
}

impl Record for Journal {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        let unit = self.unit.as_deref();
        if self.dedup && journald_duplicate(c, &ts, unit, &self.message)? {
            return Ok(());
        }
        // syslog priorities: 0 to 3 are errors, 4 a warning; below that only an override stores one
        let severity = self.severity.or(match self.priority {
            ..=3 => Some(Severity::Error),
            4 => Some(Severity::Warning),
            _ => None,
        });
        let summary = format!(
            "{}: {}",
            unit.unwrap_or("system"),
            self.message
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>()
        );
        // before the row itself: `index` takes the row's id from the last insert
        if let Some((key, cursor)) = &self.cursor {
            upsert_state(c, key, cursor)?;
        }
        c.execute(
            "INSERT INTO journald (ts, priority, unit, pid, message) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, self.priority, unit, self.pid, self.message],
        )?;
        index(c, "journald", &ts, severity.map(|s| (s, summary.as_str())))
    }
}

/// Applies `routine::JOURNAL` to journal lines stored before a rule existed, so the history reads
/// the same as new lines. It scans the whole table, so it runs only when the rules have changed.
pub struct DemoteRoutine;

impl Record for DemoteRoutine {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        const KEY: &str = "journald.routine";
        let rules = &crate::sources::routine::JOURNAL;
        let fingerprint: String = rules
            .iter()
            .map(|r| format!("{}\t{}\n", r.unit.unwrap_or(""), r.text))
            .collect();
        let done: Option<String> = c
            .query_row("SELECT value FROM state WHERE key = ?1", [KEY], |r| {
                r.get(0)
            })
            .ok();
        if done.as_deref() == Some(fingerprint.as_str()) {
            return Ok(());
        }
        let mut demoted = 0;
        for r in rules.iter() {
            demoted += c.execute(
                "UPDATE messages SET severity = ?3
                 WHERE source = 'journald' AND severity != ?3 AND ref_id IN (
                     SELECT journald_id FROM journald
                     WHERE instr(message, ?1) > 0 AND (?2 IS NULL OR unit = ?2))",
                params![r.text, r.unit, Severity::Info],
            )?;
        }
        upsert_state(c, KEY, &fingerprint)?;
        if demoted > 0 {
            eprintln!("journald: {demoted} stored lines now count as routine");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::open;

    fn journal(ts: i64, unit: &str, message: &str, cursor: Option<(&str, &str)>) -> Journal {
        Journal {
            ts,
            priority: if cursor.is_some() { 3 } else { 1 },
            unit: Some(unit.to_string()),
            pid: Some(5),
            message: message.to_string(),
            severity: None,
            cursor: cursor.map(|(k, c)| (k.to_string(), c.to_string())),
            dedup: false,
        }
    }

    fn state(c: &Connection, key: &str) -> Option<String> {
        c.query_row("SELECT value FROM state WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .ok()
    }

    #[test]
    fn the_cursor_is_saved_with_the_row_without_shifting_its_id() {
        let c = open(":memory:");
        journal(
            1_700_000_100,
            "unit",
            "boom",
            Some(("journald.cursor.test", "s=abc")),
        )
        .store(&c)
        .unwrap();
        assert_eq!(state(&c, "journald.cursor.test").as_deref(), Some("s=abc"));
        let (ref_id, id): (i64, i64) = c
            .query_row(
                "SELECT ref_id, journald_id FROM messages, journald WHERE source = 'journald'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            ref_id, id,
            "the alert points at the journal row, not the state row"
        );
    }

    #[test]
    fn a_line_already_stored_in_an_overlapping_second_is_skipped() {
        let c = open(":memory:");
        let line = || Journal {
            dedup: true,
            ..journal(1_700_000_100, "unit", "boom", None)
        };
        line().store(&c).unwrap();
        line().store(&c).unwrap();
        assert_eq!(
            crate::database::count(&c, "SELECT COUNT(*) FROM journald"),
            1
        );
    }

    #[test]
    fn routine_lines_stored_as_errors_are_demoted_once() {
        let c = open(":memory:");
        let probe = "  alice : a password is required ; TTY=pts/0 ; PWD=/ ; USER=root ; COMMAND=/usr/bin/true";
        journal(1_700_000_100, "unit", "boom", None)
            .store(&c)
            .unwrap();
        journal(1_700_000_200, "session-2.scope", probe, None)
            .store(&c)
            .unwrap();
        let severity_of = |text: &str| -> String {
            c.query_row(
                "SELECT m.severity FROM messages m JOIN journald j ON m.ref_id = j.journald_id
                 WHERE m.source = 'journald' AND j.message = ?1",
                [text],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(severity_of(probe), "error");
        DemoteRoutine.store(&c).unwrap();
        assert_eq!(severity_of(probe), "info");
        assert_eq!(severity_of("boom"), "error", "unrelated lines keep theirs");
        assert!(
            state(&c, "journald.routine").is_some(),
            "and it is not run again"
        );
    }
}
