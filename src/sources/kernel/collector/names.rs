//! Process names, kept while the processes are alive: an exit event carries no name, and by the
//! time it arrives `/proc/<pid>` is usually gone.

use std::collections::HashMap;

/// Keeps a new program's name: by the time it exits, `/proc/<pid>` is usually gone.
pub(super) fn remember(names: &mut HashMap<u32, String>, pid: u32) {
    if let Some(name) = read_comm(pid) {
        if names.len() > 50_000 {
            // exits lost to a queue overflow leave stale entries: start over from what is alive
            *names = scan_proc_names();
        }
        names.insert(pid, name);
    }
}

pub(super) fn scan_proc_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let pid = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok());
            if let Some(pid) = pid
                && let Some(name) = read_comm(pid)
            {
                names.insert(pid, name);
            }
        }
    }
    names
}

pub(super) fn read_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}
