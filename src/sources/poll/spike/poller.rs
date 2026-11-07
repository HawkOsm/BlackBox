use super::context::{gpu_procs, on_ac, spotify};
use super::procs::{add_switch_rates, busiest, read_all};
use super::store::Spike;
use super::trigger::Trigger;
use crate::database;
use crate::sources::poll::Poller;
use crate::sources::poll::profiles::{read_ppd, read_tccd};
use crate::sources::poll::sampler::{read_max_mhz, watts};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) const HWMON: &str = "/sys/class/hwmon";
const RAPL: &str = "/sys/class/powercap/intel-rapl:0";
/// How long the processes are watched after a trigger.
const MEASURE: Duration = Duration::from_secs(1);
/// How long the busiest are then watched for their context switches.
const SWITCHES: Duration = Duration::from_millis(500);

pub(super) struct SpikeWatch {
    /// The package temperature's `temp*_input`; None on a CPU we cannot find one for.
    package: Option<PathBuf>,
    trigger: Trigger,
}

impl SpikeWatch {
    pub(super) fn new() -> Self {
        let package = find_package(Path::new(HWMON));
        if package.is_none() {
            eprintln!("spike: no CPU package temperature in {HWMON}, not watching");
        }
        SpikeWatch {
            package,
            trigger: Trigger::new(),
        }
    }
}

impl Poller for SpikeWatch {
    fn every(&self) -> Duration {
        Duration::from_secs(1)
    }

    /// One file read a second; the capture runs only on a spike.
    fn poll(&mut self) {
        let Some(path) = &self.package else { return };
        let Some(temp) = read_u64(path).map(|m| m as f64 / 1000.0) else {
            return;
        };
        if let Some(rise) = self.trigger.see(temp) {
            database::write(capture(temp, rise));
        }
    }
}

/// What was running: the processes over the next second, the busiest five's context switches over
/// the half second after, then the rest of the moment. The commands run last, so their own CPU
/// time does not land in either measurement.
fn capture(temp: f64, rise: f64) -> Spike {
    let ts = database::now();
    let cpu_mhz = read_max_mhz();
    let energy = read_u64(&Path::new(RAPL).join("energy_uj"));
    let before = read_all();
    let start = Instant::now();
    std::thread::sleep(MEASURE);
    let after = read_all();
    let secs = start.elapsed().as_secs_f64();
    let max_range = read_u64(&Path::new(RAPL).join("max_energy_range_uj")).unwrap_or(u64::MAX);
    let cpu_watt = energy
        .zip(read_u64(&Path::new(RAPL).join("energy_uj")))
        .and_then(|(e0, e1)| watts(e0, e1, max_range, secs));
    let mut procs = busiest(&before, &after, secs, 5);
    add_switch_rates(&mut procs, SWITCHES);
    Spike {
        ts,
        temp,
        rise,
        cpu_mhz,
        cpu_watt,
        on_ac: on_ac(),
        ppd_profile: read_ppd(),
        tccd_profile: read_tccd(),
        spotify: spotify(),
        gpu_procs: gpu_procs(),
        procs,
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The CPU package sensor: Intel's `coretemp` "Package id 0", or AMD's `k10temp` "Tctl".
pub(super) fn find_package(root: &Path) -> Option<PathBuf> {
    let read = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
    };
    let mut chips: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    chips.sort();
    for chip in chips {
        let name = read(chip.join("name")).unwrap_or_default();
        let wanted = match name.as_str() {
            "coretemp" => "Package id",
            "k10temp" => "Tctl",
            _ => continue,
        };
        for n in 1..=64 {
            if read(chip.join(format!("temp{n}_label"))).is_some_and(|l| l.starts_with(wanted)) {
                return Some(chip.join(format!("temp{n}_input")));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::find_package;
    use std::path::PathBuf;

    #[test]
    fn the_package_sensor_is_found_among_other_chips() {
        let root: PathBuf = std::env::temp_dir().join(format!("bb-spike-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let chip = |dir: &str, name: &str, labels: &[(u32, &str)]| {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
            for (n, l) in labels {
                std::fs::write(d.join(format!("temp{n}_label")), format!("{l}\n")).unwrap();
            }
        };
        chip("hwmon0", "nvme", &[(1, "Composite")]);
        chip("hwmon5", "coretemp", &[(2, "Core 0"), (1, "Package id 0")]);
        assert_eq!(find_package(&root), Some(root.join("hwmon5/temp1_input")));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(find_package(&root), None);
    }
}
