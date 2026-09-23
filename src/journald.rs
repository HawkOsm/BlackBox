use crate::database;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

pub struct Entry {
    pub ts: i64,
    pub priority: i32,
    pub unit: Option<String>,
    pub pid: Option<i32>,
    pub message: String,
    pub coredump: Option<Coredump>,
}

/// systemd-coredump's record of a crash: who crashed, even when the process was already gone.
pub struct Coredump {
    pub pid: i32,
    pub comm: Option<String>,
    pub exe: Option<String>,
}

fn text(v: &Value) -> Option<String> {
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

pub fn parse_line(line: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(line).ok()?;
    let micros: i64 = v.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
    let priority: i32 = v.get("PRIORITY").and_then(text)?.parse().ok()?;
    let unit = ["_SYSTEMD_UNIT", "_SYSTEMD_USER_UNIT", "SYSLOG_IDENTIFIER"]
        .iter()
        .find_map(|k| v.get(*k).and_then(text));
    let pid = v.get("_PID").and_then(text).and_then(|p| p.parse().ok());
    let message = v.get("MESSAGE").and_then(text)?;
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
        ts: micros / 1_000_000,
        priority,
        unit,
        pid,
        message,
        coredump,
    })
}

pub fn start() {
    std::thread::spawn(|| {
        loop {
            // resume from the last stored entry so a restart doesn't lose the events around a crash
            let since = match database::last_ts("journald") {
                Some(ts) => format!("--since={ts} UTC"),
                None => "--lines=0".to_string(),
            };
            let child = Command::new("journalctl")
                .args([
                    "--follow",
                    "--output=json",
                    "--priority=warning",
                    "--no-pager",
                    &since,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut child) => {
                    if let Some(out) = child.stdout.take() {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if let Some(e) = parse_line(&line) {
                                database::add_journald_event(
                                    e.ts,
                                    e.priority,
                                    e.unit.as_deref(),
                                    e.pid,
                                    &e.message,
                                );
                                if let Some(c) = &e.coredump {
                                    database::name_crash(
                                        e.ts,
                                        c.pid,
                                        c.comm.as_deref(),
                                        c.exe.as_deref(),
                                    );
                                }
                            }
                        }
                    }
                    let _ = child.wait();
                }
                Err(e) => eprintln!("journalctl unavailable: {e}"),
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
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
    fn rejects_garbage() {
        assert!(parse_line("not json").is_none());
    }
}
