# Architecture

How BlackBox works, source by source, with what was measured. Companions: the tables and columns
in [database.md](database.md), building and testing in [development.md](development.md), the
recipe for a new source in [adding-a-source.md](adding-a-source.md), and the reasoning behind
each design decision in [decisions/](decisions/).

## Data flow

```
 kernel proc events ──(BPF filter)──► collector ─┐
 /proc, nvidia-smi ──────────────────► sampler ──┤
 journalctl --follow (warning+) ─────► journald ─┤
 journalctl --follow (panics) ───────► journald ─┼──► writer ──────► blackbox.db ◄── Blackbox app
 tail -F /var/log/audit/audit.log ───► auditd ───┤   (one thread,      (SQLite,       (read only,
 package history (every 30 s) ───────► packages ─┤    one transaction   50 GB ring)     only while open)
 previous boot's last journal entry ─► boot ─────┘    per burst)
```

One process (`blackbox`, a systemd user service) runs the collector on the main thread, plus a
thread each, named after it, for the sampler, the two journald followers, the auditd follower, one
reader per package manager found and the one-off boot check, and two for the database: the writer
and the trim job. The viewer is a separate GTK app that opens the database read-only and never
talks to the service.

## Sources and the writer

The code is split by job, and the sources by how their data arrives:

```
src/
  sources.rs                 the overview, run()        sources/supervise.rs   the supervisor
  sources/
    kernel/collector         the kernel pushes events (netlink); runs on the main thread
    follow/journald, auditd  a long-running process's lines (journalctl, tail -F)
    poll/sampler, packages   read on a timer (every 10 s, every 30 s)
    once/boot                read once at start-up
    routine.rs               routine messages, shared by journald and auditd
  database.rs                the list of what the database offers
  database/                  knows no source
    connection.rs  writer.rs  sql.rs  time.rs  severity.rs  scheme.rs  state.rs  retention.rs
```

Each folder has a file of the same name next to it (`follow.rs` beside `follow/`, Rust's 2018
layout instead of `follow/mod.rs`). That file is the folder's table of contents: what it is for,
the list of its files, and what it offers; the code is in the files, each named after the one job
it does. A source, for example `follow/auditd/`, has `parse.rs` (what a line is), `follower.rs`
(how it is followed), `store.rs` (its records and their SQL) and what else it needs
(`resume.rs`).

- **One loop per group** (the Strategy pattern). `follow/follower.rs` holds the follow loop once:
  run the process, hand over every line, run it again when it ends. A followed source implements
  `Follower` and says only what to run (`command`, resuming after what is stored), what one line
  means (`line`) and what an end means (`ended`: journald forgets a cursor the journal no longer
  has). `poll.rs` does the same for `Poller` (`every`, `poll`). Before, journald and auditd
  each had their own copy of the follow loop.
- **A supervisor runs every source but the collector** on a thread named after it. A source that
  panics is reported and run again with the state it had, after 30 s, doubling while it keeps
  panicking up to 30 min (back to 30 s once it has run fine for 10 min), so one that always
  panics does not fill the journal. Before, its thread died and the recorder carried on without
  it: a sampler panic would have meant no CPU, memory or temperature row for the rest of the run. A source that returns (boot) is done. The collector
  stays on the main thread: when its netlink socket fails the process exits, and systemd starts
  it again. It subscribes before any other source starts, so a kernel that refuses the
  subscription stops the process before anything has been queued that would be lost with it.

Sources never commit anything themselves. A source builds a record, `database::write(record)`
queues it, and one writer thread stores it (producer–consumer):

```
 collector ─┐
 sampler ───┤
 journald ──┤  record   ┌─────────┐  everything queued   ┌────────┐
 auditd ────┼─────────► │ channel │ ───────────────────► │ writer │ ──► one transaction ──► blackbox.db
 packages ──┤           │ 256     │  so far              │ thread │
 boot ──────┘           └─────────┘                      └────────┘
 start-up reads (last_ts, get_state, boot_recorded, last_package_ts) ──► a read-only connection
```

