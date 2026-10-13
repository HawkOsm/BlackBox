use std::time::{Duration, Instant};

/// The wait before running a source again after its first panic; it doubles with each panic in a
/// row, up to `RESTART_MAX`.
const RESTART: Duration = Duration::from_secs(30);
const RESTART_MAX: Duration = Duration::from_secs(30 * 60);
/// A source that ran this long before it panicked was working: its next panic starts over at
/// `RESTART`.
const STEADY: Duration = Duration::from_secs(10 * 60);

/// Runs a source on a thread named after it. A source that panics is reported and run again, with
/// the state it had: otherwise it would stay silent for the rest of the run while everything else
/// carries on. The wait starts at 30 s and doubles while it keeps panicking, up to 30 min, so one
/// that always panics does not fill the journal. One that returns (boot) is done.
pub fn supervise(name: &str, run: impl FnMut() + Send + 'static) {
    supervise_with(name, RESTART, run);
}

fn supervise_with(name: &str, first: Duration, mut run: impl FnMut() + Send + 'static) {
    let label = name.to_string();
    std::thread::Builder::new()
        .name(label.clone())
        .spawn(move || {
            let mut wait = None;
            loop {
                let started = Instant::now();
                // the panic message itself is printed by Rust, with this thread's name
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut run)).is_ok() {
                    return;
                }
                let next = next_wait(wait, started.elapsed(), first);
                eprintln!("{label}: stopped by a panic; running it again in {next:?}");
                std::thread::sleep(next);
                wait = Some(next);
            }
        })
        .expect("cannot start a thread");
}

/// The wait before the next run: `first` after the first panic, or after a run that was working
/// for a while; otherwise twice the last wait, at most `RESTART_MAX`.
fn next_wait(last: Option<Duration>, ran: Duration, first: Duration) -> Duration {
    match last {
        Some(last) if ran < STEADY => (last * 2).min(RESTART_MAX),
        _ => first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn restarts_back_off_and_start_over_after_a_steady_run() {
        let quick = Duration::from_secs(1);
        let mut wait = None;
        let mut waits = Vec::new();
        for _ in 0..8 {
            let next = next_wait(wait, quick, RESTART);
            waits.push(next.as_secs());
            wait = Some(next);
        }
        assert_eq!(waits, [30, 60, 120, 240, 480, 960, 1800, 1800]);
        assert_eq!(
            next_wait(Some(RESTART_MAX), STEADY, RESTART),
            RESTART,
            "it ran fine for 10 min: a new problem starts over"
        );
    }

    #[test]
    fn a_source_that_panics_runs_again_and_one_that_returns_is_done() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = runs.clone();
        let (done, finished) = std::sync::mpsc::channel();
        supervise_with("test-source", Duration::from_millis(1), move || {
            // panics on its first two runs, returns on the third
            if counted.fetch_add(1, Ordering::SeqCst) < 2 {
                panic!("a source failed (expected in this test)");
            }
            done.send(()).unwrap();
        });
        finished
            .recv_timeout(Duration::from_secs(5))
            .expect("the source was not run again");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(runs.load(Ordering::SeqCst), 3, "and not run a fourth time");
    }
}
