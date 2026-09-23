//! What a process exit means. Shared by the collector, its kernel-side filter and the journald
//! follower, so all three agree on which exits are worth keeping.

/// Signals that mean the process crashed: they all dump core by default. SIGTERM, SIGHUP, SIGINT
/// and SIGPIPE are routine shutdowns, and SIGKILL is mostly benign here (real OOM kills are logged
/// by the kernel to journald).
pub const CRASH_SIGNALS: [u32; 7] = [4, 5, 6, 7, 8, 11, 31];

/// A shell reports a child killed by signal N as exit status 128 + N. These are the routine
/// signals above (HUP, INT, KILL, PIPE, TERM) seen through a shell: a script stopped with Ctrl-C
/// exits 130, one stopped by `timeout` or systemd exits 143.
pub const ROUTINE_STATUSES: [u32; 5] = [129, 130, 137, 141, 143];

/// Exit status of a Rust program that panicked.
const RUST_PANIC: u32 = 101;

/// Whether an exit is recorded, and how loudly.
///
/// Status 1 is left out on purpose: it is what `grep`, `pgrep`, `modprobe` and friends return for
/// "not found", which on a desktop means hundreds of polling helpers a minute. Of 3,270 exits
/// recorded on the development machine, status 0 and 1 were 99.7%; the other 11 were signal deaths
/// and genuine failures.
pub fn severity(exit_code: u32) -> Option<&'static str> {
    let sig = exit_code & 0x7f;
    if sig != 0 {
        return CRASH_SIGNALS.contains(&sig).then_some("error");
    }
    match (exit_code >> 8) & 0xff {
        0 | 1 => None,
        s if ROUTINE_STATUSES.contains(&s) => None,
        RUST_PANIC => Some("error"),
        _ => Some("warning"),
    }
}

fn signal_name(sig: u32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        31 => "SIGSYS",
        _ => "an unnamed signal",
    }
}

pub fn describe(exit_code: u32) -> String {
    let sig = exit_code & 0x7f;
    if sig != 0 {
        let core = if exit_code & 0x80 != 0 {
            ", core dumped"
        } else {
            ""
        };
        format!("killed by {} ({}){}", signal_name(sig), sig, core)
    } else {
        let status = (exit_code >> 8) & 0xff;
        let note = if status == RUST_PANIC {
            " (Rust panic)"
        } else {
            ""
        };
        format!("exited with status {status}{note}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_crashes_and_real_failures_only() {
        assert_eq!(severity(0), None);
        assert_eq!(severity(1 << 8), None, "status 1 is routine");
        assert_eq!(severity(2 << 8), Some("warning"));
        assert_eq!(severity(124 << 8), Some("warning"));
        assert_eq!(severity(143 << 8), None, "a shell reporting SIGTERM");
        assert_eq!(severity(130 << 8), None, "a shell reporting Ctrl-C");
        assert_eq!(
            severity(139 << 8),
            Some("warning"),
            "a shell reporting SIGSEGV"
        );
        assert_eq!(severity(101 << 8), Some("error"), "a Rust panic");
        assert_eq!(severity(11), Some("error"));
        assert_eq!(severity(11 | 0x80), Some("error"), "SIGSEGV with core");
        assert_eq!(severity(31), Some("error"), "seccomp kill");
        assert_eq!(severity(9), None, "SIGKILL");
        assert_eq!(severity(15), None, "SIGTERM");
    }

    #[test]
    fn describes_exits() {
        assert_eq!(describe(139), "killed by SIGSEGV (11), core dumped");
        assert_eq!(describe(101 << 8), "exited with status 101 (Rust panic)");
        assert_eq!(describe(2 << 8), "exited with status 2");
    }
}
