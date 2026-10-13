//! Once per boot: record that the system started, and whether the previous boot ended with a
//! clean shutdown. A freeze, a kernel panic or a power loss kills the collector along with
//! everything else, so this is the only way that kind of failure gets recorded at all.
//!
//! - `end.rs`: how the previous boot ended, from its last journal entry.
//! - `store.rs`: `Boot`, `MarkShutdown` and their SQL.

mod end;
mod store;

use crate::database::{self, Severity};
use crate::sources::supervise;
use end::{describe, previous_end};
use store::{Boot, MarkShutdown, boot_recorded};

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// How long before a clean shutdown's last entry errors count as shutdown noise.
const SHUTDOWN_WINDOW: i64 = 30;

/// When this boot started, from `btime` in /proc/stat.
pub fn boot_time() -> Option<i64> {
    read("/proc/stat")?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

pub fn start() {
    supervise("boot", || {
        // journald writes boot ids without the dashes
        let Some(this_boot) = read("/proc/sys/kernel/random/boot_id").map(|b| b.replace('-', ""))
        else {
            return;
        };
        let previous = previous_end();
        if let Some(end) = &previous
            && end.clean
        {
            // cheap and idempotent, so it runs at every start
            database::write(MarkShutdown {
                from: end.ts - SHUTDOWN_WINDOW,
                to: end.ts,
            });
        }
        if boot_recorded(&this_boot, "start") {
            return; // the service restarted within this boot
        }
        if let Some(end) = &previous
            && !end.clean
            && !boot_recorded(&end.boot, "unclean_end")
        {
            database::write(Boot {
                ts: end.ts,
                kind: "unclean_end",
                kernel_boot: end.boot.clone(),
                kernel: None,
                note: Some(end.last_message.clone()),
                severity: Severity::Error,
                summary: describe(end).to_string(),
            });
        }
        let kernel = read("/proc/sys/kernel/osrelease");
        let started = boot_time().unwrap_or_else(database::now);
        let summary = format!(
            "System started, kernel {}",
            kernel.as_deref().unwrap_or("unknown")
        );
        database::write(Boot {
            ts: started,
            kind: "start",
            kernel_boot: this_boot,
            kernel,
            note: None,
            severity: Severity::Info,
            summary,
        });
    });
}
