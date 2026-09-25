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
