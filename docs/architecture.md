# Architecture

## Data flow

```
 kernel proc events ──(BPF filter)──► collector ─┐
 /proc, nvidia-smi ──────────────────► sampler ──┤
 journalctl --follow (warning+) ─────► journald ─┤
 journalctl --follow (panics) ───────► journald ─┼──► database.rs ──► blackbox.db ◄── Blackbox app
 tail -F /var/log/audit/audit.log ───► auditd ───┤   (one connection,   (SQLite,       (read only,
 /var/log/pacman.log (every 30 s) ───► pacman ───┤    WAL, one          7.5 GB ring)    only while open)
 previous boot's last journal entry ─► boot ─────┘    transaction per
                                                      event)
```

One process (`blackbox`, a systemd user service) runs the collector on the main thread, plus
threads for the sampler, two journald followers, the auditd follower, the pacman reader, the trim
job, and a one-off boot check at startup. The viewer is a separate
GTK app that opens the database read-only and never talks to the service.

## Components

### Collector (`collector.rs`, `socket.rs`, `netlink.rs`)

Subscribes to the kernel's process connector over a raw netlink socket and receives exec and exit
events.

- **What is stored** (`exit.rs`, one rule shared by the filter and the collector):

  | Exit | Stored as |
  |---|---|
  | killed by SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV or SIGSYS (a crash) | error |
  | exit status 101 (a Rust panic) | error |
  | any other exit status except 0 and 1 | warning |
  | status 0 or 1, SIGTERM, SIGHUP, SIGINT, SIGPIPE, SIGKILL, and a shell's 128+N report of those (129, 130, 137, 141, 143) | not stored |

  Status 1 is what `grep`, `pgrep` and `modprobe` return for "not found": of 3,270 exits recorded on
  this machine, status 0 and 1 were 99.7%, and every status-1 exit came from `pgrep` or `modprobe`.
- **Kernel-side filter.** A classic BPF program attached to the socket applies that rule in the
  kernel, so only exec events (needed for names) and exits worth storing ever wake the process. Fork
  events, thread exits and clean exits are dropped before they are queued. A unit test runs the
  filter through a small BPF interpreter for all 65,536 wait statuses and checks it agrees with
  `exit::severity`.
- **Batching.** After each event the collector sleeps `BLACKBOX_BATCH_MS` (100 ms), then drains the
  queue in one go. This takes wakeups to about 7 per second.
- **Process names.** The exit event carries no name and `/proc/<pid>` is usually gone by then. So
  the collector fills a pid to name table at startup and on every exec (reading `/proc/<pid>/comm`),
  and looks the name up on exit. If that fails, the summary falls back to the parent's name:
  `? (pid 4242, child of bash)`. A crash that dumps core is named afterwards from systemd-coredump's
  journal entry (`COREDUMP_PID`, `COREDUMP_COMM`, `COREDUMP_EXE`), which also fills in `custom.exe`
  with the full executable path.
- **Overflow.** If the kernel queue overflows (`ENOBUFS`) it logs it and carries on.

### Sampler (`sampler.rs`)

Reads `/proc/stat`, `/proc/meminfo`, `/proc/loadavg` and `/proc/diskstats` every 10 s. The
`sysstat` package is not used. The same numbers come from `/proc` with no dependency. GPU data
comes from `nvidia-smi` every 30 s, and is skipped while the card is runtime-suspended so it never
wakes the GPU. `nvidia-smi` gets 5 s to answer and is killed after that, so a hung driver cannot
stall the sampler. A `messages` row is written only when a threshold is crossed:

| Metric | Warning | Error |
|---|---|---|
| Memory used | 85% | 95% |
| CPU busy | 90% | – |
| Load average | above 2 × cores | – |
| GPU temperature | 85 C | – |
| GPU memory | 90% | – |

### journald (`journald.rs`)

Two `journalctl --follow --output=json` processes:

