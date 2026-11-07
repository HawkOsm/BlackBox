//! Temperature spikes: the CPU package temperature once a second, and, when it jumps, what was
//! running at that moment. Nothing is stored while it stays calm; the 10 s sampler keeps the
//! temperature itself.
//!
//! - `trigger.rs`: when a reading is a spike (+15 C in 3 s, or 80 C), and the cooldown after one.
//! - `procs.rs`: each process's live CPU use and context switches, from `/proc` over one second.
//! - `context.rs`: the rest of the moment: AC, profiles, Spotify, the GPU's processes.
//! - `poller.rs`: the watch, and capturing a spike.
//! - `store.rs`: `Spike` and its SQL.

mod context;
mod poller;
mod procs;
mod store;
mod trigger;

use super::poll;
use crate::sources::supervise;
use poller::SpikeWatch;
use std::path::PathBuf;

/// The CPU package temperature file the watch reads, for `--check`.
pub(crate) fn package_sensor() -> Option<PathBuf> {
    poller::find_package(std::path::Path::new(poller::HWMON))
}

pub fn start() {
    let mut watch = SpikeWatch::new();
    supervise("spike", move || poll(&mut watch));
}
