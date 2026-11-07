use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, params};

/// A captured spike: the moment, and the processes busiest in the second after it.
pub struct Spike {
    pub ts: i64,
    pub temp: f64,
    pub rise: f64,
    pub cpu_mhz: Option<f64>,
    pub cpu_watt: Option<f64>,
    pub on_ac: Option<bool>,
    pub ppd_profile: Option<String>,
    pub tccd_profile: Option<String>,
    pub spotify: Option<String>,
    pub gpu_procs: Option<String>,
    pub procs: Vec<SpikeProc>,
}

pub struct SpikeProc {
    pub pid: i32,
    pub comm: String,
    pub cpu_pct: f64,
    pub ctxt_per_s: Option<f64>,
}

impl Spike {
    /// `cpu 86C, +17C in 3 s · firefox 120%, spotify 15%`
    fn summary(&self) -> String {
        let mut text = format!("cpu {:.0}C, {:+.0}C in 3 s", self.temp, self.rise);
        let top: Vec<String> = self
            .procs
            .iter()
            .take(3)
            .map(|p| format!("{} {:.0}%", p.comm, p.cpu_pct))
            .collect();
        if !top.is_empty() {
            text.push_str(" · ");
            text.push_str(&top.join(", "));
        }
        text
    }
}

impl Record for Spike {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO spikes (ts, temp, rise, cpu_mhz, cpu_watt, on_ac, ppd_profile,
                                 tccd_profile, spotify, gpu_procs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                ts,
                self.temp,
                self.rise,
                self.cpu_mhz,
                self.cpu_watt,
                self.on_ac,
                self.ppd_profile,
                self.tccd_profile,
                self.spotify,
                self.gpu_procs
            ],
        )?;
        let spike_id = c.last_insert_rowid();
        // right after the spike row: `index` takes its ref_id from the last insert
        index(c, "spikes", &ts, Some((Severity::Warning, &self.summary())))?;
        for p in &self.procs {
            c.execute(
                "INSERT INTO spike_procs (ts, spike_id, pid, comm, cpu_pct, ctxt_per_s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![ts, spike_id, p.pid, p.comm, p.cpu_pct, p.ctxt_per_s],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{count, open};

    #[test]
    fn a_spike_is_listed_and_its_processes_point_at_it() {
        let c = open(":memory:");
        let proc = |comm: &str, cpu_pct| SpikeProc {
            pid: 7,
            comm: comm.into(),
            cpu_pct,
            ctxt_per_s: Some(900.0),
        };
        Spike {
            ts: 1_700_000_000,
            temp: 86.4,
            rise: 17.2,
            cpu_mhz: Some(4800.0),
            cpu_watt: Some(61.0),
            on_ac: Some(true),
            ppd_profile: Some("balanced".into()),
            tccd_profile: None,
            spotify: Some("Playing".into()),
            gpu_procs: None,
            procs: vec![proc("firefox", 120.0), proc("spotify", 15.0)],
        }
        .store(&c)
        .unwrap();
        let (summary, ref_id): (String, i64) = c
            .query_row(
                "SELECT summary, ref_id FROM messages WHERE source = 'spikes'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(summary, "cpu 86C, +17C in 3 s · firefox 120%, spotify 15%");
        let spike_id = count(&c, "SELECT spike_id FROM spikes");
        assert_eq!(ref_id, spike_id);
        assert_eq!(
            count(
                &c,
                &format!("SELECT COUNT(*) FROM spike_procs WHERE spike_id = {spike_id}")
            ),
            2
        );
    }
}
