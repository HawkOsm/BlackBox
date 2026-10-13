use crate::database::{Record, Severity, exists, index, read, ts_epoch, ts_text};
use rusqlite::{Connection, params};

/// One package manager transaction.
pub struct PackageChange {
    pub ts: i64,
    pub command: Option<String>,
    /// One line per package: `upgraded linux (6.9-1 -> 6.10-1)`.
    pub changes: String,
    pub summary: String,
}

/// Stored unless the same transaction is already stored: a restart reads the history again from
/// the newest stored second, and two transactions can share that second.
impl Record for PackageChange {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        if package_change_exists(c, self.ts, &self.changes)? {
            return Ok(());
        }
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO packages (ts, command, changes) VALUES (?1, ?2, ?3)",
            params![ts, self.command, self.changes],
        )?;
        index(c, "packages", &ts, Some((Severity::Info, &self.summary)))
    }
}

/// Whether this exact transaction is already stored.
fn package_change_exists(c: &Connection, ts: i64, changes: &str) -> rusqlite::Result<bool> {
    exists(
        c,
        "SELECT 1 FROM packages WHERE ts = ?1 AND changes = ?2",
        params![ts_text(ts), changes],
    )
}

/// The newest transaction stored from one package manager. Its summary starts with the manager's
/// name (`apt: upgraded 3 · …`), which keeps two managers on one machine apart.
pub fn last_package_ts(manager: &str) -> Option<i64> {
    read(|c| newest(c, manager))
}

fn newest(c: &Connection, manager: &str) -> Option<i64> {
    c.query_row(
        "SELECT MAX(ts) FROM messages WHERE source = 'packages' AND summary LIKE ?1 || ': %'",
        params![manager],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .as_deref()
    .and_then(ts_epoch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{commit, count, open};

    fn change(ts: i64, changes: &str, summary: &str) -> Box<dyn Record> {
        Box::new(PackageChange {
            ts,
            command: None,
            changes: changes.to_string(),
            summary: summary.to_string(),
        })
    }

    #[test]
    fn a_transaction_is_stored_once_and_resumed_per_manager() {
        let mut c = open(":memory:");
        let bash = "upgraded bash (1 -> 2)";
        // the same transaction twice in one batch, and again in the next
        commit(
            &mut c,
            vec![
                change(1_700_000_050, bash, "apt: upgraded 1 · bash"),
                change(1_700_000_050, bash, "apt: upgraded 1 · bash"),
                change(1_700_000_060, "removed x (1)", "pacman: removed 1 · x"),
            ],
        );
        commit(
            &mut c,
            vec![change(1_700_000_050, bash, "apt: upgraded 1 · bash")],
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM packages"), 2);
        // a different transaction in the same second is not a duplicate
        commit(
            &mut c,
            vec![change(
                1_700_000_050,
                "upgraded bash (2 -> 3)",
                "apt: upgraded 1 · bash",
            )],
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM packages"), 3);

        assert_eq!(newest(&c, "apt"), Some(1_700_000_050));
        assert_eq!(newest(&c, "pacman"), Some(1_700_000_060));
        assert_eq!(newest(&c, "dnf"), None);
    }
}
