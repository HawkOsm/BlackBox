//! Keeps only the low-noise, high-value records: failed logins and auth, sudo usage, anomalies
//! (crashes, promiscuous mode), MAC denials, and hits on our own `bb_*` rules.

use super::store::Audit;
use crate::database::Severity;
use crate::sources::routine;

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!(" {name}=");
    let start = line.find(&pattern)? + pattern.len();
    let rest = &line[start..];
    match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next(),
        None => rest.split(|c: char| c.is_whitespace() || c == '\'').next(),
    }
}

/// What an audit record is, before deciding how much it matters.
#[derive(Debug, PartialEq)]
enum Found<'a> {
    /// A failed `USER_AUTH`, `USER_LOGIN` or `USER_ACCT`.
    FailedLogin {
        kind: &'a str,
        who: &'a str,
        via: &'a str,
    },
    /// A successful `USER_AUTH`, `USER_LOGIN` or `USER_CMD` (sudo): kept, without an alert.
    Login { kind: &'a str },
    /// A `SYSCALL` that hit one of our rules (`bb_*` keys, `deploy/blackbox.rules`).
    RuleHit { key: &'a str, via: &'a str },
    /// A network interface entering or leaving promiscuous mode. The kernel writes it with no pid
    /// or exe: the interface is what identifies it.
    Promiscuous {
        dev: Option<&'a str>,
        prom: Option<&'a str>,
    },
    /// Any other `ANOM_*` (a crash, …) or an `AVC` denial.
    Anomaly {
        kind: &'a str,
        via: &'a str,
        pid: Option<i32>,
    },
}

/// Which of the records worth keeping this line is; `None` for everything else.
fn classify(line: &str) -> Option<Found<'_>> {
    let kind = line.strip_prefix("type=")?.split_whitespace().next()?;
    let via = field(line, "exe")
        .or_else(|| field(line, "comm"))
        .unwrap_or("?");
    let failed = field(line, "res") == Some("failed");
    Some(match kind {
        "USER_AUTH" | "USER_LOGIN" | "USER_ACCT" if failed => Found::FailedLogin {
            kind,
            who: field(line, "acct").unwrap_or("?"),
            via,
        },
        "USER_AUTH" | "USER_LOGIN" | "USER_CMD" => Found::Login { kind },
        "SYSCALL" => Found::RuleHit {
            key: field(line, "key").filter(|k| k.starts_with("bb_"))?,
            via,
        },
        "ANOM_PROMISCUOUS" => Found::Promiscuous {
            dev: field(line, "dev"),
            prom: field(line, "prom"),
        },
        k if k.starts_with("ANOM_") || k == "AVC" => Found::Anomaly {
            kind: k,
            via,
            pid: number(line, "pid"),
        },
        _ => return None,
    })
}

impl Found<'_> {
    /// What is stored as the event type: the record type, or for a rule hit the rule's key.
    fn event_type(&self) -> &str {
        match self {
            Found::FailedLogin { kind, .. }
            | Found::Login { kind }
            | Found::Anomaly { kind, .. } => kind,
            Found::RuleHit { key, .. } => key,
            Found::Promiscuous { .. } => "ANOM_PROMISCUOUS",
        }
    }

    /// How much it matters, and the one line the viewer shows. `booted` is when this boot started.
    fn alert(&self, ts: i64, booted: Option<i64>) -> Option<(Severity, String)> {
        Some(match *self {
            Found::FailedLogin { kind, who, via } => (
                Severity::Warning,
                format!("{kind} failed for {who} via {via}"),
            ),
            Found::Login { .. } => return None,
            Found::RuleHit { key, via } => {
                // the kernel loads about 30 modules while it brings up drivers: expected, not a
                // warning
                let booting = booted.is_some_and(|b| (0..BOOT_SETTLE_SECS).contains(&(ts - b)));
                let severity = if key == "bb_modules" && booting {
                    Severity::Info
                } else {
                    Severity::Warning
                };
                (severity, format!("audit rule {key} hit by {via}"))
            }
            Found::Promiscuous { dev, prom } => {
                let severity = if routine::promiscuous(dev, prom) {
                    Severity::Info
                } else {
                    Severity::Error
                };
                let left = if prom == Some("0") {
                    " (leaving it)"
                } else {
                    ""
                };
                let text = format!("ANOM_PROMISCUOUS from {}{left}", dev.unwrap_or("?"));
                (severity, text)
            }
            Found::Anomaly { kind, via, pid } => (
                Severity::Error,
                format!("{kind} from {via} (pid {})", pid.unwrap_or(0)),
            ),
        })
    }
}

fn number(line: &str, name: &str) -> Option<i32> {
    field(line, name).and_then(|v| v.parse().ok())
}

/// The record to store for one line of the audit log, if it is one worth keeping.
pub fn parse_line(line: &str, booted: Option<i64>) -> Option<Audit> {
    let ts = line_ts(line)?;
    let found = classify(line)?;
    let exe = field(line, "exe").or_else(|| field(line, "comm"));
    Some(Audit {
        ts,
        event_type: found.event_type().to_string(),
        pid: number(line, "pid"),
        uid: number(line, "uid"),
        executable: exe.map(String::from),
        alert: found.alert(ts, booted),
    })
}

pub(super) fn line_ts(line: &str) -> Option<i64> {
    line.split("audit(").nth(1)?.split('.').next()?.parse().ok()
}

