//! The active power profiles, stored only when one changes: power-profiles-daemon's (`balanced`,
//! `performance`, …) and, on TUXEDO laptops, tccd's. Asked over D-Bus every 10 s; a machine
//! without one of the daemons just has no rows for it.
//!
//! - `read.rs`: asking each daemon, with `busctl`.
//! - `poller.rs`: noticing a change.
//! - `store.rs`: `ProfileChange` and its SQL.

mod poller;
mod read;
mod store;

pub(crate) use read::{read_ppd, read_tccd};

use super::poll;
use crate::sources::supervise;
use poller::ProfilePoller;

pub fn start() {
    let mut profiles = ProfilePoller::resume();
    supervise("profiles", move || poll(&mut profiles));
}
