//! `/var/log/apt/history.log`: one `Start-Date:` … `End-Date:` block per apt run, in local time.
//! Covers apt, apt-get, aptitude and unattended-upgrades; a bare `dpkg -i` is not in it.

use super::{Transaction, local_ts};

#[derive(Default)]
pub struct Parser {
    open: Option<Transaction>,
}

fn action(key: &str) -> Option<&'static str> {
    Some(match key {
        "Install" => "installed",
        "Upgrade" => "upgraded",
        "Downgrade" => "downgraded",
        "Remove" | "Purge" => "removed",
        "Reinstall" => "reinstalled",
        _ => return None,
    })
}

/// `bash:amd64 (5.2-1, 5.2-2), foo:amd64 (1.0, automatic)` -> `upgraded bash (5.2-1 -> 5.2-2)`, …
fn changes(action: &str, list: &str) -> Vec<String> {
    list.split("), ")
        .filter_map(|item| {
            let (name, versions) = item.trim_end_matches(')').split_once(" (")?;
            // the architecture says nothing on a single-arch machine and hides the name
            let name = name.split(':').next()?;
            let versions: Vec<&str> = versions.split(", ").filter(|v| *v != "automatic").collect();
            Some(match versions.as_slice() {
                [old, new] => format!("{action} {name} ({old} -> {new})"),
                [v, ..] => format!("{action} {name} ({v})"),
                [] => format!("{action} {name}"),
            })
        })
        .collect()
}

impl super::Parser for Parser {
    fn line(&mut self, line: &str) -> Option<Transaction> {
        let (key, value) = line.split_once(": ")?;
        match key {
            "Start-Date" => {
                self.open = local_ts(value).map(|ts| Transaction {
                    ts,
                    ..Transaction::default()
                });
            }
            "Commandline" => {
                if let Some(t) = self.open.as_mut() {
                    t.command = Some(value.to_string());
                }
            }
            // a run that failed half-way still has an End-Date; keep whatever it changed
            "End-Date" => return self.open.take().filter(|t| !t.changes.is_empty()),
            _ => {
                if let (Some(action), Some(t)) = (action(key), self.open.as_mut()) {
                    t.changes.extend(changes(action, value));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Parser as _, local_ts, read_all, summary};
    use super::*;

    // from an Ubuntu 24.04 container, 2026-10-12
    const LOG: &str = "
Start-Date: 2026-10-12  04:36:13
Commandline: apt-get -y -qq upgrade
Requested-By: osm (1000)
Upgrade: libaudit-common:amd64 (1:3.1.2-2.1build1.1, 1:3.1.2-2.1ubuntu0.1), libaudit1:amd64 (1:3.1.2-2.1build1.1, 1:3.1.2-2.1ubuntu0.1), libssl3t64:amd64 (3.0.13-0ubuntu3.15, 3.0.13-0ubuntu3.16)
End-Date: 2026-10-12  04:36:13

Start-Date: 2026-10-12  04:36:15
Commandline: apt-get -y -qq install less
Install: less:amd64 (590-2ubuntu2.1), libfoo1:amd64 (1.0-1, automatic)
End-Date: 2026-10-12  04:36:15

Start-Date: 2026-10-12  04:36:20
Commandline: apt-get purge tree
Purge: tree:amd64 (2.1.0-1)
End-Date: 2026-10-12  04:36:20

Start-Date: 2026-10-12  04:37:00
Commandline: apt-get install broken
Error: Sub-process /usr/bin/dpkg returned an error code (1)
End-Date: 2026-10-12  04:37:01

Start-Date: 2026-10-12  04:38:00
Commandline: apt-get upgrade
";

    #[test]
    fn one_row_per_finished_run() {
        let done = read_all::<Parser>(LOG);
        assert_eq!(
            done.len(),
            3,
            "the failed run changed nothing and the last one has not ended"
        );
        assert_eq!(done[0].ts, local_ts("2026-10-12 04:36:13").unwrap());
        assert_eq!(done[0].command.as_deref(), Some("apt-get -y -qq upgrade"));
        assert_eq!(
            done[0].changes,
            [
                "upgraded libaudit-common (1:3.1.2-2.1build1.1 -> 1:3.1.2-2.1ubuntu0.1)",
                "upgraded libaudit1 (1:3.1.2-2.1build1.1 -> 1:3.1.2-2.1ubuntu0.1)",
                "upgraded libssl3t64 (3.0.13-0ubuntu3.15 -> 3.0.13-0ubuntu3.16)",
            ]
        );
        assert_eq!(
            done[1].changes,
            [
                "installed less (590-2ubuntu2.1)",
                "installed libfoo1 (1.0-1)"
            ]
        );
        assert_eq!(done[2].changes, ["removed tree (2.1.0-1)"]);
        assert_eq!(summary("apt", &done[1]), "apt: installed 2 · less, libfoo1");
    }

    #[test]
    fn lines_outside_a_run_are_ignored() {
        let mut p = Parser::default();
        assert!(p.line("Upgrade: bash:amd64 (1, 2)").is_none());
        assert!(p.line("End-Date: 2026-10-12  04:36:13").is_none());
        assert!(p.line("garbage").is_none());
    }
}
