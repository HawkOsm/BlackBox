use crate::database;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::Duration;

pub struct AuditEvent {
    pub ts: i64,
    pub event_type: String,
    pub pid: Option<i32>,
    pub uid: Option<i32>,
    pub executable: Option<String>,
    pub alert: Option<(&'static str, String)>,
}

fn log_path() -> String {
    std::env::var("BLACKBOX_AUDIT_LOG").unwrap_or_else(|_| "/var/log/audit/audit.log".to_string())
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!(" {name}=");
    let start = line.find(&pattern)? + pattern.len();
    let rest = &line[start..];
    match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next(),
        None => rest.split(|c: char| c.is_whitespace() || c == '\'').next(),
    }
}

/// Keeps only the low-noise, high-value records: failed logins and auth, sudo usage,
/// anomalies (crashes, promiscuous mode), MAC denials, and hits on our own `bb_*` rules.
pub fn parse_line(line: &str) -> Option<AuditEvent> {
    let kind = line.strip_prefix("type=")?.split_whitespace().next()?;
    let ts = line_ts(line)?;
    let pid = field(line, "pid").and_then(|v| v.parse().ok());
    let uid = field(line, "uid").and_then(|v| v.parse().ok());
    let key = field(line, "key");
    let who = field(line, "acct").unwrap_or("?");
    let exe = field(line, "exe")
        .or_else(|| field(line, "comm"))
        .map(String::from);
    let via = exe.as_deref().unwrap_or("?");
    let failed = field(line, "res") == Some("failed");

    let (event_type, alert) = match kind {
        "USER_AUTH" | "USER_LOGIN" | "USER_ACCT" if failed => (
            kind.to_string(),
            Some(("warning", format!("{kind} failed for {who} via {via}"))),
        ),
        "USER_AUTH" | "USER_LOGIN" | "USER_CMD" => (kind.to_string(), None),
        "SYSCALL" if key.is_some_and(|k| k.starts_with("bb_")) => {
            let k = key?;
            (
                k.to_string(),
                Some(("warning", format!("audit rule {k} hit by {via}"))),
            )
        }
        k if k.starts_with("ANOM_") || k == "AVC" => (
            k.to_string(),
            Some((
                "error",
                format!("{k} from {via} (pid {})", pid.unwrap_or(0)),
            )),
        ),
        _ => return None,
    };
    Some(AuditEvent {
        ts,
        event_type,
        pid,
        uid,
        executable: exe,
        alert,
    })
}

fn line_ts(line: &str) -> Option<i64> {
    line.split("audit(").nth(1)?.split('.').next()?.parse().ok()
}

