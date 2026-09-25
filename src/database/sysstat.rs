use super::{record, ts_text};
use rusqlite::params;

pub struct Sample {
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub disk_io_kb: f64,
    pub load_avg: f64,
    pub gpu_pct: Option<f64>,
    pub gpu_mem_pct: Option<f64>,
    pub gpu_temp: Option<f64>,
}

pub fn add_sysstat_event(ts: i64, s: &Sample, alert: Option<(&str, &str)>) {
    let ts = ts_text(ts);
    record("sysstat", &ts, alert, |tx| {
        tx.execute(
            "INSERT INTO sysstat (ts, cpu_pct, mem_pct, disk_io_kb, load_avg, gpu_pct, gpu_mem_pct, gpu_temp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ts, s.cpu_pct, s.mem_pct, s.disk_io_kb, s.load_avg, s.gpu_pct, s.gpu_mem_pct,
                s.gpu_temp
            ],
        )
        .map(|_| ())
    });
}
