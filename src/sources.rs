//! Everything the recorder watches, grouped by how the data arrives. Each source reads one part of
//! the machine and hands what it finds to `database::write()` as a `Record` defined in its own
//! `store.rs`, next to that record's SQL. Sources never commit anything themselves.
//!
//! | group     | source      | reads                                              | stores into                               |
//! |-----------|-------------|----------------------------------------------------|-------------------------------------------|
//! | `kernel/` | `collector` | process events over netlink                        | `custom`                                  |
//! | `follow/` | `journald`  | `journalctl --follow`, warnings and panics         | `journald`                                |
//! | `follow/` | `auditd`    | `tail -F /var/log/audit/audit.log`                 | `auditd`                                  |
//! | `poll/`   | `sampler`   | `/proc`, `/sys`, `nvidia-smi`, every 10 s          | `sysstat`, `power`, `sensors`, `procstat` |
//! | `poll/`   | `packages`  | pacman, apt, zypper and dnf history, every 30 s    | `packages`                                |
//! | `once/`   | `boot`      | the previous boot's last journal entry             | `boot`                                    |
//!
//! `follow/` and `poll/` each hold their group's loop once; `supervise.rs` runs every source but
//! the collector on a thread of its own. `routine.rs` is shared by journald and auditd.

pub mod follow;
pub mod kernel;
pub mod once;
pub mod poll;
pub mod routine;
mod supervise;

pub use supervise::supervise;

/// Starts every source and runs the collector on this thread; returns only if it stops, once what
/// is still queued for the database is stored.
pub fn run() {
    // first: if the kernel refuses the subscription the process exits, and no source has queued
    // anything yet that would be lost with it
    let socket = kernel::collector::subscribe();
    follow::journald::start();
    follow::auditd::start();
    poll::sampler::start();
    poll::packages::start();
    once::boot::start();
    kernel::collector::listen(socket);
    crate::database::drain();
}
