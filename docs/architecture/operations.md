# Performance, known gaps, tests

## Performance

Measured on the development machine (22 cores) with the installed service, using the service's
cgroup counters, which include the `journalctl` child. Measured before the writer thread and the
supervisor existed, and not re-measured since:

| | Before tuning | Now |
|---|---|---|
| CPU, whole service | 0.34% of one core | 0.14% of one core (0.006% of the machine) |
| Memory | 5.4 MB | 5.1 MB |
| Collector wakeups | 98 to 136 per second | 8.5 per second |

Other costs: one `nvidia-smi` call is about 13 ms of CPU, hence the 30 s GPU interval. The database
grows by roughly 180 bytes per stored event.

To measure again, take two readings a minute apart and divide the CPU difference by the elapsed
time:

```bash
systemctl --user show blackbox -p CPUUsageNSec -p MemoryCurrent
```

**Trade-off of batching.** A process that dies within about 100 ms of starting loses its name lookup
(measured: 92% of such exits unnamed at 100 ms, versus only forked-without-exec subshells at 0 ms).
Crashes that dump core get the name back from systemd-coredump. Lowering `BLACKBOX_BATCH_MS` trades
wakeups for names; 0 ms gives about 50 wakeups per second, about 0.1% of one core more.

## Known gaps

- auditd needs a one-time `sudo bash deploy/setup-auditd.sh`. Checked live: all 16 rules load, and a
  denied write to `/etc/fstab` is stored as a `bb_boot` warning within 2 s. auditctl prints "Old
  style watch rules are slower" for each `-w` rule; that is a deprecation notice, not an error.
- After downtime the audit follower resumes from the current log only. Records that rotated into
  `audit.log.1` while the service was stopped are missed.
- A failure that exits with status 1 is not recorded: it looks the same as the polling helpers that
  return 1 for "not found" hundreds of times a minute.
- Very short-lived processes that fail without dumping core usually have no name, only their
  parent's.
- If the kernel queue overflows under a burst, events are dropped and the collector logs it.
- Records still queued for the writer when the service is killed (SIGTERM, poweroff) are lost: one
  commit's worth at most. When the process ends on its own it waits for them (`database::drain`).
- On a first run, both journald followers resume by time from the newest journal row of either one.
  A feed with older unstored entries than the other's newest can skip them once.

## Tests

`cargo test` runs about 80 tests. Each sits next to the code it tests, so it can reach private
functions.

- **Parsing:** journald and audit lines (coredump fields, cursors, colour codes), audit
  `classify`, raw kernel events (`decode`: execs and exits of whole processes, not threads or
  forks), package history formats, clean and unclean boot endings.
- **Rules:** the exit rule, and the kernel filter against it for all 65,536 wait statuses; what an
  exit becomes as a record; journald's resume arguments and when its cursor is gone.
- **Records** (each `store.rs`, against an in-memory database): the journal cursor not shifting row
  ids, duplicates skipped within and across batches, a NaN sensor value skipped, a shutdown marked
  once, a crash named from its coredump entry.
- **Machinery:** the writer (four sources at once, a failed record rolled back alone, a burst as one
  commit, a lone record not held back, `drain`); the follow loop (every line handed over, the end
  reported, no process left behind after a panic); the supervisor (a panicking source runs again with
  backoff, one that returns does not); `exists` telling an error from a missing row; `read()`
  refusing to write; a text log read as it grows or while missing; dnf read again only when its
  database changes; the audit log resumed after downtime; the ring-buffer trim, including one
  far-future row.

`tests/end_to_end.rs` runs the real binary with every path it reads pointed at a temporary folder:

- `--version`, `--help` and an unknown argument.
- `--check` reporting a fixture pacman history without creating a database.
- A recording run that must store one source of every kind (pacman history imported, this boot, a
  line appended to a fixture audit log, a real `SIGSEGV` crash, a system sample), then, restarted on
  the same database, store none of it twice.

A container gets no kernel process events, so there the recording run says it is skipped and
passes. Where it runs it takes about 16 s.

A test is kept only after it has been seen to fail: break the code it guards on purpose, the test
must go red, then put the code back (`.claude/skills/verify-recorder`). Beyond tests, changes run on
the real machine against a throwaway database: a real crash, a real journal warning, a restart, and
an `fsync` count with an `LD_PRELOAD` shim when a change claims fewer commits.
