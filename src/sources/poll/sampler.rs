//! The machine's load and health every 10 s: CPU, memory, disk I/O, load, the GPU, power and every
//! hwmon sensor, as one `Tick`.
//!
//! - `poller.rs`: the `Sampler`, one look every 10 s.
//! - `proc.rs`: the counters in `/proc`, and the processes using the most memory.
//! - `gpu.rs`: `nvidia-smi`, only while the card is awake.
//! - `power.rs`: CPU watts (RAPL) and the battery.
//! - `sensors.rs`: every temperature, fan and power reading in hwmon and RAPL.
//! - `alert.rs`: when a sample is worth an alert.
//! - `store.rs`: `Tick` and its SQL.

mod alert;
mod gpu;
mod poller;
mod power;
mod proc;
mod sensors;
mod store;

use super::poll;
use crate::sources::supervise;
use poller::Sampler;
use power::Power;
use sensors::Sensors;
use std::time::Instant;

pub fn start() {
    let mut sampler = Sampler {
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as f64)
            .unwrap_or(1.0),
        prev: None,
        last: Instant::now(),
        tick: 0,
        power: Power::new(),
        sensors: Sensors::new(),
    };
    supervise("sampler", move || poll(&mut sampler));
}
