//! dnf keeps no plain-text package log worth parsing (dnf5 none at all), but both generations keep a
//! history database that a normal user may read: dnf5 (Fedora 41+) in
//! `/usr/lib/sysimage/libdnf5/transaction_history.sqlite`, dnf4 (RHEL, Rocky and Alma 8–10) in
//! `/var/lib/dnf/history.sqlite`. It is opened read-only, and only when it has changed.
//!
//! An upgrade is two rows there, the new version and the one it replaced; they are paired back into
//! one `upgraded name (old -> new)` line.

use super::{History, Transaction};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::time::SystemTime;

const DNF5: &str = "/usr/lib/sysimage/libdnf5/transaction_history.sqlite";
const DNF4: &str = "/var/lib/dnf/history.sqlite";

/// The database dnf writes on this machine. dnf5 wins: a machine upgraded from dnf4 keeps the old,
/// no longer written file.
pub fn db_path() -> Option<String> {
    if let Ok(path) = std::env::var("BLACKBOX_DNF_DB") {
        return Some(path);
    }
    [DNF5, DNF4]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(str::to_string)
}

/// What one history row means for the summary.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// a change on its own: installed, removed, reinstalled
    Changed(&'static str),
    /// the new side of an upgrade or downgrade
    New(&'static str),
    /// the version an upgrade, downgrade or obsoleting package replaced
    Old,
    Skip,
}

/// One package row of a transaction: what it means, the name, and `[epoch:]version-release`.
type Item = (Role, String, String);

/// dnf5's `trans_item_action` table.
fn role_dnf5(action: i64) -> Role {
    match action {
        1 => Role::Changed("installed"),
        2 => Role::New("upgraded"),
        3 => Role::New("downgraded"),
        4 => Role::Changed("reinstalled"),
        5 => Role::Changed("removed"),
        6 => Role::Old,  // Replaced
        _ => Role::Skip, // reason change
    }
}

/// dnf4's `TransactionItemAction` enum (libdnf), which has no table of its own.
fn role_dnf4(action: i64) -> Role {
    match action {
        1 => Role::Changed("installed"),
        2 => Role::New("downgraded"),
        4 => Role::Changed("installed"), // Obsolete: the package that replaces another
        6 => Role::New("upgraded"),
        8 => Role::Changed("removed"),
        9 => Role::Changed("reinstalled"),
        3 | 5 | 7 => Role::Old, // Downgraded, Obsoleted, Upgraded
        _ => Role::Skip,        // 10 Reinstalled (the same version again), 11 reason change
    }
}

/// Finished transactions that began at or after `since` (epoch seconds), oldest first.
pub fn transactions(path: &str, since: i64) -> rusqlite::Result<Vec<Transaction>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    read(&conn, since)
}

fn read(conn: &Connection, since: i64) -> rusqlite::Result<Vec<Transaction>> {
    let dnf5 = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE name = 'pkg_name'")?
        .exists([])?;
    // dt_end is only set once a transaction is over; dnf5 also marks a crashed one as still Started
    let sql = if dnf5 {
        "SELECT t.id, t.dt_begin, t.description, ti.action_id, n.name, r.epoch, r.version, r.release
         FROM trans t JOIN trans_item ti ON ti.trans_id = t.id JOIN rpm r ON r.item_id = ti.item_id
         JOIN pkg_name n ON n.id = r.name_id
         WHERE t.dt_end IS NOT NULL AND t.state_id != 1 AND t.dt_begin >= ?1 ORDER BY t.id, ti.id"
    } else {
        "SELECT t.id, t.dt_begin, t.cmdline, ti.action, r.name, r.epoch, r.version, r.release
         FROM trans t JOIN trans_item ti ON ti.trans_id = t.id JOIN rpm r ON r.item_id = ti.item_id
         WHERE t.dt_end IS NOT NULL AND t.dt_begin >= ?1 ORDER BY t.id, ti.id"
    };
    let role = if dnf5 { role_dnf5 } else { role_dnf4 };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([since], |r| {
        let epoch: i64 = r.get(5)?;
        let (version, release): (String, String) = (r.get(6)?, r.get(7)?);
        let evr = if epoch > 0 {
            format!("{epoch}:{version}-{release}")
        } else {
            format!("{version}-{release}")
        };
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
            role(r.get(3)?),
            r.get::<_, String>(4)?,
            evr,
        ))
    })?;
    let mut found: Vec<(i64, Transaction, Vec<Item>)> = Vec::new();
    for row in rows {
        let (id, ts, command, role, name, evr) = row?;
        if found.last().is_none_or(|(last, _, _)| *last != id) {
            let command = command.filter(|c| !c.is_empty());
            let t = Transaction {
                ts,
                command,
                changes: Vec::new(),
            };
            found.push((id, t, Vec::new()));
        }
        found.last_mut().unwrap().2.push((role, name, evr));
    }
    Ok(found
        .into_iter()
        .map(|(_, mut t, items)| {
            t.changes = pair(&items);
            t
        })
        .filter(|t| !t.changes.is_empty())
        .collect())
}