- **A record stores itself.** `Record` is a trait with one method, `store(&self, &Connection)`,
  and each source implements it for its own records, next to their SQL. The writer only calls
  `store` (the Command pattern), so it never names a source, and a new source adds a record in its
  own folder without touching `database/`. An earlier version had one `enum Write` with a variant
  per record and a `match` in the writer; it was dropped because every new record meant editing
  `database/` too. A trait is the right fit when new kinds keep coming and each has one operation.
- **Only the writer can store.** `store` needs a `Connection`, and only the writer thread has one
  that may write: sources get a second, read-only connection for their start-up checks through
  `database::read()`, so a write there fails instead of going past the writer.
- **A followed process never outlives its run.** It is stopped and reaped when the run ends, even
  by a panic in the source, so restarts leave no `journalctl` or `tail` behind.
  Its lines are read as bytes and decoded leniently: a line that is not valid UTF-8 is handed on
  with `�` in it, instead of stopping the read and restarting the process at the same line forever.
- **One commit per burst, without waiting.** The writer blocks for one write, then takes whatever
  else is already queued and commits it all in one transaction. A lone event commits at once, as
  before, so `synchronous=FULL` still puts every event on disk before the next can be lost. A timed
  batch ("commit every 500 ms") was rejected: it would leave the last half second in memory at
  exactly the moment a flight recorder is for. Measured on a fresh database, 45 s each, counting
  `fsync`/`fdatasync` calls with an `LD_PRELOAD` shim: 207 before, 37 to 47 after, with the same
  rows stored (most of the difference is the first import of 162 pacman transactions).
- **A record is what must commit together.** `Journal` carries its cursor, so the row and the
  cursor land in one transaction. `Tick` is one sampler tick (the system sample, power, every
  sensor and the top processes), one commit where it was four, so the steady state dropped from
  about 3.3 commits per 10 s to 1. Updates are records too (`CrashName`, `MarkShutdown`,
  `DemoteRoutine`, `SetState`).
- **One failure does not sink a batch.** Each write runs in its own savepoint: one that fails is
  logged and rolled back alone (a `Tick` as a whole), and the rest of the batch is kept. A sensor
  value that is not a number is skipped before it gets there: SQLite stores NaN as NULL, which the
  column rejects, and it would take the tick's system sample and alert down with it.
- **De-duplication runs in the writer.** Whether a package transaction or a journal line is already
  stored is checked inside the writer's transaction, which also sees the batch's own earlier rows.
- **Sources remember where they are.** A source reads where it stopped from the database once, at
  start-up, and keeps it in memory after that: the journald followers their cursor and newest
  time, the audit follower its newest record. Reading it back from the database after a
  `journalctl` or `tail` restart could return a position older than rows still in the queue, and
  store them twice.
- **Two connections.** The writer's, the only one that may write, which the trim job also uses
  between the writer's batches (its deletes run in 1,000-row chunks, so the writer waits at most one
  chunk); and a read-only one for every start-up read, through `database::read()`. WAL lets the
  reader see each committed batch next to the writer.
- **The queue is small and loses nothing while running.** 256 slots, each a pointer to a boxed
  record; a larger burst makes the sources wait while the writer commits. When the process ends on
  its own (the collector stops or its socket fails) it first waits, at most 5 s, for the queue to be
  committed (`database::drain`). What is still queued when the process is killed is lost: the writes
  of one commit (milliseconds), about what was lost before, when a source was killed in the middle
  of its own write.
- **Failure.** If the writer thread panics, it exits the process and systemd restarts the service,
  as it does when the collector's netlink read fails. Otherwise every source would queue into
  nothing without saying so.

The package readers use the same idea one level down: `packages::History` (`name`, `path`, `poll`,
`history`) is implemented once for a text log, generic over its line `Parser`
(`sources/poll/packages/text_log.rs`), and once for dnf's database (`sources/poll/packages/dnf.rs`).
A text log that is missing for a moment (rotation) counts as "nothing new", not as an error. The
reader and `--check` only see `Box<dyn History>`, so adding a package manager is one reader plus
one line in `detect()`.

## Components

### Collector (`sources/kernel/collector/`)

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
- **In code, three steps:** `decode` reads the raw netlink message into an `Event` (`Exec` or
  `Exit`; the one `unsafe` read), `remember` keeps the name of a new program, and `exit_record`
  turns an exit worth keeping into an `Exit` record.

### Power (`sources/poll/sampler/power.rs`)

