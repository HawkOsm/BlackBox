//! Once per boot: record that the system started, and whether the previous boot ended with a
//! clean shutdown. A freeze, a kernel panic or a power loss kills the collector along with
//! everything else, so this is the only way that kind of failure gets recorded at all.

use crate::database::{self, Boot};
use crate::journald::text;
use serde_json::Value;
use std::process::Command;

/// journald's "Journal stopped", the last entry of every clean shutdown.
const JOURNAL_STOPPED: &str = "d93fb3c9c24d451a97cea615ce59c00b";

/// The last journal entry of a boot.
pub struct End {
    pub ts: i64,
    pub boot: String,
    pub clean: bool,
    pub last_message: String,
}

pub fn parse_last_entry(json: &str) -> Option<End> {
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    let micros: i64 = v.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
    Some(End {
        ts: micros / 1_000_000,
        boot: v.get("_BOOT_ID")?.as_str()?.to_string(),
        clean: v.get("MESSAGE_ID").and_then(Value::as_str) == Some(JOURNAL_STOPPED),
        last_message: v.get("MESSAGE").and_then(text).unwrap_or_default(),
    })
}

/// What the end of an unclean boot most likely was, from the last thing it logged.
pub fn describe(end: &End) -> &'static str {
    let m = &end.last_message;
    if m.contains("Filesystems sync") || m.contains("PM: suspend") {
        "The system stopped while suspending and never resumed (power loss or a failed resume)"
    } else {
        "The system stopped here without a clean shutdown (a freeze, a kernel panic or power loss)"
    }
}

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

fn previous_end() -> Option<End> {
    let out = Command::new("journalctl")
        .args(["--boot=-1", "--lines=1", "--output=json", "--no-pager"])
        .output()
        .ok()?;
    // fails when the journal is not persistent or this is the first boot it knows
    out.status
        .success()
        .then(|| parse_last_entry(&String::from_utf8_lossy(&out.stdout)))?
}

pub fn start() {
    std::thread::spawn(|| {
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
            database::mark_shutdown(end.ts - SHUTDOWN_WINDOW, end.ts);
        }
        if database::boot_recorded(&this_boot, "start") {
            return; // the service restarted within this boot
        }
        if let Some(end) = &previous
            && !end.clean
            && !database::boot_recorded(&end.boot, "unclean_end")
        {
            database::add_boot_event(
                &Boot {
                    ts: end.ts,
                    kind: "unclean_end",
                    kernel_boot: &end.boot,
                    kernel: None,
                    note: Some(&end.last_message),
                },
                "error",
                describe(end),
            );
        }
        let kernel = read("/proc/sys/kernel/osrelease");
        let started = boot_time().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
        database::add_boot_event(
            &Boot {
                ts: started,
                kind: "start",
                kernel_boot: &this_boot,
                kernel: kernel.as_deref(),
                note: None,
            },
            "info",
            &format!("System started, kernel {}", kernel.as_deref().unwrap_or("unknown")),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_shutdown_ends_with_journal_stopped() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1790000000000000","_BOOT_ID":"3df9a5a0","MESSAGE_ID":"d93fb3c9c24d451a97cea615ce59c00b","MESSAGE":"Journal stopped"}"#;
        let end = parse_last_entry(line).unwrap();
        assert!(end.clean);
        assert_eq!(end.ts, 1_790_000_000);
        assert_eq!(end.boot, "3df9a5a0");
    }

    #[test]
    fn anything_else_is_an_unclean_end() {
        // the last entries of real unclean boots on the development machine
        let freeze = r#"{"__REALTIME_TIMESTAMP":"1","_BOOT_ID":"a","MESSAGE":"Endpoint unregistered: sender=:1.257 path=/MediaEndpoint"}"#;
        let end = parse_last_entry(freeze).unwrap();
        assert!(!end.clean);
        assert!(describe(&end).contains("without a clean shutdown"));

        let suspend = r#"{"__REALTIME_TIMESTAMP":"1","_BOOT_ID":"b","MESSAGE":"Filesystems sync: 0.099 seconds"}"#;
        let end = parse_last_entry(suspend).unwrap();
        assert!(!end.clean);
        assert!(describe(&end).contains("never resumed"));
    }

    #[test]
    fn no_previous_boot_is_not_an_error() {
        assert!(parse_last_entry("").is_none());
    }
}
