//! The audit log, followed with `tail -F`.
//!
//! - `parse.rs`: what one line is (`classify`, `Found`) and how much it matters.
//! - `resume.rs`: where in the log to start again after a restart.
//! - `follower.rs`: the `Follower` that runs `tail` and stores each record.
//! - `store.rs`: `Audit` and its SQL.

mod follower;
mod parse;
mod resume;
mod store;

use crate::database;
use crate::sources::follow::follow;
use crate::sources::supervise;
use follower::Auditd;

pub fn log_path() -> String {
    std::env::var("BLACKBOX_AUDIT_LOG").unwrap_or_else(|_| "/var/log/audit/audit.log".to_string())
}

pub fn start() {
    let mut audit = Auditd {
        booted: crate::sources::once::boot::boot_time(),
        last: database::last_ts("auditd")
            .as_deref()
            .and_then(database::ts_epoch),
    };
    supervise("auditd", move || follow(&mut audit));
}
