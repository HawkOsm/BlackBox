//! The NVIDIA GPU, through `nvidia-smi`: asked only while the card is awake, and given up on if
//! the driver hangs.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Whether the NVIDIA card is in runtime suspend: asking `nvidia-smi` then would wake it.
pub(crate) fn nvidia_asleep() -> bool {
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

/// Runs a command but gives up, killing it, after `limit`: a hung driver must not stall the sampler.
pub(crate) fn output_within(cmd: &mut Command, limit: Duration) -> Option<std::process::Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            // the output is a few bytes, so it fits in the pipe while we wait
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let program = cmd.get_program().to_string_lossy();
                eprintln!("{program} did not answer within {limit:?}, skipped");
                return None;
            }
        }
    }
}

/// One reading of the card. `watt` and `mhz` are None on their own when the card does not report
/// them.
#[derive(Clone, Copy)]
pub(super) struct Gpu {
    pub(super) pct: f64,
    pub(super) mem_pct: f64,
    pub(super) temp: f64,
    pub(super) watt: Option<f64>,
    pub(super) mhz: Option<f64>,
}

/// The card from nvidia-smi; None when absent, failing, or asleep.
pub(super) fn read_gpu() -> Option<Gpu> {
    if nvidia_asleep() {
        return None;
    }
    let out = output_within(
        Command::new("nvidia-smi").args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,clocks.gr",
            "--format=csv,noheader,nounits",
        ]),
        Duration::from_secs(5),
    )?;
    if !out.status.success() {
        return None;
    }
    parse_gpu(&String::from_utf8(out.stdout).ok()?)
}

fn parse_gpu(text: &str) -> Option<Gpu> {
    // an unsupported field prints "[N/A]": parse each on its own so one cannot hide the others
    let v: Vec<Option<f64>> = text
        .lines()
        .next()?
        .split(',')
        .map(|x| x.trim().parse().ok())
        .collect();
    if v.len() != 6 {
        return None;
    }
    let (util, used, total, temp) = (v[0]?, v[1]?, v[2]?, v[3]?);
    if total <= 0.0 {
        return None;
    }
    Some(Gpu {
        pct: util,
        mem_pct: 100.0 * used / total,
        temp,
        watt: v[4],
        mhz: v[5],
    })
}

#[cfg(test)]
mod tests {
    use super::parse_gpu;

    #[test]
    fn the_clock_is_read_and_a_missing_field_stays_alone() {
        let g = parse_gpu("12, 400, 8000, 51, [N/A], 1155\n").unwrap();
        assert_eq!((g.pct, g.mem_pct, g.temp), (12.0, 5.0, 51.0));
        assert_eq!((g.watt, g.mhz), (None, Some(1155.0)));
        assert!(
            parse_gpu("12, 400, 8000, 51, 9.5\n").is_none(),
            "an old field count"
        );
    }
}
