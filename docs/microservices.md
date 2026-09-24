# Microservices design (draft)

Status: in progress (phases 0 and 1 done). [architecture.md](architecture.md) still describes
what runs today.

## Goals

1. **Practice.** Split the single `blackbox` process into small services that talk over IPC. Each
   one can be started, stopped, crashed and upgraded on its own.
2. **Isolation.** A bug in one source (say, a parser panic in `auditd.rs`) should restart only that
   source. Today it takes every thread down with it.
3. **Control center.** Add a service that can *act* on the machine, not just record it: list
   processes, terminate, kill, suspend or renice them, and start, stop or restart systemd units. The
   desktop app gains "Processes" and "Services" pages, in the spirit of Windows Task Manager and
   Control Panel.

Not a goal: to be lighter than today. Seven processes will cost more memory and wakeups than seven
threads. The budget below says how much more we accept, and phase 0 records the baseline so the
cost is measured rather than guessed.

## Where the seams already are

Every source reaches SQLite only through the public functions in `database.rs`. That gives us the
service boundary almost for free:

| Source | Writes | Reads back (at startup, for resume) |
|---|---|---|
| collector | `add_custom_event` | – |
| sampler | `add_sysstat_event` | – |
| journald | `add_journald` (row plus cursor in one transaction), `name_crash`, `set_state` | `get_state` (cursor), `last_ts("journald")` |
| auditd | `add_auditd_event` | `last_ts("auditd")` |
| pacman | `add_package_change` | `last_ts("packages")` |
| boot | `add_boot_event`, `mark_shutdown` | `boot_recorded` |
| trim | `trim_to_budget`, `checkpoint` | `logical_size_bytes` |

The rule that follows: **one service owns the database, and the IPC operations are these
functions.** Sources never see SQL. That keeps the single-writer property SQLite wants, and it
keeps the journald cursor atomic: the cursor travels in the same message as its row and is
committed in the same transaction, exactly as it is today.

## Services

```
                          ┌──────────────────────────────── blackbox.target ─┐
 netlink proc events ───► │ bb-proc      ─┐                                  │
 /proc, nvidia-smi ─────► │ bb-sampler   ─┤                                  │
 journalctl --follow ───► │ bb-journald  ─┤  store.sock   ┌──────────┐       │
 audit.log ─────────────► │ bb-auditd    ─┼─────────────► │ bb-store │ ──► blackbox.db
 pacman.log ────────────► │ bb-pacman    ─┤  (NDJSON)     │ + trim   │       │    ▲
 previous boot ─────────► │ bb-boot      ─┘               └──────────┘       │    │ read-only
                          │                                     ▲ records    │    │
 /proc, signals, ◄──────► │ bb-control ◄── control.sock ──┐     │ actions    │    │
 systemd D-Bus            │      └──────────────────────────────┘            │    │
                          └──────────────────────────────────────────────────┘    │
                                                           Blackbox app ──────────┘
                                                           (GTK) ──► control.sock
```

| Service | Comes from | Kind | Notes |
|---|---|---|---|
| `bb-store` | `database.rs`, trim thread | long-running, socket-activated | The only writer. Owns the schema, migrations, trim and checkpoints. |
| `bb-proc` | `collector.rs`, `socket.rs`, `netlink.rs`, `exit.rs` | long-running | Highest event rate. Keeps its 100 ms batching and sends one message per batch. |
| `bb-sampler` | `sampler.rs` | long-running | |
| `bb-journald` | `journald.rs` | long-running | Runs both followers. They could be split into two instances later (`bb-journald@warnings`, `@panics`). |
| `bb-auditd` | `auditd.rs` | long-running | Only starts when the audit log is readable (`ConditionPathIsReadWrite=` / `ExecCondition=`). |
| `bb-pacman` | `pacman.rs` | long-running, or a timer | Polls every 30 s today. A `.path` unit on `/var/log/pacman.log` would remove the polling entirely. That makes a nice exercise. |
| `bb-boot` | `boot.rs` | `Type=oneshot` | Runs once at login and exits. |
| `bb-control` | new | long-running, socket-activated | The control center backend. See below. |

