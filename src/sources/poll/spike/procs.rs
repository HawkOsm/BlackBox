//! Each process's CPU use right now: its CPU time read twice, a second apart. `ps` shows the
//! average over the process's whole life, which hides a burst in a long-running program.
//!
//! Context switches are counted afterwards, for the winners only: they are per thread, and reading
//! every thread of every process (~2,200 files here) cost ~25 ms a look, for 5 numbers.

use super::store::SpikeProc;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One look at a process.
pub(super) struct ProcTimes {
    comm: String,
    /// user + system time, in clock ticks, all threads
    ticks: u64,
}

/// Every process now.
pub(super) fn read_all() -> HashMap<i32, ProcTimes> {
    let mut all = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return all;
    };
    for entry in dir.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue; // it just exited
        };
        let Some((comm, ticks)) = parse_stat(&stat) else {
            continue;
        };
        all.insert(pid, ProcTimes { comm, ticks });
    }
    all
}

/// (comm, utime + stime) from `/proc/<pid>/stat`. The name is in brackets and may hold spaces or
/// brackets itself, so the fields are counted from the last `)`.
fn parse_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    // after the name: state is field 3, utime 14, stime 15
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((comm, utime + stime))
}

/// Voluntary plus involuntary switches of one `status` file.
fn ctxt(status: &str) -> Option<u64> {
    let field = |name: &str| -> Option<u64> {
        status
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    Some(field("voluntary_ctxt_switches:")? + field("nonvoluntary_ctxt_switches:")?)
}

/// The process's own `status` counts only its main thread; the threads are in `task/`.
fn threads_ctxt(pid: i32) -> Option<u64> {
    let mut sum = 0;
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))
        .ok()?
        .flatten()
    {
        let status = std::fs::read_to_string(task.path().join("status")).ok()?;
        sum += ctxt(&status)?;
    }
    Some(sum)
}

/// Fills in each process's context switches per second, counted over `wait`.
pub(super) fn add_switch_rates(procs: &mut [SpikeProc], wait: Duration) {
    let before: Vec<Option<u64>> = procs.iter().map(|p| threads_ctxt(p.pid)).collect();
    let start = Instant::now();
    std::thread::sleep(wait);
    let secs = start.elapsed().as_secs_f64();
    for (p, b) in procs.iter_mut().zip(before) {
        p.ctxt_per_s = rate(b, threads_ctxt(p.pid), secs);
    }
}

/// Switches per second between two counts; None when either is missing, or the count fell (a
/// thread that exited between the looks takes its switches with it).
fn rate(before: Option<u64>, after: Option<u64>, secs: f64) -> Option<f64> {
    let delta = after?.checked_sub(before?)?;
    (secs > 0.0).then(|| delta as f64 / secs)
}

/// The `n` processes that used the most CPU between two looks `secs` apart. A process missing from
/// either look, or with no CPU time at all, is left out. Their switches are not counted yet.
pub(super) fn busiest(
    before: &HashMap<i32, ProcTimes>,
    after: &HashMap<i32, ProcTimes>,
    secs: f64,
    n: usize,
) -> Vec<SpikeProc> {
    let hz = clock_ticks();
    let mut out: Vec<SpikeProc> = Vec::new();
    for (pid, a) in after {
        let Some(b) = before.get(pid) else { continue };
        let Some(ticks) = a.ticks.checked_sub(b.ticks).filter(|t| *t > 0) else {
            continue; // idle, or the pid was reused by a new process
        };
        out.push(SpikeProc {
            pid: *pid,
            comm: a.comm.clone(),
            cpu_pct: 100.0 * ticks as f64 / hz / secs,
            ctxt_per_s: None,
        });
    }
    out.sort_by(|x, y| y.cpu_pct.total_cmp(&x.cpu_pct));
    out.truncate(n);
    out
}

fn clock_ticks() -> f64 {
    // SAFETY: sysconf only reads a constant of the running system
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 { hz as f64 } else { 100.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_spaces_and_brackets_does_not_shift_the_fields() {
        let stat =
            "4242 (Web (Content) x) S 1 4242 4242 0 -1 4194560 100 0 0 0 731 52 0 0 20 0 30 0";
        assert_eq!(parse_stat(stat), Some(("Web (Content) x".to_string(), 783)));
        assert_eq!(parse_stat("4242 (short) S 1"), None);
    }

    #[test]
    fn switches_add_both_kinds() {
        let status = "Name:\tx\nvoluntary_ctxt_switches:\t40\nnonvoluntary_ctxt_switches:\t2\n";
        assert_eq!(ctxt(status), Some(42));
    }

    #[test]
    fn a_rate_needs_both_counts_and_a_count_that_did_not_fall() {
        assert_eq!(rate(Some(100), Some(350), 0.5), Some(500.0));
        assert_eq!(rate(Some(100), Some(90), 0.5), None);
        assert_eq!(rate(None, Some(90), 0.5), None);
    }

    #[test]
    fn the_busiest_are_ranked_by_the_change_not_the_total() {
        let times = |comm: &str, ticks| ProcTimes {
            comm: comm.into(),
            ticks,
        };
        let hz = clock_ticks();
        let t = |secs: f64| (secs * hz) as u64;
        // `old` has used far more CPU in its life, `burst` far more in this second
        let before = HashMap::from([
            (1, times("old", t(5000.0))),
            (2, times("burst", 0)),
            (3, times("idle", 7)),
        ]);
        let after = HashMap::from([
            (1, times("old", t(5000.1))),
            (2, times("burst", t(0.9))),
            (3, times("idle", 7)),
            (4, times("new", t(1.0))),
        ]);
        let top = busiest(&before, &after, 1.0, 5);
        let names: Vec<&str> = top.iter().map(|p| p.comm.as_str()).collect();
        assert_eq!(
            names,
            ["burst", "old"],
            "idle and the newcomer are left out"
        );
        assert!((top[0].cpu_pct - 90.0).abs() < 1.0);
    }
}
