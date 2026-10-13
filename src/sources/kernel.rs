//! Sources the kernel pushes events to.
//!
//! - `collector/`: process exits over the netlink process connector. It runs on the main thread:
//!   it blocks in the kernel until an event arrives, and when its socket fails the process exits so
//!   systemd starts it again.

pub mod collector;