## IPC

**Transport:** Unix domain sockets under `$XDG_RUNTIME_DIR/blackbox/` (`store.sock`,
`control.sock`), mode 0600. The server checks `SO_PEERCRED` and rejects any uid other than its
own.

**Encoding:** newline-delimited JSON, one message per line. `serde_json` is already a dependency,
Python's GTK app can speak it with the standard library, and you can debug by hand with
`socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/blackbox/store.sock`. A binary format (length-prefixed
bincode or protobuf) would be faster, but the whole service wakes fewer than 10 times a second today,
so that speed doesn't matter here.

**Messages** (the `bb-proto` crate, shared by every service):

```jsonc
// client → server, first line on every connection
{"hello": {"proto": 1, "service": "bb-journald"}}

// writes: one "op" per existing database.rs function, with a sequence number
{"seq": 41, "op": "add_journald", "ts": 1758700000, "priority": 3, "unit": "foo.service",
 "pid": 812, "message": "…", "severity": null,
 "cursor": ["journald.warnings", "s=…"], "dedup": false}
{"seq": 42, "op": "add_custom_batch", "events": [{"ts": …, "pid": …, …}, …]}

// reads: the same shape, answered with a value
{"seq": 43, "op": "get_state", "key": "journald.warnings"}

// server → client
{"ack": 41}
{"ack": 43, "value": "s=…"}
{"nack": 42, "error": "…"}          // bad message: logged and not retried
```

**Delivery:** a write counts as done only after `ack`, which the store sends *after* commit.
Everything sent but not yet acked stays in the client's buffer.

- **Store restarts:** the client reconnects with backoff (100 ms doubling to 5 s) and resends
  everything unacked in order. A duplicate is possible if the store committed but died before
  acking. The existing dedup covers journald. Other tables get a `(source, client_seq)` idempotency
  key: the store keeps the last committed seq per source in `state` and ignores anything at or below
  it.
- **Source restarts:** unacked events in memory are lost. Resume then works exactly as today, from
  the journal cursor, `last_ts`, or the audit log offset. `bb-proc` cannot resume, since kernel
  events are live only, and the same is true today.
- **Store down for long:** each client buffer is capped (say 10,000 events or 8 MB). When it fills,
  the oldest events are dropped and counted, and after reconnecting the client sends one
  `{"op": "gap", "service": …, "dropped": N, "from": ts, "to": ts}`. That gap is stored as a
  warning, so the recorder itself shows that it missed something.

**Socket activation:** `blackbox-store.socket` listens from login. Sources can connect before
`bb-store` is running: systemd starts it on the first connection, and the kernel queues the
connection until it accepts. This takes care of startup ordering with no `sleep`s and no retry
loops.

## systemd layout

```
blackbox.target                  WantedBy=default.target, Wants= every unit below
blackbox-store.socket            ListenStream=%t/blackbox/store.sock, SocketMode=0600
blackbox-store.service           the writer; Restart=always
blackbox-proc.service            After=/Wants=blackbox-store.socket
blackbox-sampler.service         ...
blackbox-journald.service
blackbox-auditd.service          + ExecCondition checking the audit log is readable
blackbox-pacman.service  (or .path + oneshot .service)
blackbox-boot.service            Type=oneshot, RemainAfterExit=yes
blackbox-control.socket/.service
```

Every service keeps today's `Nice=5`, `UMask=0077`, `NoNewPrivileges=yes` and accounting. Services
can now be hardened *individually*, which is the practical payoff of the split. For example,
`bb-sampler` gets `PrivateNetwork=yes` and `ProtectHome=read-only`, and `bb-pacman` gets
`ReadOnlyPaths=/`. Only `bb-store` needs write access to `~/.local/share/blackbox`. (User units
cannot use every sandboxing directive. Each one gets checked with `systemd-analyze --user security`
as we go.)

