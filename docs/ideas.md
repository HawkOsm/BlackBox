# Ideas

Not planned or scheduled. Notes for possible future features.

## Package-update context for crash correlation

**Motivation.** On 2026-10-07 Quickshell crashed repeatedly (SIGSEGV in
`QQuickRepeater::clear`, five identical reports). The likely trigger was a
`qt6-declarative` 6.11.2-1 -> 6.11.2-2 update on 2026-09-22; rolling back to the
cached 6.11.2-1 package stopped the crashes. Finding that took manual digging through
`pacman.log`, `coredumpctl` and the journal.

**Idea.** Have BlackBox record package changes next to process events, so a crash can be
lined up with "what changed on this machine shortly before".

- Parse `/var/log/pacman.log` (`upgraded` / `installed` / `removed` lines) into an
  events table with timestamp, package, old and new version.
- Link crash events (SIGSEGV/SIGABRT/SIGBUS coredumps) to packages upgraded earlier,
  especially packages the crashing binary links against.
- Surface it as "crashed N times since `<package>` was upgraded on `<date>`".

**Related: pacman cache health.** Downgrading only worked because the old package was
still in `/var/cache/pacman/pkg`. A cheap check is to warn when the cache no longer
holds the previous version of a recently upgraded package.

Maintenance already set up on this machine: `paccache.timer` is enabled (weekly, keeps
the last 3 versions of each package):

```bash
sudo systemctl enable --now paccache.timer
```

**Out of scope for now:** filesystem snapshots (Timeshift/snapper). The root filesystem
is ext4, so snapper is not an option, and Timeshift was deliberately skipped.

## NVMe disk health (SMART)

**Motivation.** The recorder logs both NVMe drives' temperatures (hwmon needs no root), but not
their wear and error counters: rated life used, spare capacity, data written, media errors,
power-on hours, and unsafe shutdowns (which also climb on a freeze or power loss, so they would
line up with the unclean-boot rows).

**What was tried, 2026-10-08.** Reading the SMART log with `nvme smart-log -o json` as a normal
user fails with `smart log: Permission denied`, on kernel 7.2.8 with the user in group `wheel`:

- `/dev/nvme0` is `root:root 0600` by default.
- A udev rule giving group `wheel` read access (`0640`) did not help.
- Neither did read and write (`0660`). The kernel refuses unprivileged SMART passthrough whatever
  the device permissions are. Both rules were removed and the device modes restored.
- sysfs has no SMART counters, only the temperature.

**Idea that would work.** A small root-run systemd timer, installed once by a `sudo` setup
script, that every 10 minutes runs `nvme smart-log -o json` for each controller in
`/sys/class/nvme` and writes the result to `/var/lib/blackbox/` (mode `0640`, group `wheel`). The
user-level recorder reads that file instead of touching the device, so only a read-only command
runs as root and the drives stay closed to everyone else. A first sketch used a narrow `health`
table of (ts, device, metric, value) rows and a `disk` source in the problems list.

**Alerts worth having.** Only on change: a critical-warning bit, spare capacity at its
threshold, rising media errors, 90% of rated life used, and each rise in unsafe shutdowns.

**Cost.** One system service and one timer on the machine, and a sudo prompt at install time.
`install.sh` already has an optional sudo step (`deploy/setup-sensors.sh`) it could extend.
`nvme-cli` is already installed here. A SATA drive would need `smartctl` instead.
