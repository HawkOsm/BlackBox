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

mod apt;
mod dnf;
mod pacman;
mod zypper;

use crate::database;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(30);
/// A log that never marks the end of a transaction (zypper) has its open one stored once the log
/// has been quiet this long. Long enough to outlast a kernel's initramfs rebuild between two lines.
const QUIET: Duration = Duration::from_secs(300);
pub const ACTIONS: [&str; 5] = [
    "upgraded",
    "installed",
    "removed",
    "downgraded",
    "reinstalled",
];
/// Packages that most often explain a machine that stopped working, listed first in a summary.
/// Matched as substrings, so `linux` also covers Debian's `linux-image-…` and `kernel` Fedora's
/// `kernel-core`.
const KEY_PACKAGES: [&str; 15] = [
    "linux",
    "kernel",
    "nvidia",
    "mesa",
    "systemd",
    "glibc",
    "libc6",
    "mkinitcpio",
    "dracut",
    "initramfs",
    "grub",
    "shim",
    "ucode",
    "microcode",
    "firmware",
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Pacman,
    Apt,
    Zypper,
    Dnf,
}

#[derive(Debug)]
pub struct Manager {
    kind: Kind,
    pub path: String,
}

impl Manager {
    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Pacman => "pacman",
            Kind::Apt => "apt",
            Kind::Zypper => "zypper",
            Kind::Dnf => "dnf",
        }
    }
}

const NONE_FOUND: &str = "no pacman, apt, zypper or dnf history found";

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// The package managers whose history exists on this machine; usually exactly one.
pub fn detect() -> Vec<Manager> {
    let mut found: Vec<Manager> = [
        (
            Kind::Pacman,
            env_or("BLACKBOX_PACMAN_LOG", "/var/log/pacman.log"),
        ),
        (
            Kind::Apt,
            env_or("BLACKBOX_APT_LOG", "/var/log/apt/history.log"),
        ),
        (
            Kind::Zypper,
            env_or("BLACKBOX_ZYPPER_LOG", "/var/log/zypp/history"),
        ),
    ]
    .into_iter()
    .filter(|(_, path)| std::path::Path::new(path).exists())
    .map(|(kind, path)| Manager { kind, path })
    .collect();
    if let Some(path) = dnf::db_path() {
        found.push(Manager {
            kind: Kind::Dnf,
            path,
        });
    }
    found
}

fn is_key(name: &str) -> bool {
    KEY_PACKAGES.iter().any(|k| name.contains(k))
}

/// `pacman: upgraded 57, installed 2 · linux 6.9-1 → 6.10-1, nvidia, mesa, bash, …`
pub fn summary(manager: &str, t: &Transaction) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for action in ACTIONS {
        let n = t.changes.iter().filter(|c| c.starts_with(action)).count();
        if n > 0 {
            counts.push((action, n));
        }
    }
    let mut names: Vec<(bool, String)> = t
        .changes
        .iter()
        .filter_map(|c| {
            let mut words = c.split_whitespace();
            let action = words.next()?;
            let name = words.next()?;
            let key = is_key(name);
            // a version is worth the space only for the packages that tend to break things
            let shown = match c.split_once('(').map(|(_, v)| v.trim_end_matches(')')) {
                Some(v) if key && action == "upgraded" && v.contains("->") => {
                    format!("{name} {}", v.replace("->", "→"))
                }
                _ => name.to_string(),
            };
            Some((key, shown))
        })
        .collect();
    names.sort_by_key(|(key, _)| !key);
    let mut listed: Vec<String> = names.iter().take(4).map(|(_, n)| n.clone()).collect();
    if names.len() > 4 {
        listed.push("…".to_string());
    }
    let counted: Vec<String> = counts.iter().map(|(a, n)| format!("{a} {n}")).collect();
    format!("{manager}: {} · {}", counted.join(", "), listed.join(", "))
}

/// Stores a transaction unless it already is: a restart reads the history again from the newest
/// stored second, and two transactions can share that second.
fn store(manager: &str, t: &Transaction) {
    let changes = t.changes.join("\n");
    if database::package_change_exists(t.ts, &changes) {
        return;
    }
    database::add_package_change(t.ts, t.command.as_deref(), &changes, &summary(manager, t));
}

pub fn start() {
    let managers = detect();
    if managers.is_empty() {
        eprintln!("packages: {NONE_FOUND}; package changes are not recorded");
    }
    for m in managers {
        std::thread::spawn(move || match m.kind {
            Kind::Pacman => follow::<pacman::Parser>(&m),
            Kind::Apt => follow::<apt::Parser>(&m),
            Kind::Zypper => follow::<zypper::Parser>(&m),
            Kind::Dnf => dnf::follow(&m),
        });
    }
}