`systemctl --user stop blackbox.target` stops everything. `systemctl --user restart
blackbox-auditd` restarts one source and nothing else.

## Code layout

A Cargo workspace, so the compiler enforces the boundaries:

```
crates/
  bb-proto/     message types (serde), protocol version, socket paths
  bb-client/    connect, hello, send, ack tracking, reconnect, bounded buffer, gap reporting
  bb-store/     database.rs + trim + the socket server   (bin)
  bb-proc/      collector, socket, netlink, exit          (bin)
  bb-sampler/   (bin)
  bb-journald/  (bin)
  bb-auditd/    (bin)
  bb-pacman/    (bin)
  bb-boot/      (bin)
  bb-control/   (bin)
```

Adds `serde` with `derive`. No async runtime: each service is still a small blocking loop, which
matches the current code and keeps wakeups low.

## Control center (`bb-control`)

### What it offers

| Area | Operations | How |
|---|---|---|
| Processes | list (pid, ppid, name, user, state, CPU %, RSS, start time, cmdline, systemd unit from `/proc/<pid>/cgroup`), process tree | reads `/proc`, with CPU % computed from two samples of `/proc/<pid>/stat` |
| | terminate (SIGTERM, then offer SIGKILL after a timeout), kill (SIGKILL), suspend and resume (SIGSTOP/SIGCONT), renice | `pidfd_open` + `pidfd_send_signal`, so a pid reused between listing and clicking can't hit the wrong process; `setpriority` |
| Services | list user and system units with state; start, stop, restart, enable, disable | systemd over D-Bus (`org.freedesktop.systemd1`), or `systemctl` as a first version |
| Live view | subscribe to process list updates every 1 to 2 s | a `{"op": "subscribe", "what": "processes"}` stream on `control.sock`, active only while a client is subscribed |

The live view does **not** go through `bb-store`. Process lists are current state, not history,
and writing them to the database would blow the storage budget. `bb-control` samples only while the
app has a subscription open, so it keeps the "invisible when nobody's looking" property.

### Safety model

This is the first part of Blackbox that changes the machine, so it gets explicit rules:

1. **`bb-control` runs as your user, never as root.** The kernel then lets it signal only your own
   processes, and that is the main safety boundary. Anything owned by root or another user fails
   with `EPERM` and the UI says so.
2. **System-level actions go through polkit, not through a root helper.** Starting or stopping a
   *system* unit uses systemd's D-Bus API, which already asks polkit and shows the standard
   password prompt. Killing another user's or root's process is **out of scope for v1**. If it's
   wanted later, a small polkit-guarded helper can be designed as its own step.
3. **Refuse list, enforced in the service rather than the UI:** pid 1, kernel threads, the
   compositor and session leader (killing them logs you out), `bb-control` itself, and `bb-store`.
   Blackbox's own services are managed through the "Services" page and never by killing their
   pids.
4. **Every action is recorded.** `bb-control` sends `{"op": "add_control_action", …}` to the store
   (who, what, target pid/unit/name, result). It shows up in the timeline like any other event, so
   "I killed X at 14:02 and then Y broke" is visible.
5. **The UI confirms** destructive actions (terminate, kill, stop), showing the process name,
   cmdline and unit.

### New table

```sql
CREATE TABLE control (
    control_id INTEGER PRIMARY KEY,
    ts         TEXT NOT NULL,
    action     TEXT NOT NULL,   -- terminate, kill, suspend, resume, renice, unit_start, ...
    target     TEXT NOT NULL,   -- pid or unit name
    name       TEXT,            -- comm / unit description at the time
    result     TEXT NOT NULL    -- ok, eperm, esrch, refused, ...
);
```

It gets a `messages` row with `source = 'control'`, and trim treats it like every other table.

### UI changes

The GTK app gets two new pages, **Processes** (sortable table, search, tree toggle, right-click
actions) and **Services**. They talk to `control.sock`. The existing history pages keep reading
SQLite read-only, since routing reads through the store would add latency and a failure point for
no benefit.

