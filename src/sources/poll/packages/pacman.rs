//! `/var/log/pacman.log`: one `transaction started` … `transaction completed` block per run, with
//! the command that started it on the line before.

use super::{ACTIONS, Transaction};

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

impl super::Parser for Parser {
    fn line(&mut self, line: &str) -> Option<Transaction> {
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

#[cfg(test)]
mod tests {
    use super::super::{Parser as _, summary};
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
            summary("pacman", t),
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
