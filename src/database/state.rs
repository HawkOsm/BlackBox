use super::conn;
use rusqlite::{Connection, params};

pub(super) fn upsert_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
}

pub fn get_state(key: &str) -> Option<String> {
    conn()
        .query_row(
            "SELECT value FROM state WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .ok()
}

pub fn set_state(key: &str, value: Option<&str>) {
    let c = conn();
    let result = match value {
        Some(v) => upsert_state(&c, key, v),
        None => c
            .execute("DELETE FROM state WHERE key = ?1", params![key])
            .map(|_| ()),
    };
    if let Err(e) = result {
        eprintln!("db write failed (state): {e}");
    }
}
