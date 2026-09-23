use crate::{database, exit};
use std::collections::HashMap;

#[path = "netlink.rs"]
mod netlink;
#[path = "socket.rs"]
mod socket;

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

fn socket_handler() -> i32 {
    let sock = match socket::open_socket() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to open netlink socket: {e}");
            std::process::exit(1);
        }
    };

    let bind = socket::bind_proc_connector(sock);
    if let Err(e) = bind {
        eprintln!("Failed to bind to proc connector: {e}");
        std::process::exit(1);
    }
    socket::set_recv_buffer(sock, 1 << 20);
    if let Err(e) = socket::attach_event_filter(sock) {
        eprintln!("kernel event filter unavailable, filtering in userspace instead: {e}");
    }
    sock
}

fn netlink_handler(socket: i32) {
    let id = netlink::CbId {
        idx: libc::CN_IDX_PROC,
        val: libc::CN_VAL_PROC,
    };
    let payload = (libc::PROC_CN_MCAST_LISTEN).to_ne_bytes();
    let msg = netlink::build_cn_msg(id, 0, 0, 0, &payload);

    let send = socket::send_listen_message(&msg, socket);
    if let Err(e) = send {
        eprintln!("Failed to send listen message: {e}");
        std::process::exit(1);
    }
    println!("Listening for process events... {msg:?}");
}

fn handle_bytes(names: &mut HashMap<u32, String>, bytes: &[u8]) {
    if bytes.len() < 36 + std::mem::size_of::<exit_event>() {
        return;
    }
    let event = unsafe { std::ptr::read_unaligned(bytes[36..].as_ptr() as *const exit_event) };
    if event.process_pid != event.process_tgid {
        return;
    }
    if event.what == libc::PROC_EVENT_EXEC {
        if let Some(name) = read_comm(event.process_tgid) {
            if names.len() > 50_000 {
                // exits lost to a queue overflow leave stale entries: start over from what is alive
                *names = scan_proc_names();
            }
            names.insert(event.process_tgid, name);
        }
        return;
    }
    if event.what != libc::PROC_EVENT_EXIT {
        return;
    }
    let cached = names.remove(&event.process_tgid);
    // mirrors the kernel-side filter, which may be missing on an old kernel
    let Some(severity) = exit::severity(event.exit_code) else {
        return;
    };

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let comm = cached.or_else(|| read_comm(event.process_pid));
    // Very short-lived processes are gone before their name can be read; the parent usually is not.
    // A crash that dumped core is named later from systemd-coredump's journal entry.
    let who = match &comm {
        Some(name) => format!("{name} (pid {})", event.process_pid),
        None => match read_comm(event.parent_pid) {
            Some(parent) => format!("? (pid {}, child of {parent})", event.process_pid),
            None => format!("? (pid {})", event.process_pid),
        },
    };
    let summary = format!("{who} {}", exit::describe(event.exit_code));
    database::add_custom_event(
        ts,
        event.process_pid as i32,
        Some(event.parent_pid as i32),
        event.exit_code as i32,
        comm.as_deref(),
        severity,
        &summary,
    );
}

fn event_handler(socket: i32) {
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
            Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => {
                eprintln!("kernel event buffer overflowed; some process events were dropped");
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("netlink read failed: {e}");
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
                Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => {
                    eprintln!("kernel event buffer overflowed; some process events were dropped");
                }
                Err(_) => break,
            }
        }
    }
}

fn scan_proc_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let pid = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok());
            if let Some(pid) = pid
                && let Some(name) = read_comm(pid)
            {
                names.insert(pid, name);
            }
        }
    }
    names
}

fn read_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn build_collector() {
    let socket = socket_handler();
    netlink_handler(socket);
    event_handler(socket);
}