| Follower | Filter | Stored as |
|---|---|---|
| warnings | `--priority=warning` | priority 0 to 3 `error`, 4 `warning` |
| panics | `--priority=notice..info --grep='panicked at'` | `error`: Rust panics are logged at info level, below the warning filter |

- **Exact resume.** Each stored entry saves its journal cursor in the `state` table, in the same
  transaction as the row, and a restart continues with `--after-cursor`. Without a cursor (the first
  run) it resumes by time, and only the overlapping second is checked for duplicates, so identical
  messages logged in the same second are kept as the real repeats they are. If journald has vacuumed
  past the saved cursor, the follower forgets it and resumes by time.
- **Clean text.** Terminal colour codes that some services log verbatim are stripped.
- **Routine noise.** A short list of messages that look alarming but are routine here (the kernel's
  "watchdog did not stop!" at every shutdown) is stored as `info` instead of an alert.
- systemd-coredump entries also name the matching crash in `custom` (see Process names above), and
  the viewer shows their stack trace next to the crash.

### auditd (`auditd.rs`)

Follows the audit log with `tail -F` and keeps only low-volume, high-value records:

| Record | Stored as |
|---|---|
| `USER_AUTH`, `USER_LOGIN`, `USER_ACCT` with `res=failed` | warning |
| successful `USER_AUTH`, `USER_LOGIN`, `USER_CMD` (sudo) | row in `auditd`, no alert |
| `SYSCALL` with a `bb_*` key (`deploy/blackbox.rules`) | warning |
| `ANOM_*` (crashes, promiscuous mode) and `AVC` denials | error |

On restart it binary-searches the log for the first record newer than the last one stored and
follows from there, so events during downtime are picked up without reading the whole file (records
that rotated into `audit.log.1` meanwhile are not). If the log is missing or unreadable it checks
again every 30 s. `deploy/setup-auditd.sh` installs the
rules, caps the log at 4 x 500 MB and makes it readable by group `wheel`.

### Package changes (`pacman.rs`)

Reads `/var/log/pacman.log` and stores one row per transaction: the pacman command, every package
changed, and a one-line summary with the packages that most often break a machine (kernel, NVIDIA,
Mesa, systemd, glibc, mkinitcpio, GRUB, microcode, firmware) listed first, with their versions:
`pacman: upgraded 83, installed 1 · mesa 1:26.2.2-1 → 1:26.2.3-1, linux 7.2.4.arch1-2 → 7.2.6.arch2-1, …`.
The log only changes while pacman runs, so it is checked every 30 s by size instead of being
followed by a process. The first run imports the whole log (139 transactions on this machine).
The viewer lists the transactions from the week before any event it shows.

### Boots (`boot.rs`)

A freeze, a kernel panic or a power loss kills the collector with everything else, so nothing
records it at the time. Once per boot, at startup, the collector records that the system started
(with the kernel release), and reads the previous boot's last journal entry. A clean shutdown always
ends with journald's "Journal stopped" (`MESSAGE_ID` `d93fb3c9c24d451a97cea615ce59c00b`); anything
else is stored as an `error` at the moment the previous boot stopped, so the viewer shows the minutes
before it. A last entry from suspend ("Filesystems sync", "PM: suspend") is described as a suspend
that never resumed. On this machine 66 of 73 earlier boots ended cleanly; of the other 7, two stopped
while suspending.

### Database (`database.rs`)

- One shared connection, WAL mode, `synchronous=FULL`. Each event is one transaction that writes
  the source row and its `messages` row together. FULL means every commit is on disk before the
  next one: with NORMAL, the last ~30 s (`vm.dirty_expire_centisecs`) would sit in the page cache and
  be lost in a hard freeze or kernel panic, which is exactly the moment a flight recorder is for.
  Steady-state writes are about one every 10 s, so the extra fsync is negligible.
