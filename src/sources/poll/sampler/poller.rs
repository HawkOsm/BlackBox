use super::alert::assess;
use super::gpu::read_gpu;
use super::power::Power;
use super::proc::{CpuTimes, read_cpu, read_disk_sectors, read_load, read_mem_pct, read_top_procs};
use super::sensors::Sensors;
use super::store::{Sample, Tick};
use crate::database;
use crate::sources::poll::Poller;
use std::time::{Duration, Instant};

const INTERVAL_SECS: u64 = 10;

pub(super) struct Sampler {
    pub(super) cores: f64,
    /// The CPU and disk counters of the previous look: a sample is the difference.
    pub(super) prev: Option<(CpuTimes, u64)>,
    pub(super) last: Instant,
    /// Samples taken; the GPU and the top processes are read every third.
    pub(super) tick: u64,
    pub(super) power: Power,
    pub(super) sensors: Sensors,
}

impl Poller for Sampler {
    fn every(&self) -> Duration {
        Duration::from_secs(INTERVAL_SECS)
    }

    /// The first look only reads the counters; every later one stores a sample.
    fn poll(&mut self) {
        let secs = self.last.elapsed().as_secs_f64();
        self.last = Instant::now();
        let now = read_cpu().zip(read_disk_sectors());
        if let (Some((c0, d0)), Some((c1, d1))) = (&self.prev, &now) {
            let total = c1.total.saturating_sub(c0.total);
            if let (true, Some(mem), Some(load)) = (total > 0, read_mem_pct(), read_load()) {
                self.tick += 1;
                let cpu = 100.0 * c1.busy.saturating_sub(c0.busy) as f64 / total as f64;
                // a sector is 512 bytes, so sectors / 2 = KB
                let disk_kb = d1.saturating_sub(*d0) as f64 / 2.0 / secs;
                let ts = database::now();
                // nvidia-smi costs ~13 ms of CPU per call, so ask every third sample (30 s)
                let gpu = if self.tick % 3 == 1 { read_gpu() } else { None };
                let sample = Sample {
                    cpu_pct: cpu,
                    mem_pct: mem,
                    disk_io_kb: disk_kb,
                    load_avg: load,
                    gpu_pct: gpu.map(|g| g.0),
                    gpu_mem_pct: gpu.map(|g| g.1),
                    gpu_temp: gpu.map(|g| g.2),
                };
                database::write(Tick {
                    ts,
                    alert: assess(&sample, self.cores),
                    sample,
                    power: self.power.sample(gpu.and_then(|g| g.3)),
                    sensors: self.sensors.read(),
                    procs: if self.tick % 3 == 1 {
                        read_top_procs(5)
                    } else {
                        Vec::new()
                    },
                });
            }
        }
        self.prev = now;
    }
}
