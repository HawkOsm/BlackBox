use crate::database;
use std::collections::HashMap;

#[path = "netlink.rs"]
mod netlink;
#[path = "socket.rs"]
mod socket;

const TAIL_SECS: i64 = 120;

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

fn event_handler(socket: i32) {
    let mut buffer = vec![0u8; 4096];
    let mut window_until: i64 = 0;
    let mut names = scan_proc_names();
    loop {
        let bytes_read = socket::read_message(socket, &mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }

        let event = unsafe { std::ptr::read_unaligned(buffer[36..].as_ptr() as *const exit_event) };
        if event.process_pid != event.process_tgid {
            continue;
        }
        if event.what == libc::PROC_EVENT_EXEC {
            if let Some(name) = read_comm(event.process_tgid) {
                names.insert(event.process_tgid, name);
            }
            continue;
        }
        if event.what != libc::PROC_EVENT_EXIT {
            continue;
        }
        let cached = names.remove(&event.process_tgid);

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let is_crash = matches!(event.exit_code & 0x7f, 4 | 6 | 7 | 8 | 9 | 11);
        if is_crash {
            window_until = ts + TAIL_SECS;
        }
        if !is_crash && ts > window_until {
            continue;
        }

        let comm = cached.or_else(|| read_comm(event.process_pid));
        let summary = format!(
            "{} (pid {}) {}",
            comm.as_deref().unwrap_or("?"),
            event.process_pid,
            describe_exit(event.exit_code)
        );
        database::add_custom_event(
            ts,
            event.process_pid as i32,
            Some(event.parent_pid as i32),
            event.exit_code as i32,
            comm.as_deref(),
            if is_crash { "error" } else { "info" },
            &summary,
        );
    }
}

fn scan_proc_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let pid = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok());
            if let Some(pid) = pid {
                if let Some(name) = read_comm(pid) {
                    names.insert(pid, name);
                }
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

fn signal_name(sig: u32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "an unnamed signal",
    }
}

fn describe_exit(exit_code: u32) -> String {
    let sig = exit_code & 0x7f;
    if sig != 0 {
        let core = if exit_code & 0x80 != 0 { ", core dumped" } else { "" };
        format!("killed by {} ({}){}", signal_name(sig), sig, core)
    } else {
        format!("exited with status {}", exit_code >> 8)
    }
}

pub fn build_collector() {
    let socket = socket_handler();
    netlink_handler(socket);
    event_handler(socket);
}