Every sampler tick also writes one `power` row: `on_battery`, `battery_pct`, `cpu_watt`, `gpu_watt`
and `battery_watt`. CPU watts are the difference of the RAPL package counter
(`/sys/class/powercap/intel-rapl:0/energy_uj`) over the elapsed time, wrap-around included; a gap
over 30 s (suspend) is dropped rather than averaged in. The counter is root-only, so
`deploy/setup-sensors.sh` opens it to the admin group (`wheel`, `adm` or `sudo`); without it
`cpu_watt` stays NULL. `gpu_watt` is filled only on the ticks where `nvidia-smi` is polled.
`battery_watt` is the whole machine's draw and is set only while discharging.

### Sensors (`sources/poll/sampler/sensors.rs`)

Every tick also reads all of `/sys/class/hwmon` (no root needed): every temperature (each CPU
core and the package, both NVMe drives, the DRAM modules' own sensors, Wi-Fi, ACPI), every fan
and any power input, and the RAPL domains the package row does not cover (cores, uncore, DRAM,
and `psys`, the whole platform). One row per reading goes into `sensors(ts, kind, name, value)`,
named `chip/label`; chips that share a name (two NVMe drives) get their device in brackets. The
viewer charts the CPU package, the hottest NVMe drive and platform power, and lists every
reading under "Everything being logged". RAPL watts need `deploy/setup-sensors.sh`, which
`install.sh` offers to run. About 27 readings are stored per tick on the development machine, a
few hundred thousand rows a day.

### Sampler (`sources/poll/sampler/`)

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

### journald (`sources/follow/journald/`)

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
- **Clean text.** Terminal colour codes that some services log verbatim are stripped, and empty
  messages (the kernel logs bare continuation lines at error priority) are skipped.
- **Routine noise.** `src/sources/routine.rs` lists messages that look alarming but are routine on
  any Linux machine: the kernel's "watchdog did not stop!" at every shutdown, `sudo -n` probes (sudo
  logs "a password is required" at alert level), password prompts that timed out or were
  cancelled, and similar. They are stored as `info` instead of an alert. Each rule matches text
  the emitting program always prints, never a user, host, device address or path, so it holds on
  every machine; a fault of one machine is fixed there, not listed. When the list changes, rows
  already stored are re-rated once at startup. On the development machine the first list moved
  531 of 1,601 errors to `info`, 31% of the last week's.
- systemd-coredump entries also name the matching crash in `custom` (see Process names above), and
  the viewer shows their stack trace next to the crash.

### auditd (`sources/follow/auditd/`)

Follows the audit log with `tail -F` and keeps only low-volume, high-value records:

| Record | Stored as |
|---|---|
| `USER_AUTH`, `USER_LOGIN`, `USER_ACCT` with `res=failed` | warning |
| successful `USER_AUTH`, `USER_LOGIN`, `USER_CMD` (sudo) | row in `auditd`, no alert |
| `SYSCALL` with a `bb_*` key (`deploy/blackbox.rules`) | warning; module loads in the first 3 minutes after boot are `info` (about 30 per boot, the kernel bringing up drivers) |
| `ANOM_*` (crashes, promiscuous mode) and `AVC` denials | error, except promiscuous mode on a bridge port (Docker, Podman, libvirt, VM taps) or leaving it, which is info |

Reading a line is two steps: `classify` says what the record is (`Found`: a failed login, a
login, a hit on one of our rules, promiscuous mode, an anomaly), and `Found::alert` says how much
it matters and what the viewer shows. The rule that a module load in the first 3 minutes after
boot is `info` is a `match` on `RuleHit { key: "bb_modules", .. }` there.

On restart it binary-searches the log for the first record newer than the last one stored and
follows from there, so events during downtime are picked up without reading the whole file (records
that rotated into `audit.log.1` meanwhile are not). If the log is missing or unreadable it checks
again every 30 s. `deploy/setup-auditd.sh` installs the rules (only the watches whose paths exist
on this machine), caps the log at 4 x 500 MB and makes it
readable by the admin group: `wheel`, `adm` or `sudo`, whichever the user is in.

### Package changes (`sources/poll/packages/`)