- **Ring buffer.** Every 5 minutes the trim job measures the data actually in use (pages minus free
  pages). Over the budget, it deletes the oldest tenth of the time span from every table together,
  repeating until usage is under 90% of the budget. The newest time is capped at now: without that,
  one row stamped in the far future (a clock that was once wrong) stretched the span so much that
  the first pass deleted every real row (the test reproduces it: 0 of 3,000 recent rows kept). Freed pages are reused, so the file plateaus
  instead of growing.
- **Trimming must not block writers.** The delete runs in chunks of 1,000 rows with the lock released
  between chunks, and the oldest/newest timestamp lookups use one `MIN` or `MAX` per subquery (SQLite
  scans the whole index when both are in one `SELECT`). Stress test (`cargo test --release
  trim_does_not_stall_writers -- --ignored`): the worst write during a trim went from 1,217 ms
  (one big delete) to 2.9 ms on a 126 MB database and 3.6 ms on a 635 MB one, and it does not grow
  with size. Deletion runs at about 18 MB/s in the background. Note that the test's default
  location, `/tmp`, is a tmpfs where fsync costs nothing; run it with `TMPDIR` on a real disk to see
  the cost of `synchronous=FULL`. On this machine's NVMe (ext4), 1.2 M rows trimmed in 8.7 s instead
  of 7.4 s with NORMAL, and the worst concurrent write stayed at 7.4 ms (8.6 ms with NORMAL).
- `state` holds small key-value settings the service keeps between runs (the journal cursors).
- Timestamps are UTC text (`YYYY-MM-DD HH:MM:SS`). They sort correctly, and every `ts` column is
  indexed.

### Viewer (`ui/`)

A native GTK4 and libadwaita app, `io.github.hawkosm.Blackbox`, listed in the application menu. It is
read-only, refreshes every 5 s, and runs only while its window is open. The layout is an adaptive
split view that collapses to single pages below 720 px.

- **Event list:** grouped by day, filtered by Problems / Errors / Everything, sources and time range,
  with search. Runs of the same message fold into one row with a count (`×218`).
- **Overview:** error and warning counts, current CPU, memory and GPU, and a chart of the load with a
  tick for every notable event.
- **One event:** a chart of CPU, memory and GPU at ±2 min, ±10 min or ±1 h (with a table view), the
  crash's stack trace from systemd-coredump, the full row with readable field names, and every event
  around it with its offset. Copy puts all of that on the clipboard as a plain-text report.
- **Go to a time:** the clock button opens the same view for any moment, even one where nothing was
  recorded (a freeze, for example).

Chart colours are one axis (percent) with fixed hues for CPU, memory and GPU, checked for
colour-blind separation against both the light and the dark card surface.

## Storage budget

The whole system stays within 10 GB:

| Part | Limit | Set by |
|---|---|---|
| Database | 7.5 GB | `BLACKBOX_MAX_MB` |
| Audit log | 2 GB (4 x 500 MB) | `deploy/setup-auditd.sh` |
| Headroom (WAL, trim slack) | 0.5 GB | |

journald's own journal is managed by systemd and is not counted; only its warnings and worse are
copied into the database. At the volume recorded now (crashes, plus one system sample every 10 s
at roughly 1 MB a day) the database will not come near its cap for years. Storing every whole-process
exit (about 22 a second, at about 180 bytes each) would use roughly 340 MB a day, so about three weeks of
history in 7.5 GB.

## Schema

See `docs/database.drawio`. `messages` is an alert index: one row per notable event, pointing at
the full row in the matching source table through `(source, ref_id)`. That pointer is enforced by
the application, not by a foreign key, because a foreign key cannot target several tables.

## Design decisions

