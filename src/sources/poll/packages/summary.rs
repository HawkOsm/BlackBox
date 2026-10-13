use super::{ACTIONS, Transaction};

/// Packages that most often explain a machine that stopped working, listed first in a summary.
/// Matched as substrings, so `linux` also covers Debian's `linux-image-…` and `kernel` Fedora's
/// `kernel-core`.
const KEY_PACKAGES: [&str; 15] = [
    "linux",
    "kernel",
    "nvidia",
    "mesa",
    "systemd",
    "glibc",
    "libc6",
    "mkinitcpio",
    "dracut",
    "initramfs",
    "grub",
    "shim",
    "ucode",
    "microcode",
    "firmware",
];

fn is_key(name: &str) -> bool {
    KEY_PACKAGES.iter().any(|k| name.contains(k))
}

/// `pacman: upgraded 57, installed 2 · linux 6.9-1 → 6.10-1, nvidia, mesa, bash, …`
pub fn summary(manager: &str, t: &Transaction) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for action in ACTIONS {
        let n = t.changes.iter().filter(|c| c.starts_with(action)).count();
        if n > 0 {
            counts.push((action, n));
        }
    }
    let mut names: Vec<(bool, String)> = t
        .changes
        .iter()
        .filter_map(|c| {
            let mut words = c.split_whitespace();
            let action = words.next()?;
            let name = words.next()?;
            let key = is_key(name);
            // a version is worth the space only for the packages that tend to break things
            let shown = match c.split_once('(').map(|(_, v)| v.trim_end_matches(')')) {
                Some(v) if key && action == "upgraded" && v.contains("->") => {
                    format!("{name} {}", v.replace("->", "→"))
                }
                _ => name.to_string(),
            };
            Some((key, shown))
        })
        .collect();
    names.sort_by_key(|(key, _)| !key);
    let mut listed: Vec<String> = names.iter().take(4).map(|(_, n)| n.clone()).collect();
    if names.len() > 4 {
        listed.push("…".to_string());
    }
    let counted: Vec<String> = counts.iter().map(|(a, n)| format!("{a} {n}")).collect();
    format!("{manager}: {} · {}", counted.join(", "), listed.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(changes: &[&str]) -> Transaction {
        Transaction {
            ts: 0,
            command: None,
            changes: changes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn summary_names_the_manager_and_lists_key_packages_first() {
        let s = summary(
            "apt",
            &t(&[
                "upgraded bash (5.2-1 -> 5.2-2)",
                "upgraded linux-image-6.1.0-10-amd64 (6.1.38-1 -> 6.1.38-2)",
                "installed foo (1.0)",
            ]),
        );
        assert_eq!(
            s,
            "apt: upgraded 2, installed 1 · linux-image-6.1.0-10-amd64 6.1.38-1 → 6.1.38-2, bash, foo"
        );
    }

    #[test]
    fn a_key_package_without_an_old_version_shows_no_arrow() {
        // zypper logs an upgrade as an install of the new version only
        assert_eq!(
            summary("zypper", &t(&["installed kernel-default (6.11-1.1)"])),
            "zypper: installed 1 · kernel-default"
        );
    }
}