/// One line per package, an upgrade's two rows folded into `upgraded name (old -> new)`.
fn pair(items: &[Item]) -> Vec<String> {
    let mut old: HashMap<&str, &str> = items
        .iter()
        .filter(|(role, _, _)| *role == Role::Old)
        .map(|(_, name, evr)| (name.as_str(), evr.as_str()))
        .collect();
    let mut lines = Vec::new();
    for (role, name, evr) in items {
        match role {
            Role::Changed(action) => lines.push(format!("{action} {name} ({evr})")),
            Role::New(action) => match old.remove(name.as_str()) {
                Some(was) => lines.push(format!("{action} {name} ({was} -> {evr})")),
                None => lines.push(format!("{action} {name} ({evr})")),
            },
            Role::Old | Role::Skip => {}
        }
    }
    // replaced by a package of another name (an obsoleting one): it is gone
    for (role, name, evr) in items {
        if *role == Role::Old && old.remove(name.as_str()).is_some() {
            lines.push(format!("removed {name} ({evr})"));
        }
    }
    lines
}

fn modified(path: &str) -> Option<SystemTime> {
    // a commit lands in the -wal file first; the main file changes only at a checkpoint
    [path.to_string(), format!("{path}-wal")]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
}

/// dnf's history database, read again only when it (or its `-wal` file) has changed.
pub struct Dnf {
    path: String,
    since: i64,
    seen: Option<SystemTime>,
}

impl Dnf {
    pub fn new(path: String, since: i64) -> Self {
        Dnf {
            path,
            since,
            seen: None,
        }
    }
}

impl History for Dnf {
    fn name(&self) -> &'static str {
        "dnf"
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn poll(&mut self) -> Result<Vec<Transaction>, String> {
        let now = modified(&self.path);
        if now.is_none() || now == self.seen {
            return Ok(Vec::new());
        }
        let found = transactions(&self.path, self.since).map_err(|e| e.to_string())?;
        // only once read: a failed read is tried again at the next check
        self.seen = now;
        if let Some(newest) = found.iter().map(|t| t.ts).max() {
            self.since = self.since.max(newest);
        }
        Ok(found)
    }

