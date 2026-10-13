use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Wait after the process ended, before starting it again.
const AGAIN: Duration = Duration::from_secs(5);
/// Wait when there is nothing to follow yet (no audit log), before looking again.
const NOT_READY: Duration = Duration::from_secs(30);

/// What a followed source supplies to the loop.
pub trait Follower {
    /// The process to follow, set up to resume after what is already stored. `None` while there
    /// is nothing to follow yet.
    fn command(&mut self) -> Option<Command>;
    /// One line of its output.
    fn line(&mut self, line: &str);
    /// The process ended, after this many lines. A following process only ends when it cannot go
    /// on (a log removed, a journal position gone).
    fn ended(&mut self, _lines: usize) {}
}

/// Follows forever: runs the process, hands over every line, runs it again when it ends.
pub fn follow(f: &mut impl Follower) {
    loop {
        let wait = follow_once(f);
        std::thread::sleep(wait);
    }
}

/// The followed process, stopped and reaped however the run ends: normally it has already exited,
/// but if `line` panics the process would otherwise keep running, and stay behind as a zombie
/// after each restart of the source.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One run of the process. Returns how long to wait before the next.
fn follow_once(f: &mut impl Follower) -> Duration {
    let Some(mut command) = f.command() else {
        return NOT_READY;
    };
    let program = command.get_program().to_string_lossy().into_owned();
    match command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => {
            let mut child = Reaped(child);
            let mut lines = 0;
            if let Some(out) = child.0.stdout.take() {
                // Raw lines, decoded leniently: one that is not valid UTF-8 (a stray byte in the
                // audit log) is passed on with `�` in it. Stopping there would restart the process
                // at the same place, forever.
                let mut out = BufReader::new(out);
                let mut raw = Vec::new();
                while matches!(out.read_until(b'\n', &mut raw), Ok(n) if n > 0) {
                    lines += 1;
                    let line = String::from_utf8_lossy(&raw);
                    f.line(line.trim_end_matches(['\n', '\r']));
                    raw.clear();
                }
            }
            drop(child);
            f.ended(lines);
        }
        Err(e) => eprintln!("{program} unavailable: {e}"),
    }
    AGAIN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Follows `printf`, which prints two lines and ends.
    #[derive(Default)]
    struct Fake {
        ready: bool,
        lines: Vec<String>,
        ended: Option<usize>,
    }

    impl Follower for Fake {
        fn command(&mut self) -> Option<Command> {
            self.ready.then(|| {
                let mut c = Command::new("printf");
                c.arg("first\\nsecond\\n");
                c
            })
        }
        fn line(&mut self, line: &str) {
            self.lines.push(line.to_string());
        }
        fn ended(&mut self, lines: usize) {
            self.ended = Some(lines);
        }
    }

    #[test]
    fn every_line_is_handed_over_and_the_end_is_reported() {
        let mut f = Fake {
            ready: true,
            ..Fake::default()
        };
        assert_eq!(follow_once(&mut f), AGAIN);
        assert_eq!(f.lines, ["first", "second"]);
        assert_eq!(f.ended, Some(2));
    }

    /// Follows a process that prints its pid and then waits, and panics on that first line.
    struct Panics {
        pid: Option<u32>,
    }

    impl Follower for Panics {
        fn command(&mut self) -> Option<Command> {
            let mut c = Command::new("sh");
            c.args(["-c", "echo $$; exec sleep 30"]);
            Some(c)
        }
        fn line(&mut self, line: &str) {
            self.pid = line.parse().ok();
            panic!("a follower failed (expected in this test)");
        }
    }

    #[test]
    fn a_panicking_follower_leaves_no_process_behind() {
        let mut f = Panics { pid: None };
        let started = std::time::Instant::now();
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| follow_once(&mut f)));
        assert!(run.is_err());
        // stopped, not waited for: its `sleep 30` would still be running
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the process was not stopped"
        );
        let pid = f.pid.expect("the process printed its pid");
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "process {pid} is still there"
        );
    }

    /// Follows `printf` printing a line with a byte that is not UTF-8 between two good ones.
    #[derive(Default)]
    struct BadByte {
        lines: Vec<String>,
    }

    impl Follower for BadByte {
        fn command(&mut self) -> Option<Command> {
            let mut c = Command::new("printf");
            c.arg("first\\n\\377second\\nthird\\n");
            Some(c)
        }
        fn line(&mut self, line: &str) {
            self.lines.push(line.to_string());
        }
    }

    #[test]
    fn a_line_that_is_not_utf8_does_not_stop_the_rest() {
        let mut f = BadByte::default();
        follow_once(&mut f);
        assert_eq!(f.lines, ["first", "\u{FFFD}second", "third"]);
    }

    #[test]
    fn nothing_to_follow_waits_longer_and_runs_nothing() {
        let mut f = Fake::default();
        assert_eq!(follow_once(&mut f), NOT_READY);
        assert!(f.lines.is_empty());
        assert_eq!(f.ended, None);
    }
}
