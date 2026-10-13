//! How the previous boot ended, from its last journal entry.

use crate::sources::follow::journald::{entry_ts, text};
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
    Some(End {
        ts: entry_ts(&v)?,
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

pub(super) fn previous_end() -> Option<End> {
    let out = Command::new("journalctl")
        .args(["--boot=-1", "--lines=1", "--output=json", "--no-pager"])
        .output()
        .ok()?;
    // fails when the journal is not persistent or this is the first boot it knows
    out.status
        .success()
        .then(|| parse_last_entry(&String::from_utf8_lossy(&out.stdout)))?
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