/// The first line that starts at or after byte `pos`, with its start and timestamp.
fn line_at(file: &mut std::fs::File, pos: u64) -> Option<(u64, i64)> {
    let mut start = pos;
    let mut buf = vec![0u8; 16 * 1024];
    if pos > 0 {
        // the line containing pos - 1 is cut: skip past its newline
        file.seek(SeekFrom::Start(pos - 1)).ok()?;
        let n = file.read(&mut buf).ok()?;
        start = pos - 1 + buf[..n].iter().position(|&b| b == b'\n')? as u64 + 1;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let n = file.read(&mut buf).ok()?;
    let end = buf[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
    let ts = line_ts(&String::from_utf8_lossy(&buf[..end]))?;
    Some((start, ts))
}

/// Byte offset of the first record newer than `after`, found by binary search (the log is in
/// time order), so a restart picks up what happened while the collector was down without
/// reading the whole file. Records in rotated-away files are not recovered.
pub fn resume_offset(path: &str, after: i64) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let (mut lo, mut hi) = (0u64, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match line_at(&mut file, mid) {
            Some((_, ts)) if ts <= after => lo = mid + 1,
            _ => hi = mid,
        }
    }
    line_at(&mut file, lo).map_or(len, |(start, _)| start)
}

/// Module loads in the first minutes after boot are the kernel bringing up its drivers.
const BOOT_SETTLE_SECS: i64 = 180;

/// A module load while the system is still booting is expected: keep it, but not as a warning.
fn boot_noise(e: &AuditEvent, booted: Option<i64>) -> bool {
    e.event_type == "bb_modules" && booted.is_some_and(|b| (0..BOOT_SETTLE_SECS).contains(&(e.ts - b)))
}

pub fn start() {
    std::thread::spawn(|| {
        let booted = crate::boot::boot_time();
        loop {
            let path = log_path();
            if std::fs::File::open(&path).is_err() {
                // auditd not installed/running, or the log isn't readable: check again later
                std::thread::sleep(Duration::from_secs(30));
                continue;
            }
            // resume after the newest stored record; the first time, start at the end
            let from = match database::last_ts("auditd").as_deref().and_then(database::ts_epoch) {
                Some(after) => format!("+{}", resume_offset(&path, after) + 1),
                None => format!("+{}", std::fs::metadata(&path).map_or(0, |m| m.len()) + 1),
            };
            let child = Command::new("tail")
                .args(["-c", &from, "-F", &path])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                if let Some(out) = child.stdout.take() {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if let Some(e) = parse_line(&line) {
                            let quiet = boot_noise(&e, booted);
                            database::add_auditd_event(
                                e.ts,
                                &e.event_type,
                                e.pid,
                                e.uid,
                                e.executable.as_deref(),
                                e.alert.as_ref().map(|(sev, text)| {
                                    (if quiet { "info" } else { *sev }, text.as_str())
                                }),
                            );
                        }
                    }
                }
                let _ = child.wait();
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_auth_is_a_warning() {
        let line = r#"type=USER_AUTH msg=audit(1700000000.123:456): pid=1234 uid=1000 auid=1000 ses=2 msg='op=PAM:authentication grantors=? acct="alice" exe="/usr/bin/sudo" hostname=? addr=? terminal=/dev/pts/0 res=failed'"#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.ts, 1700000000);
        assert_eq!(e.event_type, "USER_AUTH");
        assert_eq!(e.pid, Some(1234));
        assert_eq!(e.uid, Some(1000));
        assert_eq!(e.executable.as_deref(), Some("/usr/bin/sudo"));
        let (sev, text) = e.alert.unwrap();
        assert_eq!(sev, "warning");
        assert!(text.contains("alice") && text.contains("/usr/bin/sudo"));
    }

    #[test]
    fn successful_sudo_is_stored_without_alert() {
        let line = r#"type=USER_CMD msg=audit(1700000001.000:457): pid=1234 uid=1000 auid=1000 ses=2 msg='cwd="/home/alice" cmd=6C73 exe="/usr/bin/sudo" terminal=pts/0 res=success'"#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.event_type, "USER_CMD");
        assert!(e.alert.is_none());
    }

    #[test]
    fn own_rule_hit_is_a_warning() {
        let line = r#"type=SYSCALL msg=audit(1700000002.500:458): arch=c000003e syscall=175 success=yes exit=0 ppid=1 pid=99 auid=1000 uid=0 comm="insmod" exe="/usr/bin/kmod" key="bb_modules""#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.event_type, "bb_modules");
        assert_eq!(e.pid, Some(99));
        assert_eq!(e.uid, Some(0));
        assert_eq!(e.alert.unwrap().0, "warning");
    }

    #[test]
    fn anomaly_is_an_error() {
        let line = r#"type=ANOM_ABEND msg=audit(1700000003.000:459): auid=1000 uid=1000 pid=555 comm="app" exe="/usr/bin/app" sig=11 res=1"#;
        let e = parse_line(line).unwrap();
        assert_eq!(e.alert.unwrap().0, "error");
    }

    #[test]
    fn noise_and_foreign_syscalls_are_ignored() {
        assert!(parse_line(r#"type=SERVICE_START msg=audit(1700000004.000:460): pid=1 uid=0 msg='unit=foo res=success'"#).is_none());
        assert!(
            parse_line(
                r#"type=SYSCALL msg=audit(1700000005.000:461): pid=5 uid=0 comm="ls" key="other""#
            )
            .is_none()
        );
        assert!(parse_line("garbage").is_none());
    }

    #[test]
    fn resumes_after_the_last_stored_record() {
        let path = std::env::temp_dir().join(format!("bb_audit_{}.log", std::process::id()));
        let lines: Vec<String> = (0..500)
            .map(|i| format!("type=SYSCALL msg=audit({}.000:{i}): pid=1 {}\n", 1000 + i / 2, "x".repeat(i % 97)))
            .collect();
        std::fs::write(&path, lines.concat()).unwrap();
        let p = path.to_str().unwrap();
        let offset_of = |i: usize| lines[..i].iter().map(|l| l.len() as u64).sum::<u64>();
        assert_eq!(resume_offset(p, 999), 0, "nothing stored yet in this range: from the top");
        assert_eq!(resume_offset(p, 1000), offset_of(2), "past both lines of second 1000");
        assert_eq!(resume_offset(p, 1123), offset_of(248));
        assert_eq!(resume_offset(p, 5000), offset_of(500), "all stored: from the end");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn module_loads_while_booting_are_not_warnings() {
        let line = r#"type=SYSCALL msg=audit(1000100.000:1): pid=9 uid=0 comm="modprobe" exe="/usr/bin/kmod" key="bb_modules""#;
        let e = parse_line(line).unwrap();
        assert!(boot_noise(&e, Some(1_000_000)), "100 s after boot");
        assert!(!boot_noise(&e, Some(999_000)), "1100 s after boot");
        assert!(!boot_noise(&e, Some(1_000_200)), "from before this boot (backfilled)");
        assert!(!boot_noise(&e, None));
    }

    #[test]
    fn ppid_is_not_mistaken_for_pid() {
        let line = r#"type=SYSCALL msg=audit(1700000006.000:462): ppid=7 pid=8 uid=0 comm="x" exe="/x" key="bb_identity""#;
        assert_eq!(parse_line(line).unwrap().pid, Some(8));
    }
}
