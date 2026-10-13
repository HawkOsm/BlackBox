//! Package changes, one row per transaction: after something breaks, the first question is often
//! "what did I upgrade?". Every package manager keeps its own history, so each has a small reader
//! that turns it into the same shape: one line per package, `upgraded linux (6.9-1 -> 6.10-1)`.
//!
//! | manager | history                                        | written                    |
//! |---------|------------------------------------------------|----------------------------|
//! | pacman  | `/var/log/pacman.log`                          | Arch and derivatives       |
//! | apt     | `/var/log/apt/history.log`                     | Debian, Ubuntu, Mint, …    |
//! | zypper  | `/var/log/zypp/history`                        | openSUSE, SLES             |
//! | dnf     | dnf5 or dnf4 history database (SQLite)         | Fedora, RHEL, Rocky, Alma  |
//!
//! Every reader whose history exists runs, so nothing has to be configured. The histories only
//! change while the package manager runs, so they are checked every 30 s instead of being followed
//! by a process that would sit there all day.
//!
//! - `reader.rs`: `start()`, one `Poller` per history, storing what is new every 30 s.
//! - `history.rs`: `History`, what a reader asks of a history, and `detect()`, which finds them.
//! - `text_log.rs`: the `History` for a text log, generic over its line `Parser`.
//! - `pacman.rs`, `apt.rs`, `zypper.rs`: one line `Parser` per text format.
//! - `dnf.rs`: the `History` for dnf's history database.
//! - `summary.rs`: the one-line summary, key packages first.
//! - `report.rs`: what `blackbox --check` prints.
//! - here: `Transaction` and `Parser`, the shapes every file shares.

mod apt;
mod dnf;
mod history;
mod pacman;
mod reader;
mod report;
mod store;
mod summary;
mod text_log;
mod zypper;

use history::History;
pub use reader::start;
pub use report::report;
use summary::summary;
#[cfg(test)]
use text_log::read_all;

/// The change verbs every reader writes, in pacman's words.
pub const ACTIONS: [&str; 5] = [
    "upgraded",
    "installed",
    "removed",
    "downgraded",
    "reinstalled",
];

#[derive(Debug, Default, PartialEq)]
pub struct Transaction {
    pub ts: i64,
    pub command: Option<String>,
    /// One line per package, in the pacman form: `upgraded bash (5.3.15-1 -> 5.3.20-1)`.
    pub changes: Vec<String>,
}

/// Turns the lines of one package manager's log into finished transactions.
pub trait Parser: Default {
    /// Feeds one line; returns the transaction it completes, if any.
    fn line(&mut self, line: &str) -> Option<Transaction>;
    /// The transaction still open once the log has gone quiet, for a format that never says
    /// that a transaction is over.
    fn quiet(&mut self) -> Option<Transaction> {
        None
    }
}

/// `2026-10-12 04:32:17` in the machine's local time, as apt and zypper write it -> epoch seconds.
fn local_ts(text: &str) -> Option<i64> {
    use chrono::TimeZone;
    // apt pads the date and time apart with two spaces
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let naive = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S").ok()?;
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_time_ignores_apts_double_space() {
        assert_eq!(
            local_ts("2026-10-12  04:32:17"),
            local_ts("2026-10-12 04:32:17")
        );
        assert!(local_ts("2026-10-12 04:32:17").is_some());
        assert!(local_ts("yesterday").is_none());
    }
}