| Decision | Reason |
|---|---|
| Crashes and failures only, not every exec or exit | Every exec and exit is about 100 events a second. Auditd and the `/proc` sampler already cover "what ran" and process churn. |
| Crash signals 4, 5, 6, 7, 8, 11, 31 | The signals that dump core. SIGTERM and SIGHUP are routine shutdowns. SIGKILL is mostly benign here (GNOME's image loader kills its helpers), and real OOM kills are logged by the kernel to journald. |
| No tail of every exit after a crash | It was dropped: two crashes stored 3,268 exits in 20 minutes, 99.7% of them status 0 or 1. Failures anywhere are now stored all the time instead, which catches a cascade without the noise. |
| Crash names from systemd-coredump | Batching keeps wakeups low but loses the name of a process that dies within ~100 ms. The coredump entry has the name and path anyway, at no extra cost. |
| No summary or roll-up table | An hourly exec and exit count catches almost nothing that load, auditd and journald do not. |
| Own `/proc` sampler | Avoids a package and a second data format for the same numbers. |
| Native GTK app, not a web page | It should exist only while open, with no server or browser. |
| pacman log read every 30 s, not followed | It changes only while pacman runs. A size check costs nothing; a `tail` process would sit there all day. |
| Unclean shutdowns detected at the next boot | Nothing can record a freeze at the time it happens. The journal's last entry, read once per boot, is enough to tell. |

## Reading an exit code

`exit_code` is a raw wait status: the low 7 bits are the signal that killed the process (0 if none),
bit 7 means a core was dumped, and `exit_code >> 8` is the normal exit status.

| Value | Meaning |
|---|---|
| 256 | exited with status 1 |
| 25856 | exited with status 101 (a Rust panic) |
| 11 | killed by SIGSEGV |
| 139 | SIGSEGV, core dumped |
| 15 | killed by SIGTERM |
| 36608 | exited with status 143 (a shell reporting that its child got SIGTERM) |

## Performance

Measured on this machine (22 cores) with the installed service, using the service's cgroup counters,
which include the `journalctl` child:

| | Before tuning | Now |
|---|---|---|
| CPU, whole service | 0.34% of one core | **0.14% of one core** (0.006% of the machine) |
| Memory | 5.4 MB | **5.1 MB** |
| Collector wakeups | 98 to 136 per second | **8.5 per second** |

Other costs measured: one `nvidia-smi` call is about 13 ms of CPU, which is why the GPU is sampled
every 30 s. The database grows by roughly 180 bytes per stored event.

To measure again:

```bash
systemctl --user show blackbox -p CPUUsageNSec -p MemoryCurrent
```

Take two readings a minute apart and divide the CPU difference by the elapsed time.

The trade-off: with batching, a process that dies within about 100 ms of starting loses its name
lookup (measured: 92% of such exits unnamed at 100 ms, versus only forked-without-exec subshells at
0 ms). Crashes that dump core get their name back from systemd-coredump. For the rest, lower
`BLACKBOX_BATCH_MS` to trade wakeups for names (0 ms gives about 50 wakeups per second, about 0.1% of
one core more).

## Known gaps

- auditd must be set up once with `sudo bash deploy/setup-auditd.sh`. Checked live: all 16 rules
  load, and a denied write to `/etc/fstab` is stored as a `bb_boot` warning within 2 s. auditctl
  prints "Old style watch rules are slower" for each `-w` rule; that is a deprecation notice, not an
  error. After downtime the follower resumes from the current log only: records that rotated into
  `audit.log.1` while the service was stopped are missed.
- A failure that exits with status 1 is not recorded: it is indistinguishable from the polling
  helpers that return 1 for "not found" hundreds of times a minute.
- Very short-lived processes that fail without dumping core usually have no name, only their
  parent's.
- If the kernel queue overflows under a burst, events are dropped and the collector logs it.

## Tests

`cargo test` covers the journald and audit parsers (including the coredump fields, cursors and
colour codes), the exit rule, the kernel filter against it for every wait status, naming a crash
from its coredump entry, the pacman parser, clean and unclean boot endings, resuming the audit log
after downtime, the journal cursor not shifting row ids, the database write path, and the ring-buffer
trim, including one far-future row (without the cap, that test keeps 0 of 3,000 recent rows).
Beyond that, the whole service was exercised against a throwaway database: a real crash, a real
journald warning, fake audit lines, a CPU burn, a restart with backfill, and a copy of an older
database to test the schema migration.