## Baseline (phase 0)

Measured 2026-09-24 with [`tools/measure.sh 120`](../tools/measure.sh) over every process in the
`blackbox.service` cgroup, with the service up for 47 minutes:

| Process | PSS |
|---|---|
| `blackbox` | 3.3 MB |
| `journalctl` (warnings) | 16.1 MB |
| `journalctl` (panics) | 17.8 MB |
| `tail -F` audit.log | 0.2 MB |
| **total** | **37 MB** |

- CPU: 0.058% of one core (7 clock ticks in 120 s, so this is only precise to about ±0.01%)
- Context switches: 7.2 /s across all threads and children
- Event rate with the current kernel filter: 180 process events in 50 min, peak 75 /min. Before
  the filter (up to 2026-09-23 20:14), the peak was 1,709 /min, so a 10,000-event client buffer
  holds several minutes even at that rate.

The README's "5 MB" is the `blackbox` process alone. The two `journalctl` followers are 90% of
the real memory cost. That makes them the first thing to look at if memory ever matters, and they
stay the same after the split.

## Migration plan

Each phase leaves a working recorder. The same tests keep passing throughout, including the BPF
filter test.

| Phase | Work | Done when |
|---|---|---|
| 0 | Record the baseline: CPU, RSS, wakeups/s (`perf stat -e 'sched:sched_wakeup'` or `powertop`), events/day | numbers written into this doc |
| 1 | Put a `Store` trait in front of the `database.rs` functions and have the sources take `&dyn Store`. The only implementation is the current in-process one. | pure refactor, no behaviour change, tests green |
| 2 | `bb-proto` + `bb-client` + store socket server. Add a `RemoteStore` impl of the trait. One binary can still run everything in-process *or* against the socket (`BLACKBOX_STORE=remote`). | same events recorded in both modes over a day |
| 3 | Split out binaries one at a time, easiest first: **pacman → sampler → boot → auditd → journald → proc**. journald comes late because of cursor atomicity, proc last because of rate and netlink. | each split service survives `kill -9` of itself and of `bb-store` with no loss beyond the documented cases |
| 4 | systemd units, socket activation, `blackbox.target`, per-service hardening, `install.sh` / `uninstall` | clean install from scratch; `systemd-analyze --user security` score per unit noted |
| 5 | Failure drills: kill store during a burst, fill the buffer, restart sources mid-journal, reboot mid-write | written up with results |
| 6 | `bb-control` read-only: process list + subscribe, Processes page in the app | matches `ps`/`top` within one sample |
| 7 | Control actions + refuse list + `control` table + confirmations; Services page | every action appears in the timeline |
| 8 | Measure again and compare with phase 0 | cost stated honestly in the README |

**Target budget after the split:** CPU under 0.5% of one core; the Rust services together under
15 MB PSS (today's `blackbox` is 3.3 MB); helper processes (`journalctl`, `tail`) unchanged; context
switches under 20 /s. Any single Rust service above 5 MB PSS gets a look.

## Decisions

All four settled on 2026-09-24 as recommended: a Cargo workspace; `bb-pacman` as a `.path` unit;
control center v1 limited to your own processes plus systemd units through polkit, with no root
helper; new pages in the existing GTK app.

## Open questions (resolved)

1. **Workspace or one crate with many `[[bin]]`s?** A workspace enforces boundaries (a source
   *cannot* import `rusqlite`). One crate is less ceremony. Recommendation: workspace, since
   boundaries are the point of the exercise.
2. **`bb-pacman` as a `.path` unit or a long-running poller?** Recommendation: `.path`. It's a good
   systemd lesson, and it removes a wakeup every 30 s.
3. **How far the control center reaches in v1.** Recommendation: your own processes plus systemd
   units through polkit. No root helper.
4. **Where the control center UI lives: the existing GTK app, or a separate app?** Recommendation:
   the same app with new pages, so the timeline and the actions sit side by side.
