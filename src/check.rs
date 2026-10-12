//! `blackbox --check`: what the recorder can see on this machine, and the one step that fixes each
//! source it cannot. Reads only; it never writes the database or starts a source.

use crate::{auditd, database, packages};
use std::process::Command;

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown Linux".to_string())
}

fn readable(path: &str) -> Result<(), String> {
    std::fs::File::open(path)
        .map(|_| ())
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "not found".to_string(),
            std::io::ErrorKind::PermissionDenied => "not readable".to_string(),
            _ => e.to_string(),
        })
}

fn audit() -> String {
    let path = auditd::log_path();
    match readable(&path) {
        Ok(()) => format!("{path}: readable"),
        Err(e) => format!("{path}: {e} (sudo bash deploy/setup-auditd.sh)"),
    }
}

/// Whether the system journal is visible, not only this user's own.
fn journal() -> String {
    let out = Command::new("journalctl")
        .args(["--system", "--quiet", "--lines=1", "--output=cat"])
        .output();
    match out {
        Err(_) => "journalctl not found: the journald source and boot tracking are off".to_string(),
        Ok(o) if !o.stdout.is_empty() => "system journal: readable".to_string(),
        Ok(_) => "system journal: not readable, only this user's entries are recorded \
                  (sudo usermod -aG systemd-journal $USER, then log in again)"
            .to_string(),
    }
}

fn gpu() -> String {
    match Command::new("nvidia-smi").arg("--list-gpus").output() {
        Ok(o) if o.status.success() => "nvidia-smi: found".to_string(),
        Ok(_) => "nvidia-smi: found, but it reports no GPU".to_string(),
        Err(_) => "nvidia-smi: not found, GPU columns stay empty".to_string(),
    }
}

fn cpu_watts() -> String {
    let zones: Vec<String> = std::fs::read_dir("/sys/class/powercap")
        .map(|dir| {
            dir.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("intel-rapl:"))
                .collect()
        })
        .unwrap_or_default();
    if zones.is_empty() {
        return "no RAPL energy counters: CPU watts stay empty".to_string();
    }
    let blocked = zones
        .iter()
        .filter(|z| readable(&format!("/sys/class/powercap/{z}/energy_uj")).is_err())
        .count();
    if blocked == 0 {
        format!("RAPL: {} counters readable", zones.len())
    } else {
        format!(
            "RAPL: {blocked} of {} counters not readable (sudo bash deploy/setup-sensors.sh)",
            zones.len()
        )
    }
}

pub fn run() {
    println!("blackbox {} on {}", env!("CARGO_PKG_VERSION"), os_name());
    let db = database::db_path();
    let size = std::fs::metadata(&db)
        .map(|m| format!("{:.1} MB", m.len() as f64 / 1e6))
        .unwrap_or_else(|_| "not created yet".to_string());
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("database", vec![format!("{db} ({size})")]),
        ("packages", packages::report()),
        ("audit log", vec![audit()]),
        ("journal", vec![journal()]),
        ("gpu", vec![gpu()]),
        ("cpu watts", vec![cpu_watts()]),
    ];
    for (label, lines) in rows {
        for (i, line) in lines.iter().enumerate() {
            println!("  {:<10} {line}", if i == 0 { label } else { "" });
        }
    }
}
