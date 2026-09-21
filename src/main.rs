use libc::sleep;

mod netlink;
mod socket;

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
        println!("Received message: {:?}", &buffer[..bytes_read]);
        unsafe { sleep(1); }
    }
}