/// Module loads in the first minutes after boot are the kernel bringing up its drivers.
const BOOT_SETTLE_SECS: i64 = 180;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_auth_is_a_warning() {
        let line = r#"type=USER_AUTH msg=audit(1700000000.123:456): pid=1234 uid=1000 auid=1000 ses=2 msg='op=PAM:authentication grantors=? acct="alice" exe="/usr/bin/sudo" hostname=? addr=? terminal=/dev/pts/0 res=failed'"#;
        let e = parse_line(line, None).unwrap();
        assert_eq!(e.ts, 1700000000);
        assert_eq!(e.event_type, "USER_AUTH");
        assert_eq!(e.pid, Some(1234));
        assert_eq!(e.uid, Some(1000));
        assert_eq!(e.executable.as_deref(), Some("/usr/bin/sudo"));
        let (sev, text) = e.alert.unwrap();
        assert_eq!(sev, Severity::Warning);
        assert!(text.contains("alice") && text.contains("/usr/bin/sudo"));
    }

    #[test]
    fn successful_sudo_is_stored_without_alert() {
        let line = r#"type=USER_CMD msg=audit(1700000001.000:457): pid=1234 uid=1000 auid=1000 ses=2 msg='cwd="/home/alice" cmd=6C73 exe="/usr/bin/sudo" terminal=pts/0 res=success'"#;
        let e = parse_line(line, None).unwrap();
        assert_eq!(e.event_type, "USER_CMD");
        assert!(e.alert.is_none());
    }

    #[test]
    fn own_rule_hit_is_a_warning() {
        let line = r#"type=SYSCALL msg=audit(1700000002.500:458): arch=c000003e syscall=175 success=yes exit=0 ppid=1 pid=99 auid=1000 uid=0 comm="insmod" exe="/usr/bin/kmod" key="bb_modules""#;
        let e = parse_line(line, None).unwrap();
        assert_eq!(e.event_type, "bb_modules");
        assert_eq!(e.pid, Some(99));
        assert_eq!(e.uid, Some(0));
        assert_eq!(e.alert.unwrap().0, Severity::Warning);
    }

    #[test]
    fn anomaly_is_an_error() {
        let line = r#"type=ANOM_ABEND msg=audit(1700000003.000:459): auid=1000 uid=1000 pid=555 comm="app" exe="/usr/bin/app" sig=11 res=1"#;
        let e = parse_line(line, None).unwrap();
        assert_eq!(e.alert.unwrap().0, Severity::Error);
    }

    #[test]
    fn promiscuous_bridge_ports_are_routine() {
        let line = |dev: &str, prom: u32| {
            format!(
                "type=ANOM_PROMISCUOUS msg=audit(1700000003.000:460): dev={dev} prom={prom} old_prom=0 auid=4294967295 uid=0 gid=0 ses=4294967295"
            )
        };
        let docker = parse_line(&line("veth14fb608", 256), None)
            .unwrap()
            .alert
            .unwrap();
        assert_eq!(
            docker,
            (
                Severity::Info,
                "ANOM_PROMISCUOUS from veth14fb608".to_string()
            )
        );
        let sniffer = parse_line(&line("physnic9", 256), None)
            .unwrap()
            .alert
            .unwrap();
        assert_eq!(sniffer.0, Severity::Error);
        let left = parse_line(&line("physnic9", 0), None)
            .unwrap()
            .alert
            .unwrap();
        assert_eq!(
            left,
            (
                Severity::Info,
                "ANOM_PROMISCUOUS from physnic9 (leaving it)".to_string()
            )
        );
    }

    #[test]
    fn noise_and_foreign_syscalls_are_ignored() {
        assert!(parse_line(r#"type=SERVICE_START msg=audit(1700000004.000:460): pid=1 uid=0 msg='unit=foo res=success'"#, None).is_none());
        assert!(
            parse_line(
                r#"type=SYSCALL msg=audit(1700000005.000:461): pid=5 uid=0 comm="ls" key="other""#,
                None
            )
            .is_none()
        );
        assert!(parse_line("garbage", None).is_none());
    }

    #[test]
    fn module_loads_while_booting_are_not_warnings() {
        let line = r#"type=SYSCALL msg=audit(1000100.000:1): pid=9 uid=0 comm="modprobe" exe="/usr/bin/kmod" key="bb_modules""#;
        let severity = |booted| parse_line(line, booted).unwrap().alert.unwrap().0;
        assert_eq!(
            severity(Some(1_000_000)),
            Severity::Info,
            "100 s after boot"
        );
        assert_eq!(
            severity(Some(999_000)),
            Severity::Warning,
            "1100 s after boot"
        );
        assert_eq!(
            severity(Some(1_000_200)),
            Severity::Warning,
            "from before this boot (backfilled)"
        );
        assert_eq!(severity(None), Severity::Warning);
    }

    #[test]
    fn a_line_is_classified_before_it_is_rated() {
        let line = r#"type=SYSCALL msg=audit(1000100.000:1): pid=9 uid=0 comm="modprobe" exe="/usr/bin/kmod" key="bb_modules""#;
        assert_eq!(
            classify(line),
            Some(Found::RuleHit {
                key: "bb_modules",
                via: "/usr/bin/kmod"
            })
        );
    }

    #[test]
    fn ppid_is_not_mistaken_for_pid() {
        let line = r#"type=SYSCALL msg=audit(1700000006.000:462): ppid=7 pid=8 uid=0 comm="x" exe="/x" key="bb_identity""#;
        assert_eq!(parse_line(line, None).unwrap().pid, Some(8));
    }
}
