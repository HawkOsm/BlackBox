use std::mem;

fn check_ret(ret: i64) -> Result<(), std::io::Error> {
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn open_socket() -> Result<i32, std::io::Error> {
    let sock = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM, libc::NETLINK_CONNECTOR) };
    check_ret(sock as i64)?;
    Ok(sock)
}

pub fn bind_proc_connector(sock: i32) -> Result<(), std::io::Error> {
    let mut addr: libc::sockaddr_nl = unsafe { mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    addr.nl_pid = 0; // Let the kernel assign the PID
    addr.nl_groups = libc::CN_IDX_PROC;

    let ret = unsafe {
        libc::bind(
            sock,
            &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };

    check_ret(ret as i64)
}

pub fn send_listen_message(msg: &[u8], sock: i32) -> Result<(), std::io::Error> {
    let call = unsafe { libc::write(sock, msg.as_ptr() as *const libc::c_void, msg.len()) };
    check_ret(call as i64)
}

pub fn read_message(sock: i32, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
    let ret = unsafe { libc::read(sock, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) };
    check_ret(ret as i64).map(|_| ret as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket() {
        let sock = open_socket().unwrap();
        bind_proc_connector(sock).unwrap();
        let msg = vec![0u8; 10]; // Example message
        let send_result = send_listen_message(&msg, sock);
        assert!(send_result.is_ok());
    }
}
