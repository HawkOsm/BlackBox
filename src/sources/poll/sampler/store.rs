use super::sensors::Reading;
use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, params};

/// One sampler tick: the system sample, power, every sensor and, every third tick, the top
/// processes. They share a timestamp and are one commit.
pub struct Tick {
    pub ts: i64,
    pub sample: Sample,
    /// Severity and summary, when a threshold is crossed.
    pub alert: Option<(Severity, String)>,
    pub power: PowerSample,
    pub sensors: Vec<Reading>,
    pub procs: Vec<ProcSample>,
}

pub struct Sample {
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub disk_io_kb: f64,
    pub load_avg: f64,
    pub gpu_pct: Option<f64>,
    pub gpu_mem_pct: Option<f64>,
    pub gpu_temp: Option<f64>,
}

/// One reading of the power state. Every field is optional: a desktop has no battery, and RAPL
/// or the GPU may be unreadable or asleep.
pub struct PowerSample {
    pub on_battery: Option<bool>,
    pub battery_pct: Option<f64>,
    pub cpu_watt: Option<f64>, // CPU package, from the RAPL energy counter
    pub gpu_watt: Option<f64>, // nvidia-smi; only polled every third sample
    pub battery_watt: Option<f64>, // whole-machine draw, only while discharging
}

pub struct ProcSample {
    pub pid: i32,
    pub comm: String,
    pub rss_kb: i64,
    pub pss_kb: Option<i64>, // Option becomes NULL in SQL when the read failed
}

impl Record for Tick {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        let p = &self.power;
        c.execute(
            "INSERT INTO power (ts, on_battery, battery_pct, cpu_watt, gpu_watt, battery_watt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ts,
                p.on_battery,
                p.battery_pct,
                p.cpu_watt,
                p.gpu_watt,
                p.battery_watt
            ],
        )?;
        // A reading that is not a number (NaN, infinite) is skipped: SQLite would store NaN as
        // NULL, which `value NOT NULL` rejects, and that would take the whole tick down with it.
        for r in self.sensors.iter().filter(|r| r.value.is_finite()) {
            c.execute(
                "INSERT INTO sensors (ts, kind, name, value) VALUES (?1, ?2, ?3, ?4)",
                params![ts, r.kind, r.name, r.value],
            )?;
        }
        for p in &self.procs {
            c.execute(
                "INSERT INTO procstat (ts, pid, comm, rss_kb, pss_kb) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ts, p.pid, p.comm, p.rss_kb, p.pss_kb],
            )?;
        }
        // last: its alert takes the row id from the last insert
        let s = &self.sample;
        c.execute(
            "INSERT INTO sysstat (ts, cpu_pct, mem_pct, disk_io_kb, load_avg, gpu_pct, gpu_mem_pct, gpu_temp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ts, s.cpu_pct, s.mem_pct, s.disk_io_kb, s.load_avg, s.gpu_pct, s.gpu_mem_pct,
                s.gpu_temp
            ],
        )?;
        let alert = self.alert.as_ref().map(|(sev, text)| (*sev, text.as_str()));
        index(c, "sysstat", &ts, alert)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    fn reading(kind: &'static str, name: &str, value: f64) -> Reading {
        Reading {
            kind,
            name: name.into(),
            value,
        }
    }

    #[test]
    fn a_tick_is_stored_whole_with_its_alert_on_the_sample() {
        let c = open(":memory:");
        Tick {
            ts: 1_700_000_100,
            sample: Sample {
                cpu_pct: 1.0,
                mem_pct: 96.0,
                disk_io_kb: 3.0,
                load_avg: 0.5,
                gpu_pct: Some(4.0),
                gpu_mem_pct: None,
                gpu_temp: None,
            },
            alert: Some((Severity::Error, "memory 96%".to_string())),
            power: PowerSample {
                on_battery: Some(true),
                battery_pct: Some(62.0),
                cpu_watt: Some(7.5),
                gpu_watt: None,
                battery_watt: Some(9.0),
            },
            sensors: vec![
                reading("temp", "nvme[nvme0]/Composite", 36.0),
                // a counter read twice in the same instant: 0 J / 0 s
                reading("power", "rapl/core", f64::NAN),
                reading("power", "rapl/psys", 21.5),
            ],
            procs: vec![ProcSample {
                pid: 1,
                comm: "init".to_string(),
                rss_kb: 1000,
                pss_kb: None,
            }],
        }
        .store(&c)
        .unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM power"), 1);
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM sensors"),
            2,
            "the NaN reading is skipped"
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM procstat"), 1);
        // the alert points at the sysstat row, not at the rows inserted with it
        let (ref_id, id): (i64, i64) = c
            .query_row(
                "SELECT ref_id, sysstat_id FROM messages, sysstat WHERE source = 'sysstat'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ref_id, id);
    }
}
