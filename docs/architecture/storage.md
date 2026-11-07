# Storage (`database/`)

## SQLite settings

- WAL mode, `synchronous=FULL`. With NORMAL, the last ~30 s (`vm.dirty_expire_centisecs`) would sit
  in the page cache and be lost in a hard freeze or kernel panic, which is when a flight recorder is
  needed. Steady state is one commit every 10 s, so the extra fsync costs little.
- Only the writer thread writes; the trim job deletes. A source row and its `messages` row are
  always written together.
- Timestamps are UTC text (`YYYY-MM-DD HH:MM:SS`). They sort correctly, and every `ts` column is
  indexed.
- `state` holds small key-value settings kept between runs (the journal cursors).
- `Severity` (`severity.rs`: `Info`, `Warning`, `Error`) is an enum, written as `info`, `warning`,
  `error`. A misspelt severity does not compile, instead of storing a row the viewer cannot place.

## Schema

Every table is in `src/database/scheme.rs`, with a comment on each column (see also
[../database.md](../database.md)). `messages` is an alert index: one row per notable event, pointing
at the full row in its source table through `(source, ref_id)`. The application enforces that
pointer, not a foreign key, because a foreign key cannot target several tables.

## Ring buffer (`retention.rs`)

Every 5 minutes the trim job measures the data in use (pages minus free pages). Over the budget, it
deletes the oldest tenth of the time span from every table together, repeating until usage is under
90% of the budget. Freed pages are reused, so the file plateaus instead of growing.

- **The newest time is capped at now.** Without the cap, one row stamped in the far future (a clock
  that was once wrong) stretched the span so much that the first pass deleted every real row. The
  test reproduces it: 0 of 3,000 recent rows kept.
- **Trimming must not block writers.** Deletes run in chunks of 1,000 rows with the lock released
  between chunks. The oldest/newest lookups use one `MIN` or `MAX` per subquery, because SQLite
  scans the whole index when both are in one `SELECT`.
- **Measured** with `cargo test --release trim_does_not_stall_writers -- --ignored`: the worst write
  during a trim went from 1,217 ms (one big delete) to 2.9 ms on a 126 MB database and 3.6 ms on a
  635 MB one, and it does not grow with size. Deletion runs at about 18 MB/s in the background.
- The test's default location, `/tmp`, is a tmpfs where fsync is free. Run it with `TMPDIR` on a real
  disk to see the cost of `synchronous=FULL`. On the development machine's NVMe (ext4), 1.2 M rows
  trimmed in 8.7 s instead of 7.4 s with NORMAL, and the worst concurrent write stayed at 7.4 ms
  (8.6 ms with NORMAL).

## Storage budget

The whole system stays within 52.5 GB:

| Part | Limit | Set by |
|---|---|---|
| Database | 50 GB | `BLACKBOX_MAX_MB` |
| Audit log | 2 GB (4 x 500 MB) | `deploy/setup-auditd.sh` |
| Headroom (WAL, trim slack) | 0.5 GB | |

journald's own journal is managed by systemd and not counted; only its warnings and worse are
copied into the database.

At the current volume (crashes, plus one system sample every 10 s at roughly 1 MB a day) the
database will not near its cap for years. Storing every whole-process exit (about 22 a second, about
180 bytes each) would use roughly 340 MB a day: about three weeks of history in 50 GB.
