use super::history::{History, NONE_FOUND, detect};
use super::store::{PackageChange, last_package_ts};
use super::{Transaction, summary};
use crate::database;
use crate::sources::poll::{Poller, poll};
use crate::sources::supervise;
use std::time::Duration;

/// Stores a transaction; the writer skips it if it is already stored.
fn store(manager: &str, t: &Transaction) {
    database::write(PackageChange {
        ts: t.ts,
        command: t.command.clone(),
        changes: t.changes.join("\n"),
        summary: summary(manager, t),
    });
}

pub fn start() {
    let histories = detect(stored_until);
    if histories.is_empty() {
        eprintln!("packages: {NONE_FOUND}; package changes are not recorded");
    }
    for history in histories {
        let name = format!("packages.{}", history.name());
        let mut reader = Reader {
            history,
            failing: false,
        };
        supervise(&name, move || poll(&mut reader));
    }
}

/// Transactions older than this are already stored (the whole history is read at startup).
fn stored_until(manager: &str) -> i64 {
    last_package_ts(manager).unwrap_or(i64::MIN)
}

/// One package manager's history, read every 30 s.
struct Reader {
    history: Box<dyn History>,
    /// Whether the last read failed: an error is reported once, not every 30 s.
    failing: bool,
}

impl Poller for Reader {
    fn every(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn poll(&mut self) {
        match self.history.poll() {
            Ok(found) => {
                for t in &found {
                    store(self.history.name(), t);
                }
                self.failing = false;
            }
            Err(e) if !self.failing => {
                eprintln!("packages: cannot read {}: {e}", self.history.path());
                self.failing = true;
            }
            Err(_) => {}
        }
    }
}
