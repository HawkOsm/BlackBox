use super::{conn, record, state::upsert_state, ts_text};
use rusqlite::params;

/// One journal entry to store.
pub struct Journal<'a> {
    pub ts: i64,
    pub priority: i32,
    pub unit: Option<&'a str>,
    pub pid: Option<i32>,
    pub message: &'a str,
    /// Overrides the severity the priority implies (a panic logged at info is still a crash).
    pub severity: Option<&'a str>,
    /// Where the follower is in the journal, saved with the row so a restart resumes exactly.
    pub cursor: Option<(&'a str, &'a str)>,
    /// Skip it if an identical row exists: only needed where a time-based resume overlaps.
    pub dedup: bool,
}

fn journald_duplicate(ts: &str, unit: Option<&str>, message: &str) -> bool {
    conn()
        .query_row(
            "SELECT 1 FROM journald WHERE ts = ?1 AND message = ?2 AND IFNULL(unit, '') = IFNULL(?3, '') LIMIT 1",
            params![ts, message, unit],
            |_| Ok(()),
        )
        .is_ok()
}

pub fn add_journald(j: &Journal) {
    let ts = ts_text(j.ts);
    if j.dedup && journald_duplicate(&ts, j.unit, j.message) {
        return;
    }
    let severity = j
        .severity
        .or_else(|| (j.priority <= 4).then_some(if j.priority <= 3 { "error" } else { "warning" }));
    let summary = format!(
        "{}: {}",
        j.unit.unwrap_or("system"),
        j.message
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>()
    );
    record(
        "journald",
        &ts,
        severity.map(|s| (s, summary.as_str())),
        |tx| {
            // before the row itself: `record` takes the row's id from the last insert
            if let Some((key, cursor)) = j.cursor {
                upsert_state(tx, key, cursor)?;
            }
            tx.execute(
            "INSERT INTO journald (ts, priority, unit, pid, message) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, j.priority, j.unit, j.pid, j.message],
        )
        .map(|_| ())
        },
    );
}

/// Applies `routine::JOURNAL` to journal lines stored before a rule existed, so the history reads
/// the same as new lines. It scans the whole table, so it runs only when the rules have changed.
pub fn demote_routine() {
    const KEY: &str = "journald.routine";
    let rules = &crate::routine::JOURNAL;
    let fingerprint: String = rules
        .iter()
        .map(|r| format!("{}\t{}\n", r.unit.unwrap_or(""), r.text))
        .collect();
    let mut guard = conn();
    let result = (|| -> rusqlite::Result<usize> {
        let tx = guard.transaction()?;
        let done: Option<String> = tx
            .query_row("SELECT value FROM state WHERE key = ?1", [KEY], |r| {
                r.get(0)
            })
            .ok();
        if done.as_deref() == Some(fingerprint.as_str()) {
            return Ok(0);
        }
        let mut demoted = 0;
        for r in rules.iter() {
            demoted += tx.execute(
                "UPDATE messages SET severity = 'info'
                 WHERE source = 'journald' AND severity != 'info' AND ref_id IN (
                     SELECT journald_id FROM journald
                     WHERE instr(message, ?1) > 0 AND (?2 IS NULL OR unit = ?2))",
                params![r.text, r.unit],
            )?;
        }
        upsert_state(&tx, KEY, &fingerprint)?;
        tx.commit()?;
        Ok(demoted)
    })();
    match result {
        Ok(0) => {}
        Ok(n) => eprintln!("journald: {n} stored lines now count as routine"),
        Err(e) => eprintln!("db write failed (journald routine): {e}"),
    }
}