Stores one row per transaction: the command, every package changed, and a one-line summary with the
packages that most often break a machine (kernel, NVIDIA, Mesa, systemd, glibc, initramfs tools,
GRUB, shim, microcode, firmware) listed first, with their versions:
`pacman: upgraded 83, installed 1 · mesa 1:26.2.2-1 → 1:26.2.3-1, linux 7.2.4.arch1-2 → 7.2.6.arch2-1, …`.
A reader runs for every history that exists. Each one turns its format into the same change lines
(`upgraded name (old -> new)`):

| Reader | History | End of a transaction |
|---|---|---|
| `pacman.rs` | `/var/log/pacman.log` | `transaction completed` / `failed` |
| `apt.rs` | `/var/log/apt/history.log` (local time) | `End-Date:` |
| `zypper.rs` | `/var/log/zypp/history` (local time) | the next `command` line, or 5 min without a new line |
| `dnf.rs` | dnf5 `transaction_history.sqlite` or dnf4 `history.sqlite`, opened read-only | `dt_end` set |

dnf5 writes no text log, and dnf4's `dnf.rpm.log` has no transaction boundaries, so dnf's history
database is read instead. Both are world-readable. An upgrade is two rows there (the new version,
and the one it replaced), which are paired back into one line. The text logs only change while the
package manager runs, so they are checked every 30 s by size. The database is queried only when
its modification time (or its `-wal` file's) changes.

The first run imports the whole history (139 transactions on this machine). After a restart, each
reader starts again from the newest second already stored for its manager, and skips any
transaction already stored with the same time and changes. Two transactions in the same second
(apt and dnf both produce them) are therefore neither lost nor stored twice.
The viewer lists the transactions from the week before any event it shows.

### Boots (`sources/once/boot/`)

A freeze, a kernel panic or a power loss kills the collector with everything else, so nothing
records it at the time. Once per boot, at startup, the recorder records that the system started
(with the kernel release), and reads the previous boot's last journal entry. A clean shutdown always
ends with journald's "Journal stopped" (`MESSAGE_ID` `d93fb3c9c24d451a97cea615ce59c00b`); anything
else is stored as an `error` at the moment the previous boot stopped, so the viewer shows the
minutes before it. A last entry from suspend ("Filesystems sync", "PM: suspend") is described as a
suspend that never resumed. On this machine 66 of 73 earlier boots ended cleanly; of the other 7,
two stopped while suspending.

After a clean shutdown, errors and warnings from its last 30 s are relabelled `info` with "(during
shutdown)": they are programs being killed as the session is torn down (on 2026-09-24 Hyprland and
Spotify both aborted during a poweroff), not failures. This runs at every start and is idempotent.

### Database (`database/`)

- WAL mode, `synchronous=FULL`. Only the writer thread writes, and the trim job deletes (see
  "Sources and the writer"); sources read through a second, read-only connection. A source row and
  its `messages` row are always written together. FULL means every commit is on
  disk before the next one: with NORMAL, the last ~30 s (`vm.dirty_expire_centisecs`) would sit in
  the page cache and be lost in a hard freeze or kernel panic, which is exactly the moment a flight
  recorder is for. Steady-state writes are one commit every 10 s, so the extra fsync is negligible.
- **Ring buffer.** Every 5 minutes the trim job measures the data actually in use (pages minus free
  pages). Over the budget, it deletes the oldest tenth of the time span from every table together,
  repeating until usage is under 90% of the budget. The newest time is capped at now: without that,
  one row stamped in the far future (a clock that was once wrong) stretched the span so much that
  the first pass deleted every real row (the test reproduces it: 0 of 3,000 recent rows kept).
  Freed pages are reused, so the file plateaus instead of growing.
- **Trimming must not block writers.** The delete runs in chunks of 1,000 rows with the lock released
  between chunks, and the oldest/newest timestamp lookups use one `MIN` or `MAX` per subquery (SQLite
  scans the whole index when both are in one `SELECT`). Stress test (`cargo test --release
  trim_does_not_stall_writers -- --ignored`): the worst write during a trim went from 1,217 ms
  (one big delete) to 2.9 ms on a 126 MB database and 3.6 ms on a 635 MB one, and it does not grow
  with size. Deletion runs at about 18 MB/s in the background. Note that the test's default
  location, `/tmp`, is a tmpfs where fsync costs nothing; run it with `TMPDIR` on a real disk to see
  the cost of `synchronous=FULL`. On this machine's NVMe (ext4), 1.2 M rows trimmed in 8.7 s instead
  of 7.4 s with NORMAL, and the worst concurrent write stayed at 7.4 ms (8.6 ms with NORMAL).
