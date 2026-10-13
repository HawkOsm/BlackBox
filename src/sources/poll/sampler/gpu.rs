//! The NVIDIA GPU, through `nvidia-smi`: asked only while the card is awake, and given up on if
//! the driver hangs.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn nvidia_asleep() -> bool {
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
                eprintln!("sampler: nvidia-smi did not answer within {limit:?}, skipped");
                return None;
            }
        }
    }
}

/// (gpu %, vram %, temp C, watts) from nvidia-smi; None when absent, failing, or the card is asleep.
/// Watts is None on its own when the card does not report power.
pub(super) fn read_gpu() -> Option<(f64, f64, f64, Option<f64>)> {
    if nvidia_asleep() {
        return None;
    }
    let out = output_within(
        Command::new("nvidia-smi").args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
            "--format=csv,noheader,nounits",
        ]),
        Duration::from_secs(5),
    )?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    // an unsupported field prints "[N/A]": parse each on its own so one cannot hide the others
    let v: Vec<Option<f64>> = text
        .lines()
        .next()?
        .split(',')
        .map(|x| x.trim().parse().ok())
        .collect();
    if v.len() != 5 {
        return None;
    }
    let (util, used, total, temp) = (v[0]?, v[1]?, v[2]?, v[3]?);
    if total <= 0.0 {
        return None;
    }
    Some((util, 100.0 * used / total, temp, v[4]))
}
