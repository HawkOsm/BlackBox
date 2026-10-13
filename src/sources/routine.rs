//! Events that look alarming but are routine on any Linux machine. They are still stored, so the
//! record stays complete, but at info level, so they are not raised as problems.
//!
//! Every rule here must hold on every machine, not only the one it was found on: it matches text
//! the emitting program always prints for that situation, and never a user name, host name,
//! device address, terminal or path that differs between machines. A real fault on one machine
//! (a missing package, a misconfigured service) is fixed on that machine, not listed here.

use std::path::Path;

/// A journal line is routine when it contains `text`, and, if `unit` is set, comes from that unit.
pub struct Rule {
    pub unit: Option<&'static str>,
    pub text: &'static str,
}

const fn any(text: &'static str) -> Rule {
    Rule { unit: None, text }
}

const fn from(unit: &'static str, text: &'static str) -> Rule {
    Rule {
        unit: Some(unit),
        text,
    }
}

pub const JOURNAL: &[Rule] = &[
    // the kernel prints this at every shutdown, when systemd hands the hardware watchdog back
    any("watchdog did not stop!"),
    // `sudo -n` (never prompt) found no cached credentials and gave up without asking: no
    // password was tried. Shell prompts, editors and scripts run `sudo -n true` to check whether
    // sudo is unlocked. sudo logs it at alert level, so it used to land as an error.
    any(" : a password is required ; "),
    // A password prompt that ended without an answer: it timed out, was cancelled with Ctrl-C or
    // Escape, or had no terminal to ask on. Wrong passwords are still raised, by pam_unix's
    // "authentication failure", sudo's "incorrect password attempts" and the audit log.
    any(":auth): conversation failed"),
    any(":auth): auth could not identify password for"),
    // gnome-keyring's PAM module runs at login before its daemon exists
    any("gkr-pam: unable to locate daemon control file"),
    // a kernel built with Intel TDX support, on a CPU or firmware without it
    any("virt/tdx: TDX not supported by the host platform"),
    // wpa_supplicant probing a capability the Wi-Fi driver does not have
    any("nl80211: kernel reports: multicast RX registrations are not supported"),
    // Intel VMD (the NVMe RAID controller) describing its fixed bus layout at every boot
    any("Primary bus is hard wired to 0"),
    // bluetoothd trying to reconnect to a paired device that is switched off or out of range
    from("bluetooth.service", "Host is down"),
    // bluetoothd's cached audio endpoint is gone after the device changed: it negotiates afresh
    from("bluetooth.service", "Unable to load LastUsed: rseid"),
    // a long-standing gnome-keyring quirk when an app looks up a secret it has already exported
    any("asked to register item /org/freedesktop/secrets/"),
    // Boot banners the kernel prints at warning level on every boot of such a machine. A module
    // built outside the kernel tree (NVIDIA, VirtualBox, ZFS) taints it by definition.
    any("loading out-of-tree module taints kernel"),
    any("NVRM: loading NVIDIA UNIX"),
    any("ENERGY_PERF_BIAS: Set to 'normal', was 'performance'"),
    // a device tree or ACPI table names no regulator for a supply that is always on
    any(" not found, using dummy regulator"),
    // a laptop whose firmware hides the PS/2 mouse port because the touchpad is on I2C or SMBus
    any("PS/2 appears to have AUX port disabled"),
];

pub fn journal(unit: Option<&str>, message: &str) -> bool {
    JOURNAL
        .iter()
        .any(|r| message.contains(r.text) && r.unit.is_none_or(|u| unit == Some(u)))
}

/// Interface name prefixes of the virtual devices that container and VM tools attach to bridges.
const BRIDGE_PORTS: [&str; 9] = [
    "veth", "docker", "br-", "virbr", "vnet", "tap", "podman", "cni", "vboxnet",
];

/// A network device entering or leaving promiscuous mode (audit `ANOM_PROMISCUOUS`). Leaving it
/// is never a threat, and a bridge port must be promiscuous to forward frames: Docker, Podman,
/// libvirt and every VM tap device do this whenever a container or VM starts. A physical
/// interface entering it, as a packet sniffer does, stays an error.
pub fn promiscuous(dev: Option<&str>, prom: Option<&str>) -> bool {
    if prom == Some("0") {
        return true;
    }
    let Some(dev) = dev.filter(|d| !d.is_empty() && !d.contains('/') && *d != "..") else {
        return false;
    };
    // A device without hardware behind it sits under /sys/devices/virtual. That only answers
    // while it exists, so the usual names count too, for records read back after a restart.
    BRIDGE_PORTS.iter().any(|p| dev.starts_with(p))
        || Path::new("/sys/devices/virtual/net").join(dev).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_probes_and_unanswered_prompts_are_routine() {
        assert!(journal(
            Some("session-3.scope"),
            "    alice : a password is required ; TTY=pts/0 ; PWD=/home/alice ; USER=root ; COMMAND=/usr/bin/true"
        ));
        assert!(journal(
            Some("session-7.scope"),
            "pam_unix(sudo:auth): conversation failed"
        ));
        assert!(journal(
            None,
            "pam_unix(polkit-1:auth): auth could not identify password for [bob]"
        ));
    }

    #[test]
    fn wrong_passwords_are_not_routine() {
        assert!(!journal(
            Some("session-3.scope"),
            "    alice : 3 incorrect password attempts ; TTY=pts/0 ; PWD=/home/alice ; USER=root ; COMMAND=/usr/bin/pacman"
        ));
        assert!(!journal(
            None,
            "pam_unix(sudo:auth): authentication failure; logname=alice uid=1000 euid=0 tty=/dev/pts/0 ruser=alice rhost=  user=alice"
        ));
    }

    #[test]
    fn a_unit_rule_only_matches_its_unit() {
        let line = "profiles/audio/avdtp.c:avdtp_connect_cb() connect to 00:11:22:33:44:55: Host is down (112)";
        assert!(journal(Some("bluetooth.service"), line));
        assert!(!journal(Some("nfs-client.target"), "server Host is down"));
        assert!(!journal(None, line));
    }

    #[test]
    fn real_faults_are_not_routine() {
        assert!(!journal(
            Some("kernel"),
            "BUG: Bad page state in process swapper"
        ));
        assert!(!journal(Some("kernel"), "x86/CPU: Running old microcode"));
        assert!(!journal(
            Some("wpa_supplicant.service"),
            "wlp0s20f3: nl80211: kernel reports: key not allowed"
        ));
    }

    #[test]
    fn promiscuous_mode_on_bridge_ports_only() {
        assert!(promiscuous(Some("veth14fb608"), Some("256")), "docker");
        assert!(promiscuous(Some("vnet3"), Some("256")), "libvirt");
        assert!(promiscuous(Some("anything"), Some("0")), "leaving it");
        assert!(!promiscuous(Some("physnic9"), Some("256")), "a sniffer");
        assert!(!promiscuous(None, Some("256")));
        assert!(!promiscuous(Some("../x"), Some("256")));
    }
}
