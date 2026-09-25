use super::{conn, record, ts_text};
use crate::exit;
use rusqlite::{OptionalExtension, params};

pub fn add_custom_event(
    ts: i64,
    pid: i32,
    ppid: Option<i32>,
    exit_code: i32,
    comm: Option<&str>,
    severity: &str,
    summary: &str,
) {
    let ts = ts_text(ts);
    record("custom", &ts, Some((severity, summary)), |tx| {
        tx.execute(
            "INSERT INTO custom (ts, pid, ppid, exit_code, comm) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, pid, ppid, exit_code, comm],
        )
        .map(|_| ())
    });
}

/// Names a crash after the fact from systemd-coredump's journal entry, which carries the process
/// name and executable even when the process was gone before the collector could look it up.
/// Matches the newest crash of that pid in the ten minutes before the entry.
pub fn name_crash(ts: i64, pid: i32, comm: Option<&str>, exe: Option<&str>) {
    let ts = ts_text(ts);
    let mut guard = conn();
    let result = (|| -> rusqlite::Result<()> {
        let tx = guard.transaction()?;
        let crash: Option<(i64, i64, Option<String>)> = tx
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
        tx.execute(
            "UPDATE custom SET comm = ?1, exe = ?2 WHERE custom_id = ?3",
            params![name, exe, id],
        )?;
        if let Some(name) = name {
            let summary = format!("{name} (pid {pid}) {}", exit::describe(exit_code as u32));
            tx.execute(
                "UPDATE messages SET summary = ?1 WHERE source = 'custom' AND ref_id = ?2",
                params![summary, id],
            )?;
        }
        tx.commit()
    })();
    if let Err(e) = result {
        eprintln!("db write failed (crash name): {e}");
    }
}
