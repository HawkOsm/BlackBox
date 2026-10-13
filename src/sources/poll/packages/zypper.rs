//! `/var/log/zypp/history`: `|`-separated lines in local time. A `command` line starts a run, then
//! one `install` or `remove` line per package. Nothing marks the end of a run, so it is closed by the
//! next `command` or by the log going quiet. An upgrade is logged as an install of the new version.

use super::{Transaction, local_ts};

#[derive(Default)]
pub struct Parser {
    open: Option<Transaction>,
}

impl super::Parser for Parser {
    fn line(&mut self, line: &str) -> Option<Transaction> {
        if line.starts_with('#') {
            return None;
        }
        let fields: Vec<&str> = line.split('|').collect();
        let kind = fields.get(1)?.trim();
        if !matches!(kind, "command" | "install" | "remove") {
            return None; // repository changes, patches
        }
        let ts = local_ts(fields.first()?)?;
        let action = match kind {
            "command" => {
                // `'zypper' '-n' 'in' 'tree'` -> `zypper -n in tree`
                let command = fields.get(3).map(|c| c.replace('\'', ""));
                let done = self.quiet();
                self.open = Some(Transaction {
                    ts,
                    command,
                    changes: Vec::new(),
                });
                return done;
            }
            "install" => "installed",
            _ => "removed",
        };
        let (name, version) = (fields.get(2)?, fields.get(3)?);
        self.open
            .get_or_insert_with(|| Transaction {
                ts,
                ..Transaction::default()
            })
            .changes
            .push(format!("{action} {name} ({version})"));
        None
    }

    fn quiet(&mut self) -> Option<Transaction> {
        self.open.take().filter(|t| !t.changes.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{local_ts, read_all};
    use super::*;

    // from an openSUSE Tumbleweed container, 2026-10-12 (checksums shortened)
    const LOG: &str = "\
# 2026-10-12 04:35:50 some-package.rpm installed ok
2026-10-12 04:35:56|command|root@380d5ed8d57d|'zypper' '-q' '-n' 'in' 'tree'|
2026-10-12 04:35:56|install|tree|2.3.2-1.3|x86_64|root@380d5ed8d57d|repo-oss|c7319b87|
2026-10-12 04:35:57|command|root@380d5ed8d57d|'zypper' '-q' '-n' 'rm' 'tree'|
2026-10-12 04:35:57|remove |tree|2.3.2-1.3|x86_64|root@380d5ed8d57d|
2026-10-12 04:36:30|radd   |repo-new|https://example.org/repo|
";

    #[test]
    fn a_run_ends_at_the_next_command_or_when_the_log_goes_quiet() {
        let done = read_all::<Parser>(LOG);
        assert_eq!(done.len(), 2);
        assert_eq!(done[0].ts, local_ts("2026-10-12 04:35:56").unwrap());
        assert_eq!(done[0].command.as_deref(), Some("zypper -q -n in tree"));
        assert_eq!(done[0].changes, ["installed tree (2.3.2-1.3)"]);
        assert_eq!(done[1].command.as_deref(), Some("zypper -q -n rm tree"));
        assert_eq!(done[1].changes, ["removed tree (2.3.2-1.3)"]);
    }

    #[test]
    fn packages_without_a_command_line_still_form_a_run() {
        // YaST and PackageKit do not always log a command first
        let done = read_all::<Parser>(
            "2026-10-12 05:00:00|install|vim|9.1-1.1|x86_64|root@h|repo-oss|ab|\n",
        );
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].command, None);
    }
}
