//! The rest of the moment a spike is captured: on AC or not, the active profiles, whether Spotify
//! plays, and the processes on the NVIDIA GPU. Each is None when it cannot be read.

use crate::sources::poll::sampler::{nvidia_asleep, output_within};
use std::process::Command;
use std::time::Duration;

/// Whether any mains supply is online; None on a machine that reports none.
pub(super) fn on_ac() -> Option<bool> {
    let mut seen = None;
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let read =
            |name: &str| std::fs::read_to_string(entry.path().join(name)).unwrap_or_default();
        if read("type").trim() == "Mains" {
            seen = Some(seen.unwrap_or(false) || read("online").trim() == "1");
        }
    }
    seen
}

/// Spotify's state, `Playing`, `Paused` or `Stopped`, from `playerctl`; None when either is
/// missing.
pub(super) fn spotify() -> Option<String> {
    let out = output_within(
        Command::new("playerctl").args(["--player=spotify", "status"]),
        Duration::from_secs(2),
    )?;
    if !out.status.success() {
        return None; // "No players found"
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

/// The processes on the GPU, from `nvidia-smi pmon`; None while the card sleeps (asking would wake
/// it) or when nvidia-smi is missing.
pub(super) fn gpu_procs() -> Option<String> {
    if nvidia_asleep() {
        return None;
    }
    let out = output_within(
        Command::new("nvidia-smi").args(["pmon", "--count", "1", "--select", "u"]),
        Duration::from_secs(5),
    )?;
    if !out.status.success() {
        return None;
    }
    Some(parse_pmon(&String::from_utf8(out.stdout).ok()?))
}

/// One `pid comm type sm%` line per process. pmon's columns: gpu pid type sm mem enc dec jpg ofa
/// command, `-` where there is no value, and `#` header lines.
fn parse_pmon(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[1] == "-" {
            continue; // a GPU with no process prints a row of dashes
        }
        let sm = if f[3] == "-" { "0" } else { f[3] };
        lines.push(format!("{} {} {} sm {sm}%", f[1], f[9..].join(" "), f[2]));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::parse_pmon;

    #[test]
    fn pmon_rows_become_one_line_each() {
        let text = "# gpu         pid   type     sm    mem    enc    dec    jpg    ofa    command \n\
                    # Idx           #    C/G      %      %      %      %      %      %    name \n\
                    \x20   0        837     G      -      -      -      -      -      -    Xorg           \n\
                    \x20   0       1490     G     19      1      -      -      -      -    Hyprland       \n";
        assert_eq!(parse_pmon(text), "837 Xorg G sm 0%\n1490 Hyprland G sm 19%");
        let idle = "# gpu pid type sm mem enc dec jpg ofa command\n    0 - - - - - - - - -\n";
        assert_eq!(parse_pmon(idle), "");
    }
}