/// Transactions older than this are already stored (the whole history is read at startup).
fn stored_until(manager: &str) -> i64 {
    database::last_package_ts(manager).unwrap_or(i64::MIN)
}

/// Reads a text log from the top, then every `POLL` reads what was appended since.
fn follow<P: Parser>(m: &Manager) {
    let stored_until = stored_until(m.name());
    let mut parser = P::default();
    let mut offset = 0u64;
    let mut pending = String::new();
    let mut grew = Instant::now();
    let handle = |t: Transaction| {
        if t.ts >= stored_until {
            store(m.name(), &t);
        }
    };
    loop {
        if let Ok(mut file) = std::fs::File::open(&m.path) {
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            if len < offset {
                // rotated or truncated: start over from the top
                offset = 0;
                pending.clear();
                parser = P::default();
            }
            let mut bytes = Vec::new();
            if len > offset
                && file.seek(SeekFrom::Start(offset)).is_ok()
                && file.read_to_end(&mut bytes).is_ok()
            {
                offset += bytes.len() as u64;
                grew = Instant::now();
                pending.push_str(&String::from_utf8_lossy(&bytes));
                // only complete lines; a half-written one waits for the next check
                let complete = pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
                for line in pending[..complete].lines() {
                    if let Some(t) = parser.line(line) {
                        handle(t);
                    }
                }
                pending.drain(..complete);
            } else if grew.elapsed() >= QUIET
                && let Some(t) = parser.quiet()
            {
                handle(t);
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Every transaction in a whole text log, the open one at the end included.
fn read_all<P: Parser>(text: &str) -> Vec<Transaction> {
    let mut parser = P::default();
    let mut found: Vec<Transaction> = text.lines().filter_map(|l| parser.line(l)).collect();
    found.extend(parser.quiet());
    found
}

/// Every transaction in one manager's whole history, read from scratch.
fn history(m: &Manager) -> Result<Vec<Transaction>, String> {
    let parse: fn(&str) -> Vec<Transaction> = match m.kind {
        Kind::Pacman => read_all::<pacman::Parser>,
        Kind::Apt => read_all::<apt::Parser>,
        Kind::Zypper => read_all::<zypper::Parser>,
        Kind::Dnf => return dnf::transactions(&m.path, i64::MIN).map_err(|e| e.to_string()),
    };
    let bytes = std::fs::read(&m.path).map_err(|e| e.to_string())?;
    Ok(parse(&String::from_utf8_lossy(&bytes)))
}

/// For `blackbox --check`: what each detected manager's history holds, read from scratch.
pub fn report() -> Vec<String> {
    let managers = detect();
    if managers.is_empty() {
        return vec![format!("{NONE_FOUND}: package changes are not recorded")];
    }
    managers
        .iter()
        .map(|m| match history(m) {
            Err(e) => format!("{} ({}): not readable: {e}", m.name(), m.path),
            Ok(all) => match all.iter().max_by_key(|t| t.ts) {
                None => format!("{} ({}): readable, no transactions yet", m.name(), m.path),
                Some(newest) => format!(
                    "{} ({}): {} transactions, newest {}: {}",
                    m.name(),
                    m.path,
                    all.len(),
                    database::ts_text(newest.ts),
                    summary(m.name(), newest)
                ),
            },
        })
        .collect()
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

    fn t(changes: &[&str]) -> Transaction {
        Transaction {
            ts: 0,
            command: None,
            changes: changes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn summary_names_the_manager_and_lists_key_packages_first() {
        let s = summary(
            "apt",
            &t(&[
                "upgraded bash (5.2-1 -> 5.2-2)",
                "upgraded linux-image-6.1.0-10-amd64 (6.1.38-1 -> 6.1.38-2)",
                "installed foo (1.0)",
            ]),
        );
        assert_eq!(
            s,
            "apt: upgraded 2, installed 1 · linux-image-6.1.0-10-amd64 6.1.38-1 → 6.1.38-2, bash, foo"
        );
    }

    #[test]
    fn a_key_package_without_an_old_version_shows_no_arrow() {
        // zypper logs an upgrade as an install of the new version only
        assert_eq!(
            summary("zypper", &t(&["installed kernel-default (6.11-1.1)"])),
            "zypper: installed 1 · kernel-default"
        );
    }

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
