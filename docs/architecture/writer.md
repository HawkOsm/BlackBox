# Sources and the writer

## One loop per group

Sources are grouped by how their data arrives, and each group has its loop written once.

- **Follow** (`follow/follower.rs`): run a process, hand over every line, run it again when it
  ends. A source implements `Follower`: `command` (what to run, resuming after what is stored),
  `line` (what one line means) and `ended` (what to do when the process ends; journald forgets a
  cursor the journal no longer has).
- **Poll** (`poll.rs`): call `poll` every `every`. A source implements `Poller`.
- A followed process is stopped and reaped when its run ends, even by a panic, so restarts leave no
  `journalctl` or `tail` behind. Lines are read as bytes and decoded leniently: invalid UTF-8
  becomes `�` instead of stopping the read and restarting the process at the same line.

## Supervisor

`sources/supervise.rs` runs every source except the collector on a thread named after it.

- A source that panics is reported and run again with the state it had, after 30 s. The delay
  doubles while it keeps panicking, up to 30 min, and returns to 30 s after 10 min of running fine.
- A source that returns (boot) is done.
- The collector stays on the main thread. If its netlink socket fails the process exits and systemd
  restarts it. It subscribes before any other source starts, so a kernel that refuses the
  subscription stops the process before anything is queued.

## The writer

Sources never commit. A source builds a record, `database::write(record)` queues it, and one writer
thread stores it.

```
 collector ─┐
 sampler ───┤
 profiles ──┤
 spike ─────┤  record   ┌─────────┐  everything queued   ┌────────┐
 journald ──┼─────────► │ channel │ ───────────────────► │ writer │ ──► one transaction ──► blackbox.db
 auditd ────┤           │ 256     │  so far              │ thread │
 packages ──┤           └─────────┘                      └────────┘
 boot ──────┘
 start-up reads (last_ts, get_state, boot_recorded, last_package_ts) ──► a read-only connection
```

- **A record stores itself.** `Record` is a trait with one method, `store(&self, &Connection)`.
  Each source implements it for its records, next to their SQL. The writer only calls `store`, so it
  names no source, and a new source adds records without touching `database/`.
- **Only the writer can store.** Sources get a second, read-only connection through
  `database::read()` for their start-up checks. A write on it fails.
- **One commit per burst.** The writer blocks for one record, takes whatever else is queued, and
  commits all of it in one transaction. A lone record commits at once, so `synchronous=FULL` still
  puts every event on disk before the next can be lost. A timed batch (commit every 500 ms) was
  rejected: it would hold the last half second in memory at the moment a flight recorder matters.
  Measured on a fresh database, 45 s each, counting `fsync`/`fdatasync` with an `LD_PRELOAD` shim:
  207 before, 37 to 47 after, same rows stored (most of the difference is the first import of 162
  pacman transactions).
- **A record is what must commit together.** `Journal` carries its cursor, so the row and the cursor
  land in one transaction. `Tick` is one sampler tick (system sample, power, sensors, top
  processes): one commit where there were four, so the steady state went from about 3.3 commits per
  10 s to 1. Updates are records too (`CrashName`, `MarkShutdown`, `DemoteRoutine`, `SetState`).
- **One failure does not sink a batch.** Each record runs in its own savepoint. A failed one is
  logged and rolled back alone (a `Tick` as a whole); the rest is kept. A sensor value that is NaN is
  skipped before it gets there: SQLite stores NaN as NULL, the column rejects it, and it would take
  the tick's system sample down with it.
- **De-duplication runs in the writer**, inside the transaction, so it also sees the batch's own
  earlier rows.
- **Sources remember where they are** in memory after reading it once at start-up: the journald
  followers their cursor and newest time, the audit follower its newest record. Re-reading the
  database after a restart could return a position older than rows still queued, and store them
  twice.
- **Two connections.** The writer's, also used by the trim job between batches (deletes run in
  1,000-row chunks, so the writer waits at most one chunk), and the read-only one. WAL lets the
  reader see each committed batch.

## Queue and failure

- 256 slots. A larger burst makes the sources wait while the writer commits.
- When the process ends on its own (collector stops or its socket fails), it waits up to 5 s for the
  queue to commit (`database::drain`). Records still queued when the process is killed are lost:
  the writes of one commit, milliseconds.
- If the writer thread panics, the process exits and systemd restarts it. Otherwise every source
  would queue into nothing.

## Package readers

`packages::History` (`name`, `path`, `poll`, `history`) is implemented once for text logs, generic
over a line `Parser` (`poll/packages/text_log.rs`), and once for dnf's database
(`poll/packages/dnf.rs`). A text log missing for a moment (rotation) counts as "nothing new", not an
error. The reader and `--check` only see `Box<dyn History>`, so a new package manager is one reader
plus one line in `detect()`.
