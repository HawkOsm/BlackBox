//! The CPU's clock: the fastest core at this instant, which shows when the CPU is boosting.

const CPUS: &str = "/sys/devices/system/cpu";

/// The highest current frequency of any core, in MHz; None without cpufreq (some VMs).
pub(crate) fn read_max_mhz() -> Option<f64> {
    let mut best: Option<u64> = None;
    for entry in std::fs::read_dir(CPUS).ok()?.flatten() {
        let name = entry.file_name();
        let is_core = name
            .to_string_lossy()
            .strip_prefix("cpu")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !is_core {
            continue; // cpufreq/, cpuidle/, …
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("cpufreq/scaling_cur_freq"))
        else {
            continue; // an offline core
        };
        if let Ok(khz) = text.trim().parse::<u64>() {
            best = best.max(Some(khz));
        }
    }
    best.map(|khz| khz as f64 / 1000.0)
}
