# Architecture

How BlackBox is built. Related docs: tables and columns in [../database.md](../database.md),
building and testing in [../development.md](../development.md), adding a source in
[../adding-a-source.md](../adding-a-source.md), reasons for each design choice in
[../decisions/](../decisions/).

| File | Covers |
|---|---|
| [writer.md](writer.md) | Source groups, supervisor, the writer thread, the record trait |
| [kernel.md](kernel.md) | Collector: exec and exit events, the kernel filter, exit codes |
| [sampler.md](sampler.md) | Sampler, power, sensors, power profiles, temperature spikes |
| [logs.md](logs.md) | journald, auditd, package changes, boots |
| [storage.md](storage.md) | SQLite settings, ring buffer, storage budget |
| [viewer.md](viewer.md) | The GTK app |
| [operations.md](operations.md) | Performance, known gaps, tests |

## Data flow

```
 kernel proc events ──(BPF filter)──► collector ─┐
 /proc, /sys, nvidia-smi ────────────► sampler ──┤
 D-Bus (busctl) ─────────────────────► profiles ─┤
 CPU temperature, 1 s ───────────────► spike ────┤
 journalctl --follow (warning+) ─────► journald ─┤
 journalctl --follow (panics) ───────► journald ─┼──► writer ──────► blackbox.db ◄── viewer
 tail -F /var/log/audit/audit.log ───► auditd ───┤   (one thread,     (SQLite)      (read only)
 package history (every 30 s) ───────► packages ─┤    one transaction
 previous boot's last journal entry ─► boot ─────┘    per burst)
```

## Processes and threads

`blackbox` is one systemd user service. The collector runs on the main thread. Every other source
runs on its own thread, named after it. The database has two more threads: the writer and the trim
job.

The viewer is a separate GTK app. It opens the database read-only and never talks to the service.

## Source layout

```
src/
  sources.rs                 overview and run()      sources/supervise.rs   the supervisor
  sources/
    kernel/collector         the kernel pushes events (netlink); main thread
    follow/journald, auditd  lines of a long-running process (journalctl, tail -F)
    poll/sampler, profiles,
         spike, packages     read on a timer
    once/boot                read once at start-up
    routine.rs               routine messages, shared by journald and auditd
  database.rs                what the database offers
  database/                  knows no source
    connection.rs  writer.rs  sql.rs  time.rs  severity.rs  scheme.rs  state.rs  retention.rs
```

Each folder has a file of the same name next to it (`follow.rs` beside `follow/`, not
`follow/mod.rs`). That file lists the folder's files and what it exports. A source folder has
`parse.rs` (what a line is), `follower.rs` or `poller.rs` (how it is read), `store.rs` (its
records and their SQL), and anything else it needs.