    fn history(&self) -> Result<Vec<Transaction>, String> {
        transactions(&self.path, i64::MIN).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::summary;
    use super::*;

    /// The tables and columns this reader uses, as dnf5 5.4 and dnf4 4.14 create them.
    const DNF5_SCHEMA: &str = "
        CREATE TABLE trans (id INTEGER PRIMARY KEY, dt_begin INTEGER NOT NULL, dt_end INTEGER,
            description TEXT, state_id INTEGER);
        CREATE TABLE trans_item (id INTEGER PRIMARY KEY, trans_id INTEGER, item_id INTEGER,
            action_id INTEGER NOT NULL);
        CREATE TABLE pkg_name (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
        CREATE TABLE rpm (item_id INTEGER NOT NULL UNIQUE, name_id INTEGER NOT NULL,
            epoch INTEGER NOT NULL, version TEXT NOT NULL, release TEXT NOT NULL);";
    const DNF4_SCHEMA: &str = "
        CREATE TABLE trans (id INTEGER PRIMARY KEY, dt_begin INTEGER NOT NULL, dt_end INTEGER,
            cmdline TEXT, state INTEGER NOT NULL);
        CREATE TABLE trans_item (id INTEGER PRIMARY KEY, trans_id INTEGER, item_id INTEGER,
            action INTEGER NOT NULL);
        CREATE TABLE rpm (item_id INTEGER UNIQUE NOT NULL, name TEXT NOT NULL,
            epoch INTEGER NOT NULL, version TEXT NOT NULL, release TEXT NOT NULL);";

    #[test]
    fn dnf5_pairs_an_upgrade_with_the_version_it_replaced() {
        // rows as a Fedora 44 container recorded `dnf upgrade glib2`, plus an unfinished run
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(DNF5_SCHEMA).unwrap();
        c.execute_batch(
            "INSERT INTO pkg_name VALUES (1, 'glib2'), (2, 'tree'), (3, 'kernel-core');
             INSERT INTO rpm VALUES (10, 1, 0, '2.88.4', '1.fc44'), (11, 1, 0, '2.88.3', '1.fc44'),
                                    (12, 2, 0, '2.2.1', '4.fc44'), (13, 3, 0, '7.0', '1.fc44');
             INSERT INTO trans VALUES (3, 100, 100, 'dnf install -y -q tree', 2),
                                      (4, 200, 201, 'dnf upgrade -y -q glib2', 2),
                                      (5, 300, NULL, 'dnf install kernel-core', 1);
             INSERT INTO trans_item VALUES (1, 3, 12, 1), (2, 4, 10, 2), (3, 4, 11, 6), (4, 5, 13, 1);",
        )
        .unwrap();
        let found = read(&c, i64::MIN).unwrap();
        assert_eq!(
            found.len(),
            2,
            "the unfinished transaction is left for later"
        );
        assert_eq!(found[0].changes, ["installed tree (2.2.1-4.fc44)"]);
        assert_eq!(found[1].ts, 200);
        assert_eq!(found[1].command.as_deref(), Some("dnf upgrade -y -q glib2"));
        assert_eq!(
            found[1].changes,
            ["upgraded glib2 (2.88.3-1.fc44 -> 2.88.4-1.fc44)"]
        );
        assert_eq!(read(&c, 150).unwrap().len(), 1, "only from `since` on");
    }

    #[test]
    fn dnf_is_read_again_only_when_its_database_changes() {
        let path = std::env::temp_dir().join(format!("bb_dnf_{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(DNF5_SCHEMA).unwrap();
        let add = |id: i64, ts: i64, name: &str| {
            // mtimes are coarse: let the clock move before a change, so it shows
            std::thread::sleep(std::time::Duration::from_millis(20));
            c.execute_batch(&format!(
                "INSERT INTO pkg_name VALUES ({id}, '{name}');
                 INSERT INTO rpm VALUES ({id}, {id}, 0, '1.0', '1.fc44');
                 INSERT INTO trans VALUES ({id}, {ts}, {ts}, 'dnf install {name}', 2);
                 INSERT INTO trans_item VALUES ({id}, {id}, {id}, 1);"
            ))
            .unwrap();
        };
        let names = |found: Vec<Transaction>| -> Vec<String> {
            found.into_iter().flat_map(|t| t.changes).collect()
        };
        add(1, 100, "old");
        add(2, 200, "stored");
        // already stored up to 200: the run at 100 is not read again
        let mut dnf = Dnf::new(path.to_string_lossy().into_owned(), 200);
        assert_eq!(
            names(dnf.poll().unwrap()),
            ["installed stored (1.0-1.fc44)"]
        );
        assert!(dnf.poll().unwrap().is_empty(), "unchanged: not read");

        add(3, 300, "new");
        // the newest second already read comes again (the writer drops the repeat), older does not
        assert_eq!(
            names(dnf.poll().unwrap()),
            [
                "installed stored (1.0-1.fc44)",
                "installed new (1.0-1.fc44)"
            ]
        );
        assert!(dnf.poll().unwrap().is_empty(), "unchanged: not read");
        add(4, 400, "newer");
        assert_eq!(
            names(dnf.poll().unwrap()),
            ["installed new (1.0-1.fc44)", "installed newer (1.0-1.fc44)"],
            "it moved on to the newest transaction it read"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dnf4_pairs_upgrades_and_keeps_epochs_and_obsoletes() {
        // rows as a Rocky 9 container recorded `dnf upgrade attr`, plus an obsoleting install
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(DNF4_SCHEMA).unwrap();
        c.execute_batch(
            "INSERT INTO rpm VALUES (1, 'attr', 0, '2.6.0', '1.el9_8'), (2, 'attr', 0, '2.5.1', '3.el9'),
                                    (3, 'libattr', 0, '2.6.0', '1.el9_8'), (4, 'libattr', 0, '2.5.1', '3.el9'),
                                    (5, 'new-ssl', 1, '3.2', '1'), (6, 'old-ssl', 1, '3.0', '7');
             INSERT INTO trans VALUES (3, 100, 101, 'upgrade -y -q attr.x86_64', 1),
                                      (4, 200, 200, 'install new-ssl', 1);
             INSERT INTO trans_item VALUES (1, 3, 1, 6), (2, 3, 2, 7), (3, 3, 3, 6), (4, 3, 4, 7),
                                           (5, 4, 5, 4), (6, 4, 6, 5);",
        )
        .unwrap();
        let found = read(&c, i64::MIN).unwrap();
        assert_eq!(
            found[0].changes,
            [
                "upgraded attr (2.5.1-3.el9 -> 2.6.0-1.el9_8)",
                "upgraded libattr (2.5.1-3.el9 -> 2.6.0-1.el9_8)",
            ]
        );
        assert_eq!(
            found[1].changes,
            ["installed new-ssl (1:3.2-1)", "removed old-ssl (1:3.0-7)"]
        );
        assert_eq!(summary("dnf", &found[0]), "dnf: upgraded 2 · attr, libattr");
    }
}
