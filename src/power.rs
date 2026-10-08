use crate::database::PowerSample;
use std::time::{Duration, Instant};

const RAPL: &str = "/sys/class/powercap/intel-rapl:0";
const SUPPLIES: &str = "/sys/class/power_supply";
/// A gap this long means the machine slept; the energy counter kept counting, our clock did not.
const MAX_GAP: Duration = Duration::from_secs(30);

fn read_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Average watts between two readings of a wrapping microjoule counter.
pub fn watts(prev: u64, now: u64, max_range: u64, secs: f64) -> Option<f64> {
    if secs <= 0.0 {
        return None;
    }
    let uj = if now >= prev {
        now - prev
    } else if prev <= max_range {
        max_range - prev + now
    } else {
        return None;
    };
    Some(uj as f64 / 1e6 / secs)
}

pub struct Power {
    prev: Option<(u64, Instant)>,
    max_range: u64,
    warned: bool,
}

impl Power {
    pub fn new() -> Self {
        Power {
            prev: None,
            max_range: read_u64(&format!("{RAPL}/max_energy_range_uj")).unwrap_or(u64::MAX),
            warned: false,
        }
    }

    /// CPU package watts since the previous call; None on the first call, after a sleep, or when
    /// the counter cannot be read (it is root-only unless `deploy/setup-sensors.sh` was run).
    fn cpu_watt(&mut self) -> Option<f64> {
        let Some(uj) = read_u64(&format!("{RAPL}/energy_uj")) else {
            if !self.warned {
                self.warned = true;
                eprintln!(
                    "power: cannot read {RAPL}/energy_uj (root-only; see deploy/setup-sensors.sh)"
                );
            }
            self.prev = None;
            return None;
        };
        let now = Instant::now();
        let prev = self.prev.replace((uj, now))?;
        let gap = now.duration_since(prev.1);
        if gap > MAX_GAP {
            return None;
        }
        watts(prev.0, uj, self.max_range, gap.as_secs_f64())
    }

    pub fn sample(&mut self, gpu_watt: Option<f64>) -> PowerSample {
        let battery = battery();
        PowerSample {
            on_battery: battery.as_ref().map(|b| b.discharging),
            battery_pct: battery.as_ref().and_then(|b| b.pct),
            cpu_watt: self.cpu_watt(),
            gpu_watt,
            battery_watt: battery.and_then(|b| b.discharging.then_some(b.watt).flatten()),
        }
    }
}

struct Battery {
    discharging: bool,
    pct: Option<f64>,
    watt: Option<f64>,
}

fn battery() -> Option<Battery> {
    for entry in std::fs::read_dir(SUPPLIES).ok()?.flatten() {
        let dir = entry.path();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let text = |name: &str| read(name).map(|s| s.trim().to_string());
        let num = |name: &str| text(name).and_then(|s| s.parse::<f64>().ok());
        if text("type").as_deref() != Some("Battery") || text("scope").as_deref() == Some("Device")
        {
            continue; // skips mains, USB, and the batteries of peripherals
        }
        // power_now where the driver has it, else current x voltage (both in micro-units)
        let watt = num("power_now")
            .map(|uw| uw / 1e6)
            .or_else(|| Some(num("current_now")? * num("voltage_now")? / 1e12));
        return Some(Battery {
            discharging: text("status").as_deref() == Some("Discharging"),
            pct: num("capacity"),
            watt: watt.map(f64::abs),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::watts;

    #[test]
    fn watts_from_counter_delta() {
        // 15 J over 10 s
        assert_eq!(
            watts(1_000_000, 16_000_000, 262_143_328_850, 10.0),
            Some(1.5)
        );
    }

    #[test]
    fn counter_wrap_is_followed() {
        // 200 uJ before the wrap, 300 after, over 1 s: 500 uJ
        let w = watts(999_800, 300, 1_000_000, 1.0).unwrap();
        assert!((w - 0.0005).abs() < 1e-12);
    }

    #[test]
    fn nonsense_is_dropped() {
        assert_eq!(watts(5_000, 10, 1_000, 1.0), None); // prev beyond the counter range
        assert_eq!(watts(1, 2, 1_000, 0.0), None);
    }
}
