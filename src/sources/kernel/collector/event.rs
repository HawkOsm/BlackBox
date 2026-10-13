//! The kernel's process event, as it arrives over netlink.

// Mirrors the kernel's exit event layout; not every field is read.
#[allow(dead_code)]
#[repr(C)]
pub struct exit_event {
    pub what: u32,
    pub cpu: u32,
    pub timestamp: u64,
    pub process_pid: u32,
    pub process_tgid: u32,
    pub exit_code: u32,
    pub exit_signal: u32,
    pub parent_pid: u32,
    pub parent_tgid: u32,
}

/// What one netlink message from the process connector says, for the two kinds the recorder uses.
#[derive(Debug, PartialEq)]
pub(super) enum Event {
    /// A process started a new program: its name is worth reading now, while it is alive.
    Exec { pid: u32 },
    /// A process ended, with its raw wait status.
    Exit { pid: u32, ppid: u32, code: u32 },
}

/// The netlink and connector headers before the event itself.
pub(super) const HEADERS: usize = 36;

/// Reads the kernel's event out of one netlink message. `None` for anything else: a thread's event
/// (only whole processes are recorded), a fork, a message too short to hold an event.
pub(super) fn decode(bytes: &[u8]) -> Option<Event> {
    if bytes.len() < HEADERS + std::mem::size_of::<exit_event>() {
        return None;
    }
    // SAFETY: the length is checked above, and read_unaligned copies, so alignment does not matter
    let e = unsafe { std::ptr::read_unaligned(bytes[HEADERS..].as_ptr() as *const exit_event) };
    if e.process_pid != e.process_tgid {
        return None;
    }
    match e.what {
        libc::PROC_EVENT_EXEC => Some(Event::Exec {
            pid: e.process_tgid,
        }),
        libc::PROC_EVENT_EXIT => Some(Event::Exit {
            pid: e.process_pid,
            ppid: e.parent_pid,
            code: e.exit_code,
        }),
        _ => None,
    }
}

/// One netlink message as the kernel sends it, for tests: the headers, then the event.
#[cfg(test)]
pub(crate) fn raw_event(what: u32, pid: u32, tgid: u32, exit_code: u32, ppid: u32) -> Vec<u8> {
    let mut p = vec![0u8; HEADERS];
    p.extend(what.to_ne_bytes());
    p.extend([0u8; 12]); // cpu, timestamp
    p.extend(pid.to_ne_bytes());
    p.extend(tgid.to_ne_bytes());
    p.extend(exit_code.to_ne_bytes());
    p.extend([0u8; 4]); // exit_signal
    p.extend(ppid.to_ne_bytes());
    p.extend(ppid.to_ne_bytes()); // parent_tgid
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_execs_and_exits_of_whole_processes() {
        assert_eq!(
            decode(&raw_event(libc::PROC_EVENT_EXEC, 42, 42, 0, 1)),
            Some(Event::Exec { pid: 42 })
        );
        assert_eq!(
            decode(&raw_event(libc::PROC_EVENT_EXIT, 42, 42, 139, 7)),
            Some(Event::Exit {
                pid: 42,
                ppid: 7,
                code: 139
            })
        );
        // a thread of process 42 ending is not a process ending
        assert_eq!(
            decode(&raw_event(libc::PROC_EVENT_EXIT, 43, 42, 139, 7)),
            None
        );
        assert_eq!(
            decode(&raw_event(libc::PROC_EVENT_FORK, 42, 42, 0, 1)),
            None
        );
        assert_eq!(decode(&[0u8; 20]), None, "too short to hold an event");
    }
}
