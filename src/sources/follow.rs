//! Sources that follow a stream: a long-running process whose output lines are events. The
//! process is started again whenever it ends.
//!
//! - `journald/`: `journalctl --follow`, warnings and panics.
//! - `auditd/`: `tail -F` on the audit log.
//!
//! The loop is in `follower.rs`, once; each source says only what to run, what one line means,
//! and what to do when the process ends (the Strategy pattern).

pub mod auditd;
mod follower;
pub mod journald;

pub use follower::{Follower, follow};
