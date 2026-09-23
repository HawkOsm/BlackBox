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

pub fn read_message_nowait(sock: i32, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
    let ret = unsafe {
        libc::recv(
            sock,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len(),
            libc::MSG_DONTWAIT,
        )
    };
    check_ret(ret as i64).map(|_| ret as usize)
}

/// Kernel-side filter so only exec events (for process names) and exits worth recording ever
/// wake us: a whole-process exit that is a crash or a failure, as `exit::severity` defines it.
///
/// Offsets count from the start of the netlink header: event kind at 36, pid at 52, tgid at 56,
/// then the wait status at 60 (low byte: signal and core flag, next byte: exit status). Classic
/// BPF word loads are big-endian, so the little-endian event ids below appear byte-swapped; the
/// wait status is read a byte at a time, which sidesteps that.
pub fn event_filter() -> Vec<libc::sock_filter> {
    const EXEC_SWAPPED: u32 = 0x0200_0000;
    const EXIT_SWAPPED: u32 = 0x0000_0080;
    const LD_W: u16 = 0x20; // A = 32-bit word at k
    const LD_B: u16 = 0x30; // A = byte at k
    const ST: u16 = 0x02; // M[k] = A
    const LDX_M: u16 = 0x61; // X = M[k]
    const AND: u16 = 0x54; // A &= k
    const JEQ: u16 = 0x15; // A == k
    const JEQ_X: u16 = 0x1d; // A == X
    const JGT: u16 = 0x25; // A > k
    const RET: u16 = 0x06;
    let insn = |code: u16, jt: u8, jf: u8, k: u32| libc::sock_filter { code, jt, jf, k };

    let signals = crate::exit::CRASH_SIGNALS;
    let routine = crate::exit::ROUTINE_STATUSES;
    let first_signal = 11;
    let status = first_signal + signals.len();
    let accept = status + 2 + routine.len();
    let reject = accept + 1;
    // jump distance from instruction `from` to `to`
    let to = |from: usize, to: usize| (to - from - 1) as u8;

    let mut program = vec![
        insn(LD_W, 0, 0, 36),                      // 0: event kind
        insn(JEQ, to(1, accept), 0, EXEC_SWAPPED), // 1: exec -> accept
        insn(JEQ, 0, to(2, reject), EXIT_SWAPPED), // 2: anything but exit -> reject
        insn(LD_W, 0, 0, 52),                      // 3: pid
        insn(ST, 0, 0, 0),                         // 4
        insn(LD_W, 0, 0, 56),                      // 5: tgid
        insn(LDX_M, 0, 0, 0),                      // 6
        insn(JEQ_X, 0, to(7, reject), 0),          // 7: a thread, not the process -> reject
        insn(LD_B, 0, 0, 60),                      // 8: signal byte
        insn(AND, 0, 0, 0x7f),                     // 9: drop the core-dump flag
        insn(JEQ, to(10, status), 0, 0),           // 10: no signal -> look at the status
    ];
    for (i, sig) in signals.iter().enumerate() {
        let at = first_signal + i;
        let miss = if i + 1 == signals.len() {
            to(at, reject)
        } else {
            0
        };
        program.push(insn(JEQ, to(at, accept), miss, *sig)); // crash signal -> accept
    }
    program.push(insn(LD_B, 0, 0, 61)); // exit status
    program.push(insn(JGT, 0, to(status + 1, reject), 1)); // 0 and 1 are routine
    for (i, code) in routine.iter().enumerate() {
        let at = status + 2 + i;
        program.push(insn(JEQ, to(at, reject), 0, *code)); // a shell reporting a routine signal
    }
    program.push(insn(RET, 0, 0, 0xffff));
    program.push(insn(RET, 0, 0, 0));
    program
}

pub fn attach_event_filter(sock: i32) -> Result<(), std::io::Error> {
    let mut program = event_filter();
    let fprog = libc::sock_fprog {
        len: program.len() as u16,
        filter: program.as_mut_ptr(),
    };
    let ret = unsafe {
        libc::setsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            &fprog as *const libc::sock_fprog as *const libc::c_void,
            mem::size_of::<libc::sock_fprog>() as libc::socklen_t,
        )
    };
    check_ret(ret as i64)
}

/// A read timeout for blocking reads; `None` blocks forever again.
pub fn set_recv_timeout(sock: i32, timeout: Option<std::time::Duration>) {
    let t = timeout.unwrap_or_default();
    let tv = libc::timeval {
        tv_sec: t.as_secs() as libc::time_t,
        tv_usec: t.subsec_micros() as libc::suseconds_t,
    };
    unsafe {
        libc::setsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const libc::timeval as *const libc::c_void,
            mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// Best effort: the kernel clamps this to net.core.rmem_max.
pub fn set_recv_buffer(sock: i32, bytes: i32) {
    unsafe {
        libc::setsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &bytes as *const i32 as *const libc::c_void,
            mem::size_of::<i32>() as libc::socklen_t,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Just enough of a classic BPF interpreter to run `event_filter`, so the filter is proven
    /// to agree with `exit::severity` for every possible wait status.
    fn run(program: &[libc::sock_filter], packet: &[u8]) -> u32 {
        let (mut a, mut x, mut mem, mut pc) = (0u32, 0u32, [0u32; 16], 0usize);
        loop {
            let i = &program[pc];
            let k = i.k as usize;
            pc += 1;
            let jump = |cond: bool| if cond { i.jt as usize } else { i.jf as usize };
            match i.code {
                0x20 => a = u32::from_be_bytes(packet[k..k + 4].try_into().unwrap()),
                0x30 => a = packet[k] as u32,
                0x02 => mem[k] = a,
                0x61 => x = mem[k],
                0x54 => a &= i.k,
                0x15 => pc += jump(a == i.k),
                0x1d => pc += jump(a == x),
                0x25 => pc += jump(a > i.k),
                0x06 => return i.k,
                other => panic!("opcode {other:#x} not handled"),
            }
        }
    }

    fn event(what: u32, pid: u32, tgid: u32, exit_code: u32) -> Vec<u8> {
        let mut p = vec![0u8; 36];
        p.extend(what.to_ne_bytes());
        p.extend([0u8; 12]); // cpu, timestamp
        p.extend(pid.to_ne_bytes());
        p.extend(tgid.to_ne_bytes());
        p.extend(exit_code.to_ne_bytes());
        p.extend([0u8; 12]);
        p
    }

    #[test]
    fn filter_agrees_with_exit_severity() {
        let program = event_filter();
        let exit = libc::PROC_EVENT_EXIT;
        for code in 0..=0xffff_u32 {
            let kept = run(&program, &event(exit, 7, 7, code)) != 0;
            assert_eq!(
                kept,
                crate::exit::severity(code).is_some(),
                "wait status {code:#x}"
            );
        }
        assert_ne!(
            run(&program, &event(libc::PROC_EVENT_EXEC, 7, 7, 0)),
            0,
            "exec is kept for names"
        );
        assert_eq!(
            run(&program, &event(exit, 8, 7, 11)),
            0,
            "a thread exit is dropped"
        );
        assert_eq!(
            run(&program, &event(libc::PROC_EVENT_FORK, 7, 7, 11)),
            0,
            "fork is dropped"
        );
    }

    #[test]
    fn test_socket() {
        let sock = open_socket().unwrap();
        bind_proc_connector(sock).unwrap();
        let msg = vec![0u8; 10]; // Example message
        let send_result = send_listen_message(&msg, sock);
        assert!(send_result.is_ok());
    }
}
