//! The system's own counters in `/proc`: CPU time, memory, load, disk I/O, and the processes using
//! the most memory.

use super::store::ProcSample;

pub(super) struct CpuTimes {
    pub(super) busy: u64,
    pub(super) total: u64,
}

pub(super) fn read_cpu() -> Option<CpuTimes> {
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

pub(super) fn read_mem_pct() -> Option<f64> {
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

pub(super) fn read_load() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub(super) fn read_disk_sectors() -> Option<u64> {
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

/// The number after `name` in a "/proc" key-value file, like "VmRSS:   1234 kB".
fn kb_field(text: &str, name: &str) -> Option<i64> {
    text.lines()
        .find(|l| l.starts_with(name))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// The `n` processes using the most memory. RSS ranks them all (cheap); the accurate
/// PSS is read only for the winners, because smaps_rollup is slow to read.
pub(super) fn read_top_procs(n: usize) -> Vec<ProcSample> {
    let mut all: Vec<ProcSample> = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return all;
    };
    for entry in dir.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue;
        };
        let Some(rss_kb) = kb_field(&status, "VmRSS:") else {
            continue; // kernel threads have no VmRSS
        };

        let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
            continue;
        };
        let comm = comm.trim();

        all.push(ProcSample {
            pid,
            comm: comm.to_string(),
            rss_kb,
            pss_kb: None,
        });
    }
    all.sort_by_key(|p| std::cmp::Reverse(p.rss_kb));
    all.truncate(n);
    for proc in &mut all {
        let Ok(smaps) = std::fs::read_to_string(format!("/proc/{}/smaps_rollup", proc.pid)) else {
            continue;
        };
        proc.pss_kb = kb_field(&smaps, "Pss:");
    }
    all
}
