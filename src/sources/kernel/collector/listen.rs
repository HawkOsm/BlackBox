//! The event loop: wait for the kernel, let a burst pile up, turn each exit worth keeping into a
//! record.

use super::event::{Event, decode};
use super::exit;
use super::names::{read_comm, remember, scan_proc_names};
use super::socket;
use super::store::Exit;
use crate::database;
use std::collections::HashMap;

fn handle_bytes(names: &mut HashMap<u32, String>, bytes: &[u8]) {
    match decode(bytes) {
        Some(Event::Exec { pid }) => remember(names, pid),
        Some(Event::Exit { pid, ppid, code }) => {
            // taken out whether or not the exit is stored, so the table only holds the living
            let known = names.remove(&pid);
            if let Some(exit) = exit_record(pid, ppid, code, known) {
                database::write(exit);
            }
        }
        None => {}
    }
}

/// The record for an exit worth storing (see `exit.rs`), named as well as it still can be.
fn exit_record(pid: u32, ppid: u32, code: u32, known: Option<String>) -> Option<Exit> {
    // mirrors the kernel-side filter, which may be missing on an old kernel
    let severity = exit::severity(code)?;
    let comm = known.or_else(|| read_comm(pid));
    // Very short-lived processes are gone before their name can be read; the parent usually is not.
    // A crash that dumped core is named later from systemd-coredump's journal entry.
    let who = match &comm {
        Some(name) => format!("{name} (pid {pid})"),
        None => match read_comm(ppid) {
            Some(parent) => format!("? (pid {pid}, child of {parent})"),
            None => format!("? (pid {pid})"),
        },
    };
    Some(Exit {
        ts: database::now(),
        pid: pid as i32,
        ppid: Some(ppid as i32),
        exit_code: code as i32,
        comm,
        severity,
        summary: format!("{who} {}", exit::describe(code)),
    })
}

/// The kernel had more events than its queue holds and dropped some.
fn overflowed(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ENOBUFS)
}

const OVERFLOW: &str = "kernel event buffer overflowed; some process events were dropped";

pub(super) fn event_handler(socket: i32) {
    let batch = std::time::Duration::from_millis(
        std::env::var("BLACKBOX_BATCH_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100),
    );
    let mut buffer = vec![0u8; 4096];
    let mut names = scan_proc_names();
    loop {
        // Sleep in the kernel until something arrives...
        match socket::read_message(socket, &mut buffer) {
            Ok(0) => break,
            Ok(n) => handle_bytes(&mut names, &buffer[..n]),
            Err(e) if overflowed(&e) => eprintln!("{OVERFLOW}"),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("netlink read failed: {e}");
                database::drain();
                std::process::exit(1);
            }
        }
        // ...then let a burst pile up and drain it in one go, so an active system
        // costs a handful of wakeups per second instead of one per event.
        std::thread::sleep(batch);
        loop {
            match socket::read_message_nowait(socket, &mut buffer) {
                Ok(0) => break,
                Ok(n) => handle_bytes(&mut names, &buffer[..n]),
                Err(e) if overflowed(&e) => eprintln!("{OVERFLOW}"),
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Severity;

    #[test]
    fn a_crash_becomes_a_record_and_a_clean_exit_does_not() {
        let crash = exit_record(4_000_000, 1, 139, Some("bash".to_string())).unwrap();
        assert_eq!(crash.severity, Severity::Error);
        assert_eq!(crash.comm.as_deref(), Some("bash"));
        assert_eq!(
            crash.summary,
            "bash (pid 4000000) killed by SIGSEGV (11), core dumped"
        );
        assert!(exit_record(4_000_000, 1, 0, Some("bash".to_string())).is_none());
    }
}
