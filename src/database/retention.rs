use super::conn;
use super::scheme::TIME_TABLES;
use rusqlite::params;

const TRIM_CHUNK: usize = 1000;

/// Bytes actually holding data (freed pages are reused, so the file itself never shrinks).
pub fn logical_size_bytes() -> u64 {
    let c = conn();
    let pragma = |name: &str| -> i64 {
        c.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap_or(0)
    };
    ((pragma("page_count") - pragma("freelist_count")).max(0) * pragma("page_size")) as u64
}

/// Ring buffer: once over `max_bytes`, drop the oldest slice of time from every table
/// until the data is back under 90% of the budget. Returns the number of rows deleted.
pub fn trim_to_budget(max_bytes: u64) -> usize {
    if logical_size_bytes() <= max_bytes {
        return 0;
    }
    let target = max_bytes / 10 * 9;
    // One aggregate per subquery: SQLite turns a lone MIN/MAX on an indexed column into an index
    // seek, but MIN and MAX together in one SELECT scan the whole index.
    let extreme = |agg: &str| -> String {
        let parts = TIME_TABLES
            .iter()
            .map(|t| format!("SELECT (SELECT {agg}(ts) FROM {t}) AS v"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        format!("SELECT {agg}(v) FROM ({parts})")
    };
    let (min_sql, max_sql) = (extreme("MIN"), extreme("MAX"));
    let mut deleted = 0;
    for _ in 0..40 {
        if logical_size_bytes() <= target {
            break;
        }
        // where the oldest tenth of the recorded time span ends
        let cutoff: Option<String> = {
            let c = conn();
            let lo = c.query_row(&min_sql, [], |r| r.get::<_, Option<String>>(0));
            let hi = c.query_row(&max_sql, [], |r| r.get::<_, Option<String>>(0));
            match (lo, hi) {
                (Ok(Some(lo)), Ok(Some(hi))) => c
                    .query_row(
                        // the newest time is capped at now: one row stamped far in the future
                        // (a clock that was once wrong) would otherwise stretch the span so far
                        // that the first tenth swallows every real row
                        "SELECT datetime(?1, '+' || max(60, (min(strftime('%s', ?2), strftime('%s', 'now')) - strftime('%s', ?1)) / 10) || ' seconds')",
                        params![lo, hi],
                        |r| r.get(0),
                    )
                    .ok(),
                _ => None,
            }
        };
        let Some(cutoff) = cutoff else { break };
        // Small chunks, lock released in between: a big single DELETE would block every writer
        // (the collector would start dropping events) for as long as it runs.
        for table in TIME_TABLES {
            loop {
                let n = {
                    let c = conn();
                    c.execute(
                        &format!(
                            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} WHERE ts < ?1 LIMIT {TRIM_CHUNK})"
                        ),
                        params![cutoff],
                    )
                    .unwrap_or(0)
                };
                deleted += n;
                if n < TRIM_CHUNK {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    checkpoint();
    deleted
}

pub fn checkpoint() {
    let _ = conn().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

pub fn start_trim_thread(max_bytes: u64) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            let deleted = trim_to_budget(max_bytes);
            if deleted > 0 {
                eprintln!(
                    "trim: deleted {deleted} old rows (budget {} MB)",
                    max_bytes / 1_000_000
                );
            } else {
                checkpoint();
            }
        }
    });
}
