//! The system journal, followed with `journalctl --follow`: one follower for warnings and worse,
//! one for Rust panics (logged at info, below the warning filter).
//!
//! - `parse.rs`: one JSON line into an `Entry`.
//! - `follower.rs`: the two feeds, and the `Follower` that runs `journalctl` and stores each entry.
//! - `store.rs`: `Journal` and its SQL, and re-rating stored lines when `routine.rs` changes.

mod follower;
mod parse;
mod store;

use crate::database;
use crate::sources::follow::follow;
use crate::sources::supervise;
use follower::{FEEDS, Journald};
pub use parse::{entry_ts, text};
use store::DemoteRoutine;

pub fn start() {
    database::write(DemoteRoutine);
    for feed in FEEDS {
        let mut journal = Journald::new(feed);
        supervise(&format!("journald.{}", feed.name), move || {
            follow(&mut journal)
        });
    }
}
