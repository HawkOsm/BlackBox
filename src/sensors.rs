use crate::power::watts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HWMON: &str = "/sys/class/hwmon";
const POWERCAP: &str = "/sys/class/powercap";
/// See `power::MAX_GAP`: a counter delta over a suspend is not a power reading.
const MAX_GAP: Duration = Duration::from_secs(30);

/// One sensor value. `kind` is "temp" (degrees C), "fan" (rpm) or "power" (watts).
pub struct Reading {
    pub kind: &'static str,
    pub name: String,
    pub value: f64,
}

fn read_text(path: &Path) -> Option<String> {
    Some(std::fs::read_to_string(path).ok()?.trim().to_string())
}

fn read_f64(path: &Path) -> Option<f64> {
    read_text(path)?.parse().ok()
}

/// Every temperature, fan and power input of every hwmon chip under `root`, named
/// `chip/label`. Two chips with the same name (two NVMe drives, two DIMMs) get their device
/// in brackets, `nvme[nvme0]/Composite`, so their series never merge. None of this needs root.
pub fn read_hwmon(root: &Path) -> Vec<Reading> {
    let mut chips: Vec<(PathBuf, String, String)> = Vec::new();
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let Some(chip) = read_text(&path.join("name")) else {
            continue;
        };
        let device = std::fs::canonicalize(path.join("device"))
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
        chips.push((path, chip, device));
    }
    chips.sort();
    let mut count: HashMap<&str, usize> = HashMap::new();
    for (_, chip, _) in &chips {
        *count.entry(chip).or_default() += 1;
    }

    let mut out = Vec::new();
    for (path, chip, device) in &chips {
        let chip_name = if count[chip.as_str()] > 1 {
            format!("{chip}[{device}]")
        } else {
            chip.clone()
        };
        let Ok(files) = std::fs::read_dir(path) else {
            continue;
        };
        let mut inputs: Vec<(&'static str, u32, String)> = Vec::new();
        for f in files.flatten() {
            let file = f.file_name().to_string_lossy().into_owned();
            // temp1_input, fan1_input, power1_average / power1_input
            let Some((stem, suffix)) = file.split_once('_') else {
                continue;
            };
            let (kind, prefix) = match (stem.trim_end_matches(|c: char| c.is_ascii_digit()), suffix)
            {
                ("temp", "input") => ("temp", "temp"),
                ("fan", "input") => ("fan", "fan"),
                ("power", "average" | "input") => ("power", "power"),
                _ => continue,
            };
            let Ok(n) = stem[prefix.len()..].parse::<u32>() else {
                continue;
            };
            inputs.push((kind, n, file));
        }
        inputs.sort();
        // a chip that has both power1_average and power1_input is read once, from the average
        inputs.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1);
        for (kind, n, file) in inputs {
            let Some(raw) = read_f64(&path.join(&file)) else {
                continue;
            };
            let value = match kind {
                "temp" => raw / 1000.0,
                "power" => raw / 1e6,
                _ => raw,
            };
            // drivers report a failed read as a huge negative or zero-ish temperature
            if kind == "temp" && !(-40.0..=200.0).contains(&value) {
                continue;
            }
            let label = read_text(&path.join(format!("{kind}{n}_label")))
                .unwrap_or_else(|| format!("{kind}{n}"));
            out.push(Reading {
                kind,
                name: format!("{chip_name}/{label}"),
                value,
            });
        }
    }
    out
}

/// Watts of the RAPL domains other than the CPU package (`power.rs` has that one): the cores,
/// the uncore (cache, memory controller, integrated GPU), DRAM, and `psys`, the whole platform.
pub struct Rapl {
    root: PathBuf,
    prev: HashMap<String, (u64, Instant)>,
}

impl Rapl {
    pub fn new(root: &Path) -> Self {
        Rapl {
            root: root.to_path_buf(),
            prev: HashMap::new(),
        }
    }

