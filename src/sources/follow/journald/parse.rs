//! One line of `journalctl --output=json`, read into an `Entry`.

use serde_json::Value;

pub struct Entry {
    pub ts: i64,
    pub priority: i32,
    pub unit: Option<String>,
    pub pid: Option<i32>,
    pub message: String,
    pub cursor: Option<String>,
    pub coredump: Option<Coredump>,
}

/// systemd-coredump's record of a crash: who crashed, even when the process was already gone.
pub struct Coredump {
    pub pid: i32,
    pub comm: Option<String>,
    pub exe: Option<String>,
}

pub fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        // journald encodes non-UTF-8 messages as an array of bytes
        Value::Array(bytes) => {
            let raw: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64().map(|n| n as u8))
                .collect();
            Some(String::from_utf8_lossy(&raw).into_owned())
        }
        _ => None,
    }
}

/// When a journal entry was written, in epoch seconds (journald keeps microseconds).
pub fn entry_ts(v: &Value) -> Option<i64> {
    let micros: i64 = v.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
    Some(micros / 1_000_000)
}

pub fn parse_line(line: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = entry_ts(&v)?;
    let priority: i32 = v.get("PRIORITY").and_then(text)?.parse().ok()?;
    let unit = ["_SYSTEMD_UNIT", "_SYSTEMD_USER_UNIT", "SYSLOG_IDENTIFIER"]
        .iter()
        .find_map(|k| v.get(*k).and_then(text));
    let pid = v.get("_PID").and_then(text).and_then(|p| p.parse().ok());
    let message = strip_ansi(&v.get("MESSAGE").and_then(text)?);
    if message.trim().is_empty() {
        return None; // the kernel sometimes logs a bare continuation line, at error priority
    }
    let coredump = v
        .get("COREDUMP_PID")
        .and_then(text)
        .and_then(|p| p.parse().ok())
        .map(|pid| Coredump {
            pid,
            comm: v.get("COREDUMP_COMM").and_then(text),
            exe: v.get("COREDUMP_EXE").and_then(text),
        });
    Some(Entry {
        ts,
        priority,
        unit,
        pid,
        message,
        cursor: v.get("__CURSOR").and_then(text),
        coredump,
    })
}

/// Drops terminal colour codes (`ESC [ ... m` and friends), which some services log verbatim.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // parameter and intermediate bytes, then one final byte from '@' to '~'
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_journal_line() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1790188688795424","PRIORITY":"3","_SYSTEMD_UNIT":"foo.service","_PID":"42","MESSAGE":"disk on fire"}"#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.ts, 1790188688);
        assert_eq!(e.priority, 3);
        assert_eq!(e.unit.as_deref(), Some("foo.service"));
        assert_eq!(e.pid, Some(42));
        assert_eq!(e.message, "disk on fire");
    }

    #[test]
    fn handles_binary_message_and_missing_unit() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1000000","PRIORITY":"4","SYSLOG_IDENTIFIER":"kernel","MESSAGE":[104,105]}"#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.message, "hi");
        assert_eq!(e.unit.as_deref(), Some("kernel"));
        assert_eq!(e.pid, None);
    }

    #[test]
    fn reads_the_coredump_fields() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1790188688795424","PRIORITY":"2","SYSLOG_IDENTIFIER":"systemd-coredump","MESSAGE":"Process 4242 (bash) of user 1000 dumped core.","COREDUMP_PID":"4242","COREDUMP_COMM":"bash","COREDUMP_EXE":"/usr/bin/bash"}"#;
        let c = parse_line(line).unwrap().coredump.unwrap();
        assert_eq!(c.pid, 4242);
        assert_eq!(c.comm.as_deref(), Some("bash"));
        assert_eq!(c.exe.as_deref(), Some("/usr/bin/bash"));
        assert!(
            parse_line(r#"{"__REALTIME_TIMESTAMP":"1","PRIORITY":"3","MESSAGE":"x"}"#)
                .unwrap()
                .coredump
                .is_none()
        );
    }

    #[test]
    fn strips_colour_codes_and_keeps_the_cursor() {
        // how warp-svc logs a panic: bytes, with colour codes
        let msg: Vec<String> =
            "\x1b[2m2026\x1b[0m \x1b[31mERROR\x1b[0m thread 'main' panicked at src/x.rs:1:2"
                .bytes()
                .map(|b| b.to_string())
                .collect();
        let line = format!(
            r#"{{"__REALTIME_TIMESTAMP":"1000000","__CURSOR":"s=1;i=2","PRIORITY":"6","MESSAGE":[{}]}}"#,
            msg.join(",")
        );
        let e = parse_line(&line).unwrap();
        assert_eq!(
            e.message,
            "2026 ERROR thread 'main' panicked at src/x.rs:1:2"
        );
        assert_eq!(e.cursor.as_deref(), Some("s=1;i=2"));
    }

    #[test]
    fn skips_empty_messages() {
        assert!(parse_line(r#"{"__REALTIME_TIMESTAMP":"1","PRIORITY":"3","SYSLOG_IDENTIFIER":"kernel","MESSAGE":""}"#).is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_line("not json").is_none());
    }
}
