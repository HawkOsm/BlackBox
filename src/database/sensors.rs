use super::{conn, ts_text};
use crate::sensors::Reading;
use rusqlite::params;

/// All of one tick's sensor readings in a single transaction.
pub fn add_sensor_readings(ts: i64, readings: &[Reading]) {
    if readings.is_empty() {
        return;
    }
    let ts = ts_text(ts);
    let mut guard = conn();
    let result = (|| -> rusqlite::Result<()> {
        let tx = guard.transaction()?;
        for r in readings {
            tx.execute(
                "INSERT INTO sensors (ts, kind, name, value) VALUES (?1, ?2, ?3, ?4)",
                params![ts, r.kind, r.name, r.value],
            )?;
        }
        tx.commit()
    })();
    if let Err(e) = result {
        eprintln!("db write failed (sensors): {e}");
    }
}
