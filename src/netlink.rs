use libc::nlmsghdr;

#[repr(C)]
pub struct CbId {
    pub(crate) idx: u32,
    pub(crate) val: u32,
}

#[repr(C)]
pub struct CnMsg {
    id: CbId,
    seq: u32,
    ack: u32,
    len: u16,
    flags: u16,
    data: [u8; 0], // Flexible array member
}

fn as_bytes<T>(val: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(val as *const T as *const u8, std::mem::size_of::<T>()) }
}

pub fn build_cn_msg(id: CbId, seq: u32, ack: u32, flags: u16, data: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(
        std::mem::size_of::<CnMsg>() + data.len() + std::mem::size_of::<nlmsghdr>(),
    );
    let cn_msg = CnMsg {
        id,
        seq,
        ack,
        len: data.len() as u16,
        flags,
        data: [],
    };
    let cn_msg_bytes = as_bytes(&cn_msg);

    let hdr = nlmsghdr {
        nlmsg_len: (std::mem::size_of::<CnMsg>() + data.len() + std::mem::size_of::<nlmsghdr>())
            as u32,
        nlmsg_type: libc::NLMSG_DONE as u16,
        nlmsg_flags: 0,
        nlmsg_seq: seq,
        nlmsg_pid: unsafe { libc::getpid() } as u32,
    };
    let hdr_bytes = as_bytes(&hdr);

    msg.extend_from_slice(hdr_bytes);
    msg.extend_from_slice(cn_msg_bytes);
    msg.extend_from_slice(data);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cn_msg() {
        let id = CbId { idx: 1, val: 2 };
        let data = [1, 2, 3, 4];
        let msg = build_cn_msg(id, 0, 0, 0, &data);
        assert_eq!(msg.len(), 40);
    }
}
