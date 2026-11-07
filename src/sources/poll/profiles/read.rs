//! Each daemon's active profile, over the system bus with `busctl --json`: no D-Bus library, and
//! `powerprofilesctl` is a Python script that costs ~100 ms per call.

use crate::sources::poll::sampler::output_within;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

pub(super) const PPD: &str = "power-profiles-daemon";
pub(super) const TCCD: &str = "tccd";

/// power-profiles-daemon's active profile: `power-saver`, `balanced` or `performance`.
pub(crate) fn read_ppd() -> Option<String> {
    let reply = busctl(&[
        "get-property",
        "net.hadess.PowerProfiles",
        "/net/hadess/PowerProfiles",
        "net.hadess.PowerProfiles",
        "ActiveProfile",
    ])?;
    reply_string(&reply)
}

/// The name of tccd's active profile (TUXEDO Control Center), as its user named it.
pub(crate) fn read_tccd() -> Option<String> {
    let reply = busctl(&[
        "call",
        "com.tuxedocomputers.tccd",
        "/com/tuxedocomputers/tccd",
        "com.tuxedocomputers.tccd",
        "GetActiveProfileJSON",
    ])?;
    tccd_name(&reply_string(&reply)?)
}

fn busctl(args: &[&str]) -> Option<String> {
    let out = output_within(
        Command::new("busctl")
            .args(["--system", "--json=short"])
            .args(args),
        Duration::from_secs(2),
    )?;
    if !out.status.success() {
        return None; // the daemon is not running
    }
    String::from_utf8(out.stdout).ok()
}

/// The one string in a busctl JSON reply: `{"type":"s","data":"x"}` from get-property,
/// `{"type":"s","data":["x"]}` from a call.
fn reply_string(reply: &str) -> Option<String> {
    let data = serde_json::from_str::<Value>(reply)
        .ok()?
        .get("data")?
        .clone();
    let s = match data {
        Value::Array(mut items) if items.len() == 1 => items.pop()?,
        other => other,
    };
    s.as_str().map(String::from)
}

fn tccd_name(json: &str) -> Option<String> {
    let profile = serde_json::from_str::<Value>(json).ok()?;
    profile.get("name")?.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::{reply_string, tccd_name};

    #[test]
    fn a_property_and_a_call_reply_both_give_their_string() {
        let property = r#"{"type":"s","data":"balanced"}"#;
        assert_eq!(reply_string(property).as_deref(), Some("balanced"));
        let call = r#"{"type":"s","data":["{\"name\":\"Balanced\",\"fan\":{}}"]}"#;
        let json = reply_string(call).unwrap();
        assert_eq!(tccd_name(&json).as_deref(), Some("Balanced"));
        assert_eq!(reply_string(""), None);
        assert_eq!(tccd_name("{}"), None);
    }
}