    pub fn read(&mut self) -> Vec<Reading> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir(&self.root) else {
            return out;
        };
        let mut domains: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("intel-rapl:"))
            })
            .collect();
        domains.sort();
        for path in domains {
            let Some(name) = read_text(&path.join("name")) else {
                continue;
            };
            if name.starts_with("package-") {
                continue;
            }
            let Some(uj) = read_f64(&path.join("energy_uj")).map(|v| v as u64) else {
                continue; // root-only until deploy/setup-sensors.sh has run
            };
            let max = read_f64(&path.join("max_energy_range_uj")).map_or(u64::MAX, |v| v as u64);
            let now = Instant::now();
            let Some((before, then)) = self.prev.insert(name.clone(), (uj, now)) else {
                continue;
            };
            let gap = now.duration_since(then);
            if gap > MAX_GAP {
                continue;
            }
            if let Some(w) = watts(before, uj, max, gap.as_secs_f64()) {
                out.push(Reading {
                    kind: "power",
                    name: format!("rapl/{name}"),
                    value: w,
                });
            }
        }
        out
    }
}

pub struct Sensors {
    rapl: Rapl,
}

impl Sensors {
    pub fn new() -> Self {
        Sensors {
            rapl: Rapl::new(Path::new(POWERCAP)),
        }
    }

    pub fn read(&mut self) -> Vec<Reading> {
        let mut all = read_hwmon(Path::new(HWMON));
        all.extend(self.rapl.read());
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(root: &Path, dir: &str, name: &str, files: &[(&str, &str)]) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
        for (f, v) in files {
            std::fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bb_sensors_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn hwmon_values_are_scaled_and_labelled() {
        let root = scratch("scale");
        chip(
            &root,
            "hwmon0",
            "coretemp",
            &[
                ("temp1_input", "59000"),
                ("temp1_label", "Package id 0"),
                ("temp2_input", "55000"), // no label: falls back to the file's own name
            ],
        );
        chip(&root, "hwmon1", "acpi_fan", &[("fan1_input", "2400")]);
        chip(
            &root,
            "hwmon2",
            "amdgpu",
            &[
                ("power1_average", "15000000"),
                ("power1_input", "99000000"), // the average wins
            ],
        );
        let got: Vec<(String, String, f64)> = read_hwmon(&root)
            .into_iter()
            .map(|r| (r.kind.to_string(), r.name, r.value))
            .collect();
        assert!(got.contains(&("temp".into(), "coretemp/Package id 0".into(), 59.0)));
        assert!(got.contains(&("temp".into(), "coretemp/temp2".into(), 55.0)));
        assert!(got.contains(&("fan".into(), "acpi_fan/fan1".into(), 2400.0)));
        assert!(got.contains(&("power".into(), "amdgpu/power1".into(), 15.0)));
        assert_eq!(got.len(), 4, "one power reading per input");
    }

    #[test]
    fn chips_sharing_a_name_stay_apart_and_bad_readings_are_dropped() {
        let root = scratch("dup");
        chip(
            &root,
            "hwmon5",
            "nvme",
            &[("temp1_input", "35850"), ("temp1_label", "Composite")],
        );
        chip(
            &root,
            "hwmon6",
            "nvme",
            &[("temp1_input", "40850"), ("temp1_label", "Composite")],
        );
        chip(&root, "hwmon7", "broken", &[("temp1_input", "-273000")]);
        let names: Vec<String> = read_hwmon(&root).into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 2, "the -273 C reading is a failed read");
        assert_ne!(names[0], names[1]);
        assert!(
            names
                .iter()
                .all(|n| n.starts_with("nvme[") && n.ends_with("]/Composite"))
        );
    }

    #[test]
    fn rapl_domains_become_watts_and_the_package_is_skipped() {
        let root = scratch("rapl");
        for (dir, name) in [
            ("intel-rapl:0", "package-0"),
            ("intel-rapl:0:0", "core"),
            ("intel-rapl:1", "psys"),
        ] {
            chip(
                &root,
                dir,
                name,
                &[
                    ("energy_uj", "1000000"),
                    ("max_energy_range_uj", "262143328850"),
                ],
            );
        }
        let mut rapl = Rapl::new(&root);
        assert!(
            rapl.read().is_empty(),
            "the first read only sets the baseline"
        );
        // pretend the baseline was taken 10 s ago, and 20 J were used since
        for v in rapl.prev.values_mut() {
            v.1 = Instant::now() - Duration::from_secs(10);
        }
        for dir in ["intel-rapl:0:0", "intel-rapl:1"] {
            std::fs::write(root.join(dir).join("energy_uj"), "21000000\n").unwrap();
        }
        let mut got: Vec<(String, f64)> =
            rapl.read().into_iter().map(|r| (r.name, r.value)).collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "rapl/core");
        assert!((got[0].1 - 2.0).abs() < 0.05, "{got:?}");
        assert_eq!(got[1].0, "rapl/psys");
    }
}