- **Severity is an enum** (`database/severity.rs`: `Info`, `Warning`, `Error`), written to the
  database as `info`, `warning`, `error`. Every source and every SQL statement that sets one uses
  it, so a misspelt severity does not compile instead of storing a row the viewer cannot place.
- `state` holds small key-value settings the service keeps between runs (the journal cursors).
- Timestamps are UTC text (`YYYY-MM-DD HH:MM:SS`). They sort correctly, and every `ts` column is
  indexed.

### Viewer (`ui/`)

Modules: `blackbox_app.py` (entry point), `window.py` (the window, built from the mixins
`sidebar.py`, `overview.py`, `detail.py` and `refresh.py`), `widgets.py`, `chart.py`, `fmt.py`
(text helpers), `consts.py` (labels, colours, stylesheet), `gtkenv.py` (pins the GTK versions) and
`data.py` (read-only queries). The list and overview are read on a worker thread and applied on the
main thread, one read at a time. The event list is paged: a new filter or time range reads only
the newest 200 rows, older pages are read as the list is scrolled towards its end, and it stops at
1,000 rows (the footer says so; narrow the range for more). Rows are built 30 per idle callback,
and every query is cancelled after 8 s, so a huge database cannot freeze the window. While older
pages are loaded the 5-second timer leaves the list alone.

A native GTK4 and libadwaita app, `io.github.hawkosm.Blackbox`, listed in the application menu. It is
read-only, refreshes every 5 s, and runs only while its window is open. The layout is an adaptive
split view that collapses to single pages below 720 px.

- **Event list:** grouped by day, filtered by Problems / Errors / Everything, sources and time range,
  with search. Repeats of the same message fold into one row with a count (`×218`), even with other
  events in between (within 10 minutes) and when only a pid differs: a login that runs the same
  failing helper 90 times shows as one row.
- **Overview:** error and warning counts, current CPU, memory, GPU and power (CPU watts, battery
  state), and three charts with a tick for every notable event: usage (CPU, memory, GPU, in %),
  temperature (GPU, in °C) and power (CPU, GPU and battery draw, in W).
- **One event:** the same three charts at ±2 min, ±10 min or ±1 h (each with a table view), the
  crash's stack trace from systemd-coredump, the full row with readable field names, and every event
  around it with its offset. Copy puts all of that on the clipboard as a plain-text report.
- **Go to a time:** the clock button opens the same view for any moment, even one where nothing was
  recorded (a freeze, for example).

Each chart has one axis in one unit (percent, °C or W) with fixed hues for CPU, memory (and
battery) and GPU, checked for colour-blind separation against both the light and the dark card
surface.

## Storage budget

The whole system stays within 52.5 GB:

| Part | Limit | Set by |
|---|---|---|
| Database | 50 GB | `BLACKBOX_MAX_MB` |
| Audit log | 2 GB (4 x 500 MB) | `deploy/setup-auditd.sh` |
| Headroom (WAL, trim slack) | 0.5 GB | |

journald's own journal is managed by systemd and is not counted; only its warnings and worse are
copied into the database. At the volume recorded now (crashes, plus one system sample every 10 s
at roughly 1 MB a day) the database will not come near its cap for years. Storing every whole-process
exit (about 22 a second, at about 180 bytes each) would use roughly 340 MB a day, so about three weeks of
history in 50 GB.

## Schema

Every table is in `src/database/scheme.rs`, with a comment on each column. `messages` is an alert
index: one row per notable event, pointing at
the full row in the matching source table through `(source, ref_id)`. That pointer is enforced by
the application, not by a foreign key, because a foreign key cannot target several tables.

## Design decisions

The short version; the options that lost and the reasoning are in [decisions/](decisions/).

