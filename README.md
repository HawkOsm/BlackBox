# Blackbox

A flight recorder for this machine. A tiny always-on service records the things you need after
something breaks (crashes, warnings, security events, system load), and a desktop app called
**Blackbox** lets you look at them and see what else was happening at the same moment.

It is built to be invisible: about 0.14% of one CPU core, 5 MB of memory, and under 10 wakeups a
second (see [docs/architecture.md](docs/architecture.md#performance)).

## What it records

| Source | What is stored |
|---|---|
| Processes | A crash (SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV, SIGSYS), named and with its stack trace from systemd-coredump, and any exit with a failure status (not 0 or 1), such as a Rust panic |
| System | CPU, memory, disk, load and NVIDIA GPU every 10 s (GPU every 30 s) |
| journald | Warnings and worse, with backfill after a restart |
| auditd | Failed logins and auth, sudo use, kernel module loads, edits to identity files, audit anomalies |

Everything lives in one SQLite file capped at 7.5 GB, and the audit log adds up to 2 GB, so the whole
thing stays within a 10 GB budget. When the database fills up, the oldest slice of time is dropped
from every table together.

It deliberately does not record every fork, exec or clean exit. That would be about 100 events a
second of noise.

## Install

```bash
deploy/install.sh
```

This needs no sudo. It builds the release binary, starts the collector as a systemd user service
(it starts at boot, since lingering is enabled) and adds **Blackbox** to the application menu.

Optional, for the auditd source (needs sudo once):

```bash
sudo bash deploy/setup-auditd.sh
```

Remove everything with `deploy/install.sh uninstall` (add `--purge` to delete the recorded data too).

## Use

- Open **Blackbox** from the app menu. It only runs while its window is open. Click any entry to see
  the details, a chart of CPU, memory and GPU around it (±2 min, ±10 min or ±1 h), a crash's stack
  trace, and every other event in that window.
- The clock button shows any moment you pick, even one where nothing was recorded.
- The copy button puts a plain-text report of the selected moment on the clipboard.
- The header shows whether the collector is alive.
- Collector status: `systemctl --user status blackbox`
- Collector log: `journalctl --user -u blackbox`
- Data: `~/.local/share/blackbox/blackbox.db`

## Settings

Environment variables, set in `deploy/blackbox.service` (or the installed copy in `~/.config/systemd/user/`):

| Variable | Default | Meaning |
|---|---|---|
| `BLACKBOX_DB` | `~/.local/share/blackbox/blackbox.db` | database path |
| `BLACKBOX_MAX_MB` | `7500` | database size cap |
| `BLACKBOX_BATCH_MS` | `100` | how long process events are allowed to pile up before being handled |
| `BLACKBOX_AUDIT_LOG` | `/var/log/audit/audit.log` | audit log to follow |

## Privacy

The database holds process names, log lines and audit events, so it never belongs in git. It lives
in `~/.local/share/blackbox/` (mode 700, files 600), and `.gitignore` blocks databases, logs, dumps,
keys and local tool settings.

## Repository

```
src/collector.rs   process exits and crashes from the kernel (netlink proc connector)
src/exit.rs        which exits are kept, and how they are described
src/socket.rs      raw netlink socket, kernel-side event filter
src/netlink.rs     netlink and connector message layout
src/sampler.rs     CPU, memory, disk, load, GPU from /proc and nvidia-smi
src/journald.rs    follows journalctl
src/auditd.rs      follows the audit log
src/database.rs    schema, writes, ring-buffer trim
ui/                the GTK4 app (blackbox_app.py) and its queries (data.py)
deploy/            installer, systemd unit, desktop entry, icon, audit rules
docs/              architecture notes and the database diagram (database.drawio)
```

## Develop

```bash
cargo test && cargo clippy
cargo test --release trim_does_not_stall_writers -- --ignored --nocapture   # stress test
python3 ui/blackbox_app.py --db some.sqlite --screenshot out.png --select-first
python3 ui/blackbox_app.py --db some.sqlite --screenshot out.png --select 2 --style dark --size 400x760
```

Screenshots render headless too: start `gtk4-broadwayd :7`, then run the app with
`GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 GSK_RENDERER=cairo`.
