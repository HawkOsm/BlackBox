use super::exit;
use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, OptionalExtension, params};

/// A process that crashed or failed, as the collector saw it exit.
pub struct Exit {
    pub ts: i64,
    pub pid: i32,
    pub ppid: Option<i32>,
    /// The raw wait status: see `exit.rs`.
    pub exit_code: i32,
    pub comm: Option<String>,
    pub severity: Severity,
    pub summary: String,
}

/// What systemd-coredump's journal entry says about a crash.
pub struct CrashName {
    pub ts: i64,
    pub pid: i32,
    pub comm: Option<String>,
    pub exe: Option<String>,
}

impl Record for Exit {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO custom (ts, pid, ppid, exit_code, comm) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, self.pid, self.ppid, self.exit_code, self.comm],
        )?;
        index(c, "custom", &ts, Some((self.severity, &self.summary)))
    }
}

/// Names a crash after the fact from systemd-coredump's journal entry, which carries the process
/// name and executable even when the process was gone before the collector could look it up.
/// Matches the newest crash of that pid in the ten minutes before the entry.
impl Record for CrashName {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        let (pid, comm, exe) = (self.pid, self.comm.as_deref(), self.exe.as_deref());
        let crash: Option<(i64, i64, Option<String>)> = c
            .query_row(
                "SELECT custom_id, exit_code, comm FROM custom
                 WHERE pid = ?1 AND exe IS NULL AND (exit_code & 127) != 0
                   AND ts BETWEEN datetime(?2, '-600 seconds') AND datetime(?2, '+60 seconds')
                 ORDER BY custom_id DESC LIMIT 1",
                params![pid, ts],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, exit_code, known)) = crash else {
            return Ok(());
        };
        let name = known.as_deref().or(comm);
        c.execute(
            "UPDATE custom SET comm = ?1, exe = ?2 WHERE custom_id = ?3",
            params![name, exe, id],
        )?;
        if let Some(name) = name {
            let summary = format!("{name} (pid {pid}) {}", exit::describe(exit_code as u32));
            c.execute(
                "UPDATE messages SET summary = ?1 WHERE source = 'custom' AND ref_id = ?2",
                params![summary, id],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    fn crash(pid: i32, comm: Option<&str>, exit_code: i32, summary: &str) -> Exit {
        Exit {
            ts: 1_700_100_000,
            pid,
            ppid: Some(1),
            exit_code,
            comm: comm.map(str::to_string),
            severity: Severity::Error,
            summary: summary.to_string(),
        }
    }

    #[test]
    fn a_crash_is_stored_with_its_alert() {
        let c = open(":memory:");
        crash(7, Some("bash"), 11, "bash (pid 7) killed by SIGSEGV (11)")
            .store(&c)
            .unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM custom"), 1);
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM messages m JOIN custom ON m.ref_id = custom_id
                 WHERE m.source = 'custom' AND m.severity = 'error'"
            ),
            1
        );
    }

    #[test]
    fn a_crash_is_named_from_its_coredump() {
        let c = open(":memory:");
        crash(4242, None, 139, "? (pid 4242) crashed")
            .store(&c)
            .unwrap();
        CrashName {
            ts: 1_700_100_003,
            pid: 4242,
            comm: Some("bash".to_string()),
            exe: Some("/usr/bin/bash".to_string()),
        }
        .store(&c)
        .unwrap();
        let (comm, exe, summary): (String, String, String) = c
            .query_row(
                "SELECT comm, exe, summary FROM custom JOIN messages
                 ON source = 'custom' AND ref_id = custom_id WHERE pid = 4242",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((comm.as_str(), exe.as_str()), ("bash", "/usr/bin/bash"));
        assert_eq!(
            summary,
            "bash (pid 4242) killed by SIGSEGV (11), core dumped"
        );
    }
}
