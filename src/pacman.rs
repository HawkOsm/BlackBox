//! Package changes from pacman's log, one row per transaction: after something breaks, the first
//! question is often "what did I upgrade?". The log only changes while pacman runs, so it is
//! checked every 30 s instead of being followed by a process that would sit there all day.

use crate::database;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(30);
const ACTIONS: [&str; 5] = [
    "upgraded",
    "installed",
    "removed",
    "downgraded",
    "reinstalled",
];
/// Packages that most often explain a machine that stopped working, listed first in a summary.
const KEY_PACKAGES: [&str; 9] = [
    "linux",
    "nvidia",
    "mesa",
    "systemd",
    "glibc",
    "mkinitcpio",
    "grub",
    "ucode",
    "firmware",
];

fn log_path() -> String {
    std::env::var("BLACKBOX_PACMAN_LOG").unwrap_or_else(|_| "/var/log/pacman.log".to_string())
}

#[derive(Debug, Default)]
pub struct Transaction {
    pub ts: i64,
    pub command: Option<String>,
    pub changes: Vec<String>,
}

/// Turns log lines into finished transactions.
#[derive(Default)]
pub struct Parser {
    command: Option<String>,
    open: Option<Transaction>,
}

/// `[2026-09-22T22:10:41+0300] [ALPM] upgraded bash (5.3.15-1 -> 5.3.20-1)` -> (time, rest)
fn split(line: &str) -> Option<(i64, &str)> {
    let (stamp, rest) = line.strip_prefix('[')?.split_once("] ")?;
    let ts = chrono::DateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S%z").ok()?;
    Some((ts.timestamp(), rest))
}

impl Parser {
    /// Feeds one line; returns the transaction it completes, if any.
    pub fn line(&mut self, line: &str) -> Option<Transaction> {
        let (ts, rest) = split(line)?;
        if let Some(cmd) = rest.strip_prefix("[PACMAN] Running '") {
            self.command = Some(cmd.trim_end_matches('\'').to_string());
            return None;
        }
        let msg = rest.strip_prefix("[ALPM] ")?;
        if msg == "transaction started" {
            self.open = Some(Transaction {
                ts,
                command: self.command.take(),
                changes: Vec::new(),
            });
            return None;
        }
        if msg.starts_with("transaction ") {
            // completed, failed or interrupted: keep whatever did change
            return self.open.take().filter(|t| !t.changes.is_empty());
        }
        let action = msg.split_whitespace().next()?;
        if ACTIONS.contains(&action)
            && let Some(t) = self.open.as_mut()
        {
            t.changes.push(msg.to_string());
        }
        None
    }
}

fn is_key(name: &str) -> bool {
    KEY_PACKAGES.iter().any(|k| name.contains(k))
}

/// `pacman: upgraded 57, installed 2 · linux 6.9-1 → 6.10-1, nvidia, mesa, bash, …`
pub fn summary(t: &Transaction) -> String {
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
                Some(v) if key && action == "upgraded" => {
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
    format!("pacman: {} · {}", counted.join(", "), listed.join(", "))
}

fn store(t: &Transaction) {
    database::add_package_change(
        t.ts,
        t.command.as_deref(),
        &t.changes.join("\n"),
        &summary(t),
    );
}

pub fn start() {
    std::thread::spawn(|| {
        let path = log_path();
        // transactions up to this time are already stored (the whole log is read at startup)
        let mut stored_until = database::last_ts("packages")
            .as_deref()
            .and_then(database::ts_epoch)
            .unwrap_or(i64::MIN);
        let mut parser = Parser::default();
        let mut offset = 0u64;
        let mut pending = String::new();
        loop {
            if let Ok(mut file) = std::fs::File::open(&path) {
                let len = file.metadata().map(|m| m.len()).unwrap_or(0);
                if len < offset {
                    // rotated or truncated: start over from the top
                    offset = 0;
                    pending.clear();
                    parser = Parser::default();
                }
                let mut bytes = Vec::new();
                if len > offset
                    && file.seek(SeekFrom::Start(offset)).is_ok()
                    && file.read_to_end(&mut bytes).is_ok()
                {
                    offset += bytes.len() as u64;
                    pending.push_str(&String::from_utf8_lossy(&bytes));
                    // only complete lines; a half-written one waits for the next check
                    let complete = pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
                    for line in pending[..complete].lines() {
                        if let Some(t) = parser.line(line)
                            && t.ts > stored_until
                        {
                            store(&t);
                            stored_until = t.ts;
                        }
                    }
                    pending.drain(..complete);
                }
            }
            std::thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
[2026-09-22T22:10:40+0300] [PACMAN] Running 'pacman -Syu'
[2026-09-22T22:10:41+0300] [ALPM] transaction started
[2026-09-22T22:10:41+0300] [ALPM] upgraded tzdata (2026c-1 -> 2026d-1)
[2026-09-22T22:10:41+0300] [ALPM] upgraded linux (7.2.5.arch1-1 -> 7.2.6.arch2-1)
[2026-09-22T22:10:41+0300] [ALPM] upgraded bash (5.3.15-1 -> 5.3.20-1)
[2026-09-22T22:10:42+0300] [ALPM] installed foo (1.0-1)
[2026-09-22T22:10:42+0300] [ALPM] upgraded bar (1-1 -> 2-1)
[2026-09-22T22:10:42+0300] [ALPM] upgraded baz (1-1 -> 2-1)
[2026-09-22T22:10:43+0300] [ALPM] transaction completed
[2026-09-22T22:10:43+0300] [ALPM] running '30-systemd-daemon-reload-system.hook'...
[2026-09-22T22:11:00+0300] [ALPM] transaction started
[2026-09-22T22:11:00+0300] [ALPM] transaction failed
";

    #[test]
    fn one_row_per_transaction_with_key_packages_first() {
        let mut p = Parser::default();
        let done: Vec<Transaction> = LOG.lines().filter_map(|l| p.line(l)).collect();
        assert_eq!(
            done.len(),
            1,
            "a failed transaction that changed nothing is not stored"
        );
        let t = &done[0];
        assert_eq!(t.ts, 1_790_104_241); // 22:10:41 +03:00
        assert_eq!(t.command.as_deref(), Some("pacman -Syu"));
        assert_eq!(t.changes.len(), 6);
        assert_eq!(
            summary(t),
            "pacman: upgraded 5, installed 1 · linux 7.2.5.arch1-1 → 7.2.6.arch2-1, tzdata, bash, foo, …"
        );
    }

    #[test]
    fn ignores_lines_it_does_not_know() {
        let mut p = Parser::default();
        assert!(p.line("garbage").is_none());
        assert!(
            p.line("[2019-01-01 10:00] [ALPM] transaction completed")
                .is_none()
        );
    }
}
