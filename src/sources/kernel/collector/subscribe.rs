//! Opening the netlink socket and subscribing to the kernel's process events.

use super::event::HEADERS;
use super::{netlink, socket};

pub(super) fn socket_handler() -> i32 {
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
    sock
}

pub(super) fn netlink_handler(socket: i32) {
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
    confirm_subscription(socket);
    // only now: the filter would drop the kernel's reply above
    if let Err(e) = socket::attach_event_filter(socket) {
        eprintln!("kernel event filter unavailable, filtering in userspace instead: {e}");
    }
}

/// Where the error code sits in the kernel's answer to a subscription: after the headers, the
/// event kind (4 bytes), the cpu (4) and the timestamp (8).
const ACK_ERR: usize = HEADERS + 16;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The kernel answers a subscription with an event of kind "none" carrying an error code. A
/// refusal would otherwise look exactly like a quiet machine: no crashes, ever.
fn confirm_subscription(socket: i32) {
    let mut buffer = vec![0u8; 4096];
    socket::set_recv_timeout(socket, Some(std::time::Duration::from_secs(2)));
    let answer = (0..64).find_map(|_| match socket::read_message(socket, &mut buffer) {
        // the reply is event kind 0 ("none"); its error code follows what, cpu and timestamp
        Ok(n) if n >= ACK_ERR + 4 && u32_at(&buffer, HEADERS) == 0 => {
            Some(Ok(u32_at(&buffer, ACK_ERR)))
        }
        Ok(_) => None, // an event that raced ahead of the reply
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => None,
        Err(e) => Some(Err(e)),
    });
    socket::set_recv_timeout(socket, None);
    match answer {
        Some(Ok(0)) => println!("subscribed to kernel process events"),
        Some(Ok(err)) => eprintln!(
            "the kernel refused the process-event subscription: {}; crashes will not be recorded",
            std::io::Error::from_raw_os_error(err as i32)
        ),
        Some(Err(e)) => eprintln!(
            "no answer to the process-event subscription ({e}); crashes may not be recorded"
        ),
        None => {
            eprintln!("no answer to the process-event subscription; crashes may not be recorded")
        }
    }
}
