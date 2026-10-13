//! Process exits, pushed by the kernel over the netlink process connector.
//!
//! - `subscribe.rs`: open the socket, attach the kernel-side filter, check the kernel said yes.
//! - `event.rs`: the kernel's event layout, and `decode`.
//! - `listen.rs`: the event loop, and `exit_record`: what an exit becomes.
//! - `names.rs`: process names, read while the processes are alive.
//! - `exit.rs`: which exits are worth keeping, and how to describe one.
//! - `socket.rs`, `netlink.rs`: the raw socket and the kernel-side (BPF) filter.
//! - `store.rs`: `Exit`, `CrashName` and their SQL.

mod event;
mod exit;
mod listen;
mod names;
mod netlink;
mod socket;
mod store;
mod subscribe;

pub use store::CrashName;

/// Subscribes to the kernel's process events, and returns the socket they arrive on. If the
/// kernel refuses, the process exits here.
pub fn subscribe() -> i32 {
    let socket = subscribe::socket_handler();
    subscribe::netlink_handler(socket);
    socket
}

/// Stores the process events as they arrive; returns only if the socket stops.
pub fn listen(socket: i32) {
    listen::event_handler(socket);
}
