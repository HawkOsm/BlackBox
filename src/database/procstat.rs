use super::{conn, ts_text};
use rusqlite::params;

pub struct ProcSample {
    pub pid: i32, // same type add_custom_event already uses for pids
    pub comm: String,
    pub rss_kb: i64,
    pub pss_kb: Option<i64>, // Option becomes NULL in SQL when the read failed
}

pub fn add_procstat(ts: i64, procs: &[ProcSample]) {
    let ts = ts_text(ts); // A: convert the timestamp once
    let mut guard = conn(); // B: lock the shared connection
    let result = (|| -> rusqlite::Result<()> {
        // C: a closure, so `?` works inside
        let tx = guard.transaction()?;
        for s in procs {
            tx.execute(
                "INSERT INTO procstat (ts, pid, comm, rss_kb, pss_kb) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ts, s.pid, s.comm, s.rss_kb, s.pss_kb],
            )?;
        }
        tx.commit() // F: all writes land together
    })(); //    the () at the end runs the closure
    if let Err(e) = result {
        // G: on failure, log and carry on
        eprintln!("db write failed (procstat): {e}");
    }
}
