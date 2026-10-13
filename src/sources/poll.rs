//! Sources read on a timer: what they read only changes slowly, or only changes at all while
//! something runs, so a look every few seconds costs less than following it.
//!
//! - `sampler/`: `/proc`, `/sys` (hwmon, RAPL, battery) and `nvidia-smi`, every 10 s.
//! - `packages/`: the pacman, apt, zypper and dnf histories, every 30 s.
//!
//! The loop is here, once; each source says only how often and what one look does (the Strategy
//! pattern).

pub mod packages;
pub mod sampler;

use std::time::Duration;

/// What a polled source supplies to the loop.
pub trait Poller {
    fn every(&self) -> Duration;
    /// One look: read what is new and hand its records to `database::write`.
    fn poll(&mut self);
}

/// Polls forever: a look at once, then one every `every()`.
pub fn poll(p: &mut impl Poller) {
    loop {
        p.poll();
        std::thread::sleep(p.every());
    }
}
