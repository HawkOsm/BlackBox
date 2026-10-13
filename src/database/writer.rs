//! The writer: one thread, the only code that commits events.
//!
//! ```text
//!  source ── database::write(record) ──► queue ──► writer thread ──► one transaction per burst
//! ```
//!
//! A source hands over a `Record`, something that knows how to store itself; the writer calls its
//! `store` inside an open transaction. The writer never knows which source a record comes from,
//! so a new source adds a `Record` in its own folder and nothing here changes.
//!
//! A lone record commits at once, so `synchronous=FULL` still puts every event on disk before the
//! next can be lost; a burst (a package history import, a crash cascade) costs one fsync instead of
//! one per row.

use super::conn;
use rusqlite::Connection;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Something a source wants stored. Everything one `store` writes commits together.
pub trait Record: Send {
    /// Writes it with the writer's connection, inside its open transaction.
    fn store(&self, c: &Connection) -> rusqlite::Result<()>;
}

/// Records waiting for the writer. A burst larger than this makes the sources wait while the
/// writer commits; it loses nothing.
const QUEUE: usize = 256;

static WRITER: OnceLock<SyncSender<Box<dyn Record>>> = OnceLock::new();
/// Records queued, and records committed, ever: `drain` waits for the second to reach what the
/// first was when it was called. The writer signals `COMMITS` after each commit.
static QUEUED: AtomicU64 = AtomicU64::new(0);
static COMMITTED: Mutex<u64> = Mutex::new(0);
static COMMITS: Condvar = Condvar::new();

/// Starts the writer thread. Runs once, after `init_database`, before any source starts.
pub fn start_writer() {
    let (tx, rx) = sync_channel(QUEUE);
    std::thread::spawn(move || {
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(rx, |records| {
                let n = records.len() as u64;
                commit(&mut conn(), records);
                *COMMITTED.lock().unwrap_or_else(|p| p.into_inner()) += n;
                COMMITS.notify_all();
            })
        }));
        // A dead writer would leave every source queueing into nothing: stop, and let systemd
        // restart the service.
        eprintln!("database writer stopped ({stopped:?}); exiting");
        std::process::exit(1);
    });
    let _ = WRITER.set(tx);
}

/// Queues a record. Returns at once unless the queue is full.
pub fn write(record: impl Record + 'static) {
    let writer = WRITER.get().expect("start_writer() must run first");
    QUEUED.fetch_add(1, Ordering::SeqCst);
    if writer.send(Box::new(record)).is_err() {
        // the writer thread exits the process itself; this only runs in the moment before
        std::thread::park();
    }
}

/// Waits, at most 5 s, until every record queued before this call is committed: for the process to
/// call before it ends on its own, so the last events are not lost with it. (A kill still loses
/// what is queued, the writes of one commit.)
pub fn drain() {
    let target = QUEUED.load(Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut committed = COMMITTED.lock().unwrap_or_else(|p| p.into_inner());
    while *committed < target {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        committed = COMMITS
            .wait_timeout(committed, left)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}

/// Hands every batch to `commit` until all senders are gone. The service commits to the database;
/// a test can commit anywhere.
fn run(rx: Receiver<Box<dyn Record>>, mut commit: impl FnMut(Vec<Box<dyn Record>>)) {
    // block for one, then take whatever else is already waiting: no added delay, one commit
    while let Ok(first) = rx.recv() {
        commit(std::iter::once(first).chain(rx.try_iter()).collect());
    }
}

/// Stores records in one transaction. Each runs in its own savepoint, so one that fails is logged
/// and rolled back alone instead of taking the rest of the batch with it.
pub(crate) fn commit(c: &mut Connection, records: Vec<Box<dyn Record>>) {
    if records.is_empty() {
        return;
    }
    let result = (|| -> rusqlite::Result<()> {
        let mut tx = c.transaction()?;
        for record in records {
            let sp = tx.savepoint()?;
            match record.store(&sp) {
                Ok(()) => sp.commit()?,
                // the error names the table; dropping the savepoint rolls this record back
                Err(e) => eprintln!("db write failed: {e}"),
            }
        }
        tx.commit()
    })();
    if let Err(e) = result {
        eprintln!("db write failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A record of no source in particular: one row in `state`.
    struct Note(String);

    impl Record for Note {
        fn store(&self, c: &Connection) -> rusqlite::Result<()> {
            c.execute(
                "INSERT INTO state (key, value) VALUES (?1, 'x')",
                [self.0.as_str()],
            )
            .map(|_| ())
        }
    }

    fn note(key: impl Into<String>) -> Box<dyn Record> {
        Box::new(Note(key.into()))
    }

    fn count(c: &Connection) -> i64 {
        c.query_row("SELECT COUNT(*) FROM state", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn records_from_many_sources_reach_the_database() {
        // a database of its own, opened the way the service opens one
        let db = Arc::new(Mutex::new(crate::database::open(":memory:")));
        let (tx, rx) = sync_channel(QUEUE);
        let writer_db = db.clone();
        let writer = std::thread::spawn(move || {
            run(rx, |records| {
                commit(&mut writer_db.lock().unwrap(), records)
            })
        });
        // four sources at once, 100 records each
        let sources: Vec<_> = (0..4)
            .map(|source| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for n in 0..100 {
                        tx.send(note(format!("{source}.{n}"))).unwrap();
                    }
                })
            })
            .collect();
        for s in sources {
            s.join().unwrap();
        }
        // with every sender gone, the writer commits what is left and stops
        drop(tx);
        writer.join().unwrap();
        assert_eq!(count(&db.lock().unwrap()), 400);
    }

    #[test]
    fn a_failed_record_is_rolled_back_alone() {
        let mut c = crate::database::open(":memory:");
        // the second one breaks the primary key of `state`
        commit(&mut c, vec![note("a"), note("a"), note("b")]);
        assert_eq!(count(&c), 2);
    }

    #[test]
    fn a_burst_is_one_commit() {
        let (tx, rx) = sync_channel(QUEUE);
        // the burst is queued before the writer looks, as when a package history is imported
        for n in 0..100 {
            tx.send(note(n.to_string())).unwrap();
        }
        drop(tx);
        let mut batches = Vec::new();
        run(rx, |records| batches.push(records.len()));
        assert_eq!(batches, [100]);
    }

    #[test]
    fn a_lone_record_is_not_held_back() {
        let (tx, rx) = sync_channel(QUEUE);
        let (committed, seen) = sync_channel(QUEUE);
        std::thread::spawn(move || run(rx, |records| committed.send(records.len()).unwrap()));
        tx.send(note("lone")).unwrap();
        // it is committed on its own right away, not after waiting for company
        assert_eq!(seen.recv_timeout(std::time::Duration::from_secs(1)), Ok(1));
    }
}
