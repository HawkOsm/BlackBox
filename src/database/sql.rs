use super::Severity;
use super::connection::read;
use super::scheme::TIME_TABLES;
use rusqlite::{Connection, OptionalExtension, Params, params};

/// Whether the query returns a row. A real error (a busy or broken database) is an error, not
/// "no": read as "no", it would make a duplicate check store the row a second time.
pub fn exists(c: &Connection, sql: &str, params: impl Params) -> rusqlite::Result<bool> {
    c.query_row(sql, params, |_| Ok(()))
        .optional()
        .map(|row| row.is_some())
}

/// Inserts the `messages` index row for the source row just inserted, when there is an alert.
pub fn index(
    c: &Connection,
    source: &str,
    ts: &str,
    alert: Option<(Severity, &str)>,
) -> rusqlite::Result<()> {
    let Some((severity, summary)) = alert else {
        return Ok(());
    };
    c.execute(
        "INSERT OR REPLACE INTO messages (source, ref_id, ts, severity, summary)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![source, c.last_insert_rowid(), ts, severity, summary],
    )
    .map(|_| ())
}

/// Latest timestamp already stored for a source table (used to backfill after a restart).
pub fn last_ts(table: &str) -> Option<String> {
    if !TIME_TABLES.contains(&table) {
        return None;
    }
    read(|c| {
        c.query_row(&format!("SELECT MAX(ts) FROM {table}"), [], |r| r.get(0))
            .ok()
            .flatten()
    })
}

/// `SELECT COUNT(*) …` as a number, for tests.
#[cfg(test)]
pub(crate) fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}
