use crate::database::{self, Sample};
use std::time::{Duration, Instant};

const INTERVAL_SECS: u64 = 10;

struct CpuTimes {
    busy: u64,
    total: u64,
}

fn read_cpu() -> Option<CpuTimes> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if fields.len() < 8 {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest is already inside user)
    let total: u64 = fields.iter().take(8).sum();
    let idle = fields[3] + fields[4];
    Some(CpuTimes {
        busy: total - idle,
        total,
    })
}

fn read_mem_pct() -> Option<f64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<f64> {
        info.lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    Some(100.0 * (1.0 - field("MemAvailable:")? / field("MemTotal:")?))
}

fn read_load() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_disk_sectors() -> Option<u64> {
    let stats = std::fs::read_to_string("/proc/diskstats").ok()?;
    let mut sectors = 0;
    for line in stats.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 {
            continue;
        }
        let name = f[2];
        let virtual_dev = ["loop", "ram", "zram", "dm-"]
            .iter()
            .any(|p| name.starts_with(p));
        // whole disks live directly in /sys/block; partitions sit one level deeper
        if virtual_dev || !std::path::Path::new(&format!("/sys/block/{name}")).exists() {
            continue;
        }
        sectors += f[5].parse::<u64>().unwrap_or(0) + f[9].parse::<u64>().unwrap_or(0);
    }
    Some(sectors)
}

fn nvidia_asleep() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return false;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let read = |name: &str| std::fs::read_to_string(path.join(name)).unwrap_or_default();
        if read("vendor").trim() == "0x10de" && read("class").starts_with("0x03") {
            return read("power/runtime_status").trim() == "suspended";
        }
    }
    false
}

/// (gpu %, vram %, temp C) from nvidia-smi; None when absent, failing, or the card is asleep.
fn read_gpu() -> Option<(f64, f64, f64)> {
    if nvidia_asleep() {
        return None;
    }
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let v: Vec<f64> = text
        .lines()
        .next()?
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    if v.len() != 4 || v[2] <= 0.0 {
        return None;
    }
    Some((v[0], 100.0 * v[1] / v[2], v[3]))
}

fn assess(s: &Sample, cores: f64) -> Option<(&'static str, String)> {
    let mut problems: Vec<String> = Vec::new();
    let mut error = false;
    if s.mem_pct >= 95.0 {
        error = true;
        problems.push(format!("memory {:.0}% used", s.mem_pct));
    } else if s.mem_pct >= 85.0 {
        problems.push(format!("memory {:.0}% used", s.mem_pct));
    }
    if s.cpu_pct >= 90.0 {
        problems.push(format!("cpu {:.0}% busy", s.cpu_pct));
    }
    if s.load_avg > cores * 2.0 {
        problems.push(format!("load {:.1} on {:.0} cores", s.load_avg, cores));
    }
    if let Some(t) = s.gpu_temp.filter(|t| *t >= 85.0) {
        problems.push(format!("gpu {t:.0}C"));
    }
    if let Some(m) = s.gpu_mem_pct.filter(|m| *m >= 90.0) {
        problems.push(format!("gpu memory {m:.0}% used"));
    }
    if problems.is_empty() {
        return None;
    }
    Some((if error { "error" } else { "warning" }, problems.join(", ")))
}

pub fn start() {
    std::thread::spawn(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get() as f64)
            .unwrap_or(1.0);
        let mut prev = read_cpu().zip(read_disk_sectors());
        let mut last = Instant::now();
        let mut tick: u64 = 0;
        loop {
            std::thread::sleep(Duration::from_secs(INTERVAL_SECS));
            tick += 1;
            let secs = last.elapsed().as_secs_f64();
            last = Instant::now();
            let now = read_cpu().zip(read_disk_sectors());

            if let (Some((c0, d0)), Some((c1, d1))) = (&prev, &now) {
                let total = c1.total.saturating_sub(c0.total);
                if let (true, Some(mem), Some(load)) = (total > 0, read_mem_pct(), read_load()) {
                    let cpu = 100.0 * c1.busy.saturating_sub(c0.busy) as f64 / total as f64;
                    // a sector is 512 bytes, so sectors / 2 = KB
                    let disk_kb = d1.saturating_sub(*d0) as f64 / 2.0 / secs;
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    // nvidia-smi costs ~13 ms of CPU per call, so ask every third sample (30 s)
                    let gpu = if tick % 3 == 1 { read_gpu() } else { None };
                    let sample = Sample {
                        cpu_pct: cpu,
                        mem_pct: mem,
                        disk_io_kb: disk_kb,
                        load_avg: load,
                        gpu_pct: gpu.map(|g| g.0),
                        gpu_mem_pct: gpu.map(|g| g.1),
                        gpu_temp: gpu.map(|g| g.2),
                    };
                    let alert = assess(&sample, cores);
                    database::add_sysstat_event(
                        ts,
                        &sample,
                        alert.as_ref().map(|(sev, text)| (*sev, text.as_str())),
                    );
                }
            }
            prev = now;
        }
    });
}
