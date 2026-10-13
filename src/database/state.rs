use super::{Record, read};
use rusqlite::{Connection, params};

/// Sets a `state` value, or with `None` forgets it.
pub struct SetState {
    pub key: String,
    pub value: Option<String>,
}

impl Record for SetState {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        match &self.value {
            Some(v) => upsert_state(c, &self.key, v),
            None => c
                .execute("DELETE FROM state WHERE key = ?1", params![self.key])
                .map(|_| ()),
        }
    }
}

/// For a record that saves where its source is in the same transaction as its row.
pub fn upsert_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
}

pub fn get_state(key: &str) -> Option<String> {
    read(|c| {
        c.query_row(
            "SELECT value FROM state WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .ok()
    })
}
