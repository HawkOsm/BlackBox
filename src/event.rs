

#[repr(C)]
pub struct proc_event {
    pub what: u32,
    pub cpu: u32,
    pub timestamp: u64,
    pub event_data: proc_event_data,
}

#[repr(C)]
pub union proc_event_data {
    pub fork: proc_event_fork,
    pub exec: proc_event_exec,
    pub exit: proc_event_exit,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct proc_event_fork {
    pub parent_pid: u32,
    pub parent_tgid: u32,
    pub child_pid: u32,
    pub child_tgid: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct proc_event_exec {
    pub process_pid: u32,
    pub process_tgid: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct proc_event_exit {
    pub process_pid: u32,
    pub process_tgid: u32,
    pub exit_code: u32,
    pub exit_signal: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proc_event() {
        let event = proc_event {
            what: 1,
            cpu: 0,
            timestamp: 1234567890,
            event_data: proc_event_data {
                fork: proc_event_fork {
                    parent_pid: 1,
                    parent_tgid: 2,
                    child_pid: 3,
                    child_tgid: 4,
                },
            },
        };
        assert!(matches!(event.event_data, proc_event_data { fork: _ }));
    }
}