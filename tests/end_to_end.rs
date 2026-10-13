//! End to end: the real `blackbox` binary, run against a throwaway database and fixture logs.
//! Every path it reads is pointed at a temporary folder, so nothing depends on this machine's own
//! package history or audit log.
//!
//! The recording test needs the kernel's process events (netlink), which a container does not
//! give: there it says it is skipped and passes, and only `--check` and the arguments are tested.

use rusqlite::{Connection, OpenFlags};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_blackbox");

/// One pacman transaction, as pacman writes its log.
const PACMAN_LOG: &str = "\
[2026-09-22T22:10:40+0300] [PACMAN] Running 'pacman -S tree'
[2026-09-22T22:10:41+0300] [ALPM] transaction started
[2026-09-22T22:10:41+0300] [ALPM] installed tree (2.2.1-1)
[2026-09-22T22:10:41+0300] [ALPM] transaction completed
";

/// A temporary folder with the fixture logs, removed when dropped.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bb_e2e_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pacman.log"), PACMAN_LOG).unwrap();
        std::fs::write(dir.join("audit.log"), "").unwrap();
        Fixture { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The binary, with every path it reads or writes inside this folder.
    fn command(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env("BLACKBOX_DB", self.path("blackbox.db"))
            .env("BLACKBOX_PACMAN_LOG", self.path("pacman.log"))
            .env("BLACKBOX_AUDIT_LOG", self.path("audit.log"))
            // not there: these managers are left out
            .env("BLACKBOX_APT_LOG", self.path("none"))
            .env("BLACKBOX_ZYPPER_LOG", self.path("none"))
            .env("BLACKBOX_DNF_DB", self.path("none"));
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// Starts the recorder; `None` when this machine gives it no process events (a container).
    /// Waits for it to say it subscribed, or to exit, rather than for a fixed time.
    fn start(&self) -> Option<Recorder> {
        let mut child = self
            .command()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (said, heard) = std::sync::mpsc::channel();
        // stderr is read all along, so the recorder never blocks on a full pipe
        let errors = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let (err_pipe, collected) = (child.stderr.take().unwrap(), errors.clone());
        let reader = std::thread::spawn(move || {
            let lines = std::io::BufRead::lines(std::io::BufReader::new(err_pipe));
            for line in lines.map_while(Result::ok) {
                let mut all = collected.lock().unwrap();
                all.push_str(&line);
                all.push('\n');
            }
        });
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in std::io::BufRead::lines(std::io::BufReader::new(out)).map_while(Result::ok)
            {
                if line.contains("subscribed to kernel process events") {
                    let _ = said.send(());
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                reader.join().unwrap();
                let err = errors.lock().unwrap().clone();
                // the three ways the collector's subscription fails (kernel/collector/subscribe.rs)
                let no_events = ["netlink socket", "proc connector", "listen message"];
                assert!(
                    no_events.iter().any(|m| err.contains(m)),
                    "the recorder stopped ({status}) for another reason: {err}"
                );
                eprintln!("skipped: no kernel process events here ({})", err.trim());
                return None;
            }
            if heard.try_recv().is_ok() {
                return Some(Recorder(child));
            }
            assert!(
                Instant::now() < deadline,
                "the recorder neither subscribed nor stopped within 20 s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// One number from the database, read-only, the way the viewer reads it.
    fn count(&self, sql: &str) -> i64 {
        let c =
            Connection::open_with_flags(self.path("blackbox.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Waits up to `limit` for a count to reach `want`; returns the last count seen.
    fn wait_for(&self, sql: &str, want: i64, limit: Duration) -> i64 {
        let deadline = Instant::now() + limit;
        loop {
            let n = if self.path("blackbox.db").exists() {
                self.count(sql)
            } else {
                0
            };
            if n >= want || Instant::now() > deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The running recorder, stopped when dropped.
struct Recorder(Child);

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

#[test]
fn version_and_help_answer_and_an_unknown_argument_fails() {
    let f = Fixture::new("args");
    let version = f.run(&["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("blackbox "));
    assert!(f.run(&["--help"]).status.success());
    assert!(!f.run(&["--bogus"]).status.success());
}

#[test]
fn check_reports_the_package_history_without_writing_anything() {
    let f = Fixture::new("check");
    let out = f.run(&["--check"]);
    assert!(out.status.success());
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(
        report.contains("pacman (") && report.contains("1 transactions"),
        "{report}"
    );
    assert!(
        report.contains("tree"),
        "the newest transaction is named: {report}"
    );
    assert!(
        !f.path("blackbox.db").exists(),
        "--check never creates the database"
    );
}

#[test]
fn records_from_every_kind_of_source_and_resumes_without_duplicates() {
    let f = Fixture::new("record");
    let Some(recorder) = f.start() else {
        return;
    };

    // poll: the pacman history is imported at once
    assert_eq!(
        f.wait_for("SELECT COUNT(*) FROM packages", 1, Duration::from_secs(10)),
        1,
        "the pacman transaction"
    );
    // once: this boot is recorded
    assert_eq!(
        f.wait_for(
            "SELECT COUNT(*) FROM boot WHERE kind = 'start'",
            1,
            Duration::from_secs(10)
        ),
        1
    );

    // follow: a line appended to the audit log, a hit on one of our rules
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    append(
        &f.path("audit.log"),
        &format!(
            "type=SYSCALL msg=audit({now}.000:1): pid=9 uid=0 comm=\"modprobe\" exe=\"/usr/bin/kmod\" key=\"bb_modules\"\n"
        ),
    );
    assert_eq!(
        f.wait_for(
            "SELECT COUNT(*) FROM auditd WHERE event_type = 'bb_modules'",
            1,
            Duration::from_secs(10)
        ),
        1,
        "the audit line"
    );

    // kernel: a real crash, found by its pid and signal: its name is best effort (a process this
    // short-lived may be gone before it can be read), so the summary is not relied on
    let mut crash = Command::new("sh")
        .args(["-c", "kill -SEGV $$"])
        .spawn()
        .unwrap();
    let pid = crash.id();
    assert!(!crash.wait().unwrap().success());
    assert_eq!(
        f.wait_for(
            &format!(
                "SELECT COUNT(*) FROM custom c JOIN messages m
                 ON m.source = 'custom' AND m.ref_id = c.custom_id
                 WHERE c.pid = {pid} AND (c.exit_code & 127) = 11 AND m.severity = 'error'"
            ),
            1,
            Duration::from_secs(10),
        ),
        1,
        "the crash"
    );

    // poll: the first system sample, 10 s after the first look
    assert!(
        f.wait_for("SELECT COUNT(*) FROM sysstat", 1, Duration::from_secs(20)) >= 1,
        "a sample"
    );

    // a restart reads every history again, and stores none of it twice
    drop(recorder);
    let again = f.start().expect("it started the first time");
    std::thread::sleep(Duration::from_secs(4));
    drop(again);
    assert_eq!(f.count("SELECT COUNT(*) FROM packages"), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM auditd"), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM boot WHERE kind = 'start'"), 1);
}
