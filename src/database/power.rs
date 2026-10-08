use super::{record, ts_text};
use rusqlite::params;

/// One reading of the power state. Every field is optional: a desktop has no battery, and RAPL
/// or the GPU may be unreadable or asleep.
pub struct PowerSample {
    pub on_battery: Option<bool>,
    pub battery_pct: Option<f64>,
    pub cpu_watt: Option<f64>, // CPU package, from the RAPL energy counter
    pub gpu_watt: Option<f64>, // nvidia-smi; only polled every third sample
    pub battery_watt: Option<f64>, // whole-machine draw, only while discharging
}

pub fn add_power_event(ts: i64, s: &PowerSample) {
    let ts = ts_text(ts);
    record("power", &ts, None, |tx| {
        tx.execute(
            "INSERT INTO power (ts, on_battery, battery_pct, cpu_watt, gpu_watt, battery_watt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ts,
                s.on_battery,
                s.battery_pct,
                s.cpu_watt,
                s.gpu_watt,
                s.battery_watt
            ],
        )
        .map(|_| ())
    });
}
