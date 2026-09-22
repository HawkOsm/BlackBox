use libc::sleep;

mod netlink;
mod socket;
mod event;

fn main() {
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

    let id = netlink::CbId {
        idx: libc::CN_IDX_PROC,
        val: libc::CN_VAL_PROC,
    };
    let payload = (libc::PROC_CN_MCAST_LISTEN).to_ne_bytes();
    let msg = netlink::build_cn_msg(id, 0, 0, 0, &payload);

    let send = socket::send_listen_message(&msg, sock);
    if let Err(e) = send {
        eprintln!("Failed to send listen message: {e}");
        std::process::exit(1);
    }
    println!("Listening for process events... {msg:?}");

    let mut buffer = vec![0u8; 4096];
    loop {
        let bytes_read = socket::read_message(sock, &mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        let event = unsafe { std::ptr::read(buffer.as_ptr() as *const event::proc_event) };
        println!("Received event: what={}, cpu={}, timestamp={}", event.what, event.cpu, event.timestamp);
        match event.what {
            libc::PROC_EVENT_FORK => unsafe {
                let fork = event.event_data.fork;
                println!("Fork event: parent_pid={}, parent_tgid={}, child_pid={}, child_tgid={}",
                    fork.parent_pid, fork.parent_tgid, fork.child_pid, fork.child_tgid);
            }
            libc::PROC_EVENT_EXEC => unsafe {
                let exec = event.event_data.exec;
                println!("Exec event: process_pid={}, process_tgid={}",
                    exec.process_pid, exec.process_tgid);
            }
            libc::PROC_EVENT_EXIT => unsafe {
                let exit = event.event_data.exit;
                println!("Exit event: process_pid={}, process_tgid={}, exit_code={}, exit_signal={}",
                    exit.process_pid, exit.process_tgid, exit.exit_code, exit.exit_signal);
            }
            _ => println!("Unknown process event: what={}", event.what),
        }
        println!("Received message: {:?}", &buffer[..bytes_read]);
        unsafe { sleep(1); }
    }
}