| Decision | Reason |
|---|---|
| Crashes and failures only, not every exec or exit | Every exec and exit is about 100 events a second. Auditd and the `/proc` sampler already cover "what ran" and process churn. |
| Crash signals 4, 5, 6, 7, 8, 11, 31 | The signals that dump core. SIGTERM and SIGHUP are routine shutdowns. SIGKILL is mostly benign here (GNOME's image loader kills its helpers), and real OOM kills are logged by the kernel to journald. |
| No tail of every exit after a crash | It was dropped: two crashes stored 3,268 exits in 20 minutes, 99.7% of them status 0 or 1. Failures anywhere are now stored all the time instead, which catches a cascade without the noise. |
| Crash names from systemd-coredump | Batching keeps wakeups low but loses the name of a process that dies within ~100 ms. The coredump entry has the name and path anyway, at no extra cost. |
| No summary or roll-up table | An hourly exec and exit count catches almost nothing that load, auditd and journald do not. |
| Own `/proc` sampler | Avoids a package and a second data format for the same numbers. |
| Native GTK app, not a web page | It should exist only while open, with no server or browser. |
| package history read every 30 s, not followed | It changes only while the package manager runs. A size check costs nothing; a `tail` process would sit there all day. |
| Unclean shutdowns detected at the next boot | Nothing can record a freeze at the time it happens. The journal's last entry, read once per boot, is enough to tell. |
| Sources grouped by how data arrives, one loop per group | journald and auditd were the same loop twice, as were the sampler and the package readers. A trait over all six sources would only rename `start()`; a trait per group removes real duplication. |
| One writer thread, records that store themselves | Sources differ in everything except where their data goes, so the shared part is the writing. One thread commits a burst in one transaction, and each record is tested on its own against an in-memory database. A trait over the six sources themselves would only rename `start()`; the trait is over what they store. |

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
which include the `journalctl` child. These were measured before the writer thread and the
supervisor; they have not been measured again since (the commits are fewer, see "Sources and the
writer", but CPU and memory are not re-checked):

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
- Records still queued for the writer when the service is killed (SIGTERM, a poweroff) are lost:
  the writes of one commit at most. Ending on its own, it waits for them (`database::drain`).
- Both journald followers resume by time, on a first run, from the newest journal row of either of
  them, so a feed with older unstored entries than the other's newest can skip them once.

## Tests

`cargo test` runs about 80 tests; each sits next to the code it tests, so it can test private
functions too.

- **Parsing:** journald and audit lines (coredump fields, cursors, colour codes), audit
  classification on its own (`classify`), raw kernel events (`decode`: execs and exits of whole
  processes, not threads or forks), the package history formats, clean and unclean boot endings.
- **Rules:** the exit rule, and the kernel filter against it for every one of the 65,536 wait
  statuses; what an exit becomes as a record; journald's resume arguments and when its cursor is
  gone.
- **Records** (each `store.rs`, against an in-memory database): the journal cursor not shifting row
  ids, duplicates skipped within and across batches, a NaN sensor value skipped, a shutdown marked
  once, a crash named from its coredump entry.
- **The machinery:** the writer thread (four sources at once, a failed record rolled back alone, a
  burst as one commit, a lone record not held back, `drain`), the follow loop (every line handed
  over, the end reported, no process left behind after a panic), the supervisor (a panicking
  source runs again with backoff, one that returns does not), `exists` telling an error from a
  missing row, `read()` refusing to write, a text log read as it grows or while it is missing, dnf
  read again only when its database changes, resuming the audit log after downtime, and the
  ring-buffer trim, including one far-future row (without the cap it keeps 0 of 3,000 recent rows).

`tests/end_to_end.rs` runs the real binary, with every path it reads pointed at a temporary folder:
`--version`, `--help` and an unknown argument; `--check` reporting a fixture pacman history
without creating a database; and a recording run that must store a source of every kind (the
pacman history imported, this boot, a line appended to a fixture audit log, a real `SIGSEGV`
crash, a system sample) and then, restarted on the same database, store none of it twice. A
container gets no kernel process events, so there the recording run says it is skipped and passes;
it takes about 16 s where it runs.

A test is only kept once it has been seen to fail: the code it guards is broken on purpose, the
test must go red, and the code is put back (`.claude/skills/verify-recorder`). Beyond the tests,
changes are run on the real machine against a throwaway database: a real crash, a real journal
warning, a restart, and an `fsync` count with an `LD_PRELOAD` shim when a change claims fewer
commits.
