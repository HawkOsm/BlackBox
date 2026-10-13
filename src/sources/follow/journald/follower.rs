use super::parse::{Entry, parse_line};
use super::store::Journal;
use crate::database::{self, SetState, Severity};
use crate::sources::follow::Follower;
use crate::sources::kernel::collector::CrashName;
use std::process::Command;

/// One `journalctl` stream: which lines it asks for.
#[derive(Clone, Copy)]
pub(super) struct Feed {
    pub(super) name: &'static str,
    filter: &'static [&'static str],
    severity: Option<Severity>,
}

pub(super) const FEEDS: [Feed; 2] = [
    Feed {
        name: "warnings",
        filter: &["--priority=warning"],
        severity: None,
    },
    // Rust panics are logged at info or notice, below the warning filter, but they are crashes.
    Feed {
        name: "panics",
        filter: &["--priority=notice..info", "--grep=panicked at"],
        severity: Some(Severity::Error),
    },
];

/// One feed being followed, and where it is.
pub(super) struct Journald {
    feed: Feed,
    key: String,
    /// Where it stopped: read once, then kept here, since an entry just sent may still be on its
    /// way to the database.
    cursor: Option<String>,
    since: Option<String>,
    /// How this run of `journalctl` resumed: by time (no cursor), up to which second.
    by_time: bool,
    overlap_until: Option<i64>,
}

impl Journald {
    pub(super) fn new(feed: Feed) -> Self {
        let key = format!("journald.cursor.{}", feed.name);
        Journald {
            feed,
            cursor: database::get_state(&key),
            since: database::last_ts("journald"),
            key,
            by_time: false,
            overlap_until: None,
        }
    }
}

impl Journald {
    /// Whether `journalctl` ended because the journal no longer has the cursor it resumed from.
    fn position_gone(&self, lines: usize) -> bool {
        !self.by_time && lines == 0
    }
}

impl Follower for Journald {
    fn command(&mut self) -> Option<Command> {
        // Resume exactly after the last stored entry. Without a saved cursor (first run), resume
        // by time: the overlapping second may hold entries already stored, so only those are
        // checked for duplicates. Identical messages later on are real repeats and are kept.
        self.by_time = self.cursor.is_none();
        self.overlap_until = self.since.as_deref().and_then(database::ts_epoch);
        let resume = match (&self.cursor, &self.since) {
            (Some(c), _) => format!("--after-cursor={c}"),
            (None, Some(ts)) => format!("--since={ts} UTC"),
            (None, None) => "--lines=0".to_string(),
        };
        let mut command = Command::new("journalctl");
        command
            .args(["--follow", "--output=json", "--no-pager"])
            .args(self.feed.filter)
            .arg(resume);
        Some(command)
    }

    fn line(&mut self, line: &str) {
        let Some(e) = parse_line(line) else {
            return;
        };
        let dedup = self.by_time && self.overlap_until.is_some_and(|t| e.ts <= t);
        if e.cursor.is_some() {
            self.cursor.clone_from(&e.cursor);
        }
        self.since = Some(database::ts_text(e.ts));
        store(e, self.feed, &self.key, dedup);
    }

    fn ended(&mut self, lines: usize) {
        // A following journalctl only ends by itself when it cannot use the cursor (the journal
        // was vacuumed past it): forget it and resume by time instead.
        if self.position_gone(lines) {
            eprintln!("journald: saved position is gone, resuming by time");
            self.cursor = None;
            database::write(SetState {
                key: self.key.clone(),
                value: None,
            });
        }
    }
}

fn store(e: Entry, f: Feed, key: &str, dedup: bool) {
    let routine = crate::sources::routine::journal(e.unit.as_deref(), &e.message);
    database::write(Journal {
        ts: e.ts,
        priority: e.priority,
        unit: e.unit,
        pid: e.pid,
        message: e.message,
        severity: f.severity.or(routine.then_some(Severity::Info)),
        cursor: e.cursor.map(|c| (key.to_string(), c)),
        dedup,
    });
    if let Some(c) = e.coredump {
        database::write(CrashName {
            ts: e.ts,
            pid: c.pid,
            comm: c.comm,
            exe: c.exe,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journald(cursor: Option<&str>, since: Option<&str>) -> Journald {
        Journald {
            feed: FEEDS[0],
            key: "journald.cursor.warnings".to_string(),
            cursor: cursor.map(str::to_string),
            since: since.map(str::to_string),
            by_time: false,
            overlap_until: None,
        }
    }

    fn resume_arg(j: &mut Journald) -> String {
        let command = j.command().unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&"--priority=warning".to_string()),
            "its feed's filter"
        );
        args.last().unwrap().clone()
    }

    #[test]
    fn resumes_by_cursor_then_by_time_then_from_now() {
        let mut j = journald(Some("s=abc"), Some("2026-10-12 04:32:17"));
        assert_eq!(resume_arg(&mut j), "--after-cursor=s=abc");
        assert!(!j.by_time);

        let mut j = journald(None, Some("2026-10-12 04:32:17"));
        assert_eq!(resume_arg(&mut j), "--since=2026-10-12 04:32:17 UTC");
        assert!(j.by_time);
        assert_eq!(j.overlap_until, database::ts_epoch("2026-10-12 04:32:17"));

        // nothing stored yet: only what is logged from now on
        let mut j = journald(None, None);
        assert_eq!(resume_arg(&mut j), "--lines=0");
    }

    #[test]
    fn a_cursor_that_yields_nothing_is_gone() {
        let mut j = journald(Some("s=abc"), None);
        j.command();
        assert!(
            j.position_gone(0),
            "journalctl ended at once: the cursor was vacuumed"
        );
        assert!(
            !j.position_gone(3),
            "it ran and then ended for another reason"
        );
        let mut by_time = journald(None, None);
        by_time.command();
        assert!(!by_time.position_gone(0), "no cursor to forget");
    }
}
