use crate::database;
use libc::sleep;

#[path = "socket.rs"]
mod socket;
#[path = "netlink.rs"]
mod netlink;
#[path = "event.rs"]
mod event;

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
    loop {
        let bytes_read = socket::read_message(socket, &mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }

        let event = unsafe { std::ptr::read_unaligned(buffer[36..].as_ptr() as *const event::proc_event) };
        let cpu = event.cpu as i32;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        match event.what {
            libc::PROC_EVENT_FORK => unsafe {
                let fork = event.event_data.fork;
                database::add_custom_event(ts, "fork", fork.child_pid as i32, Some(fork.parent_pid as i32), Some(cpu as i32));
            }
            libc::PROC_EVENT_EXEC => unsafe {
                let exec = event.event_data.exec;
                database::add_custom_event(ts, "exec", exec.process_pid as i32, None, Some(cpu as i32));
            }
            libc::PROC_EVENT_EXIT => unsafe {
                let exit = event.event_data.exit;
                database::add_custom_event(ts, "exit", exit.process_pid as i32, None, Some(cpu as i32));
            }
            _ => {}
        }
    }
}

pub fn build_collector() {
    let socket = socket_handler();
    netlink_handler(socket);
    event_handler(socket);
}