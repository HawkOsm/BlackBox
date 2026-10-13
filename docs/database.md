# The database

One SQLite file, `~/.local/share/blackbox/blackbox.db` (or `BLACKBOX_DB`), readable by its user
only. Every table is created in `src/database/scheme.rs`, which has a comment on each column; this
page is the same, organised for someone reading the data: the viewer, or you with `sqlite3`.

## How to read it

- **While the recorder runs, read-only:** `sqlite3 -readonly ~/.local/share/blackbox/blackbox.db`.
  It is in WAL mode, so a reader never blocks the recorder's writes and always sees whole commits.
  Do not hold a read transaction open for long, though: while one is open the ring buffer cannot
  reset the WAL file (`wal_checkpoint`), and it keeps growing.
- **Times are UTC text,** `YYYY-MM-DD HH:MM:SS`. They sort correctly as text, and every `ts` column
  is indexed. For local time: `datetime(ts, 'localtime')`.
- **Old rows disappear.** It is a ring buffer: past its budget (50 GB, `BLACKBOX_MAX_MB`) the
  oldest tenth of the time span is deleted from every table at once (`TIME_TABLES` in
  `scheme.rs`), so what remains is always one continuous stretch of time.
- **Only the recorder writes.** Each source's rows come from its own `store.rs` in
  `src/sources/`, committed by the one writer thread.

## `messages`: the index of everything notable

Every alert, from every source, is one row here, so a list of events needs no join:

| Column | |
|---|---|
| `source` | the table of the full row: `custom`, `journald`, `auditd`, `sysstat`, `boot`, `packages` |
| `ref_id` | the id of that row in its table (`custom_id`, `journald_id`, …) |
| `ts` | when it happened |
| `severity` | `error`, `warning` or `info` |
| `summary` | one line, ready to show |

`(source, ref_id)` is the primary key. It points at the full row, but it is not a foreign key: a
foreign key cannot point at several tables, so the recorder keeps it right (each source writes its
row and its `messages` row in the same transaction).

Not every row has a `messages` row: a system sample is stored every 10 s but listed only when it
crosses a threshold, and a successful sudo is kept in `auditd` without an alert.

## The source tables

### `custom`: processes that crashed or failed (collector)

| Column | |
|---|---|
| `custom_id`, `ts` | |
| `pid`, `ppid` | the process and its parent |
| `exit_code` | the raw wait status: `exit_code & 127` is the signal that killed it (0 if none), `exit_code & 128` a core dump, `exit_code >> 8` the exit status. See "Reading an exit code" in [architecture.md](architecture.md). |
| `comm` | its name, best effort: a very short-lived process is often gone before it can be read |
| `exe` | its full path, filled in afterwards from systemd-coredump for crashes that dumped core |

Stored: crashes (`error`), Rust panics (exit 101, `error`), any other exit status but 0 and 1
(`warning`).

### `journald`: journal lines

| Column | |
|---|---|
| `journald_id`, `ts` | |
| `priority` | syslog level, 0 (emergency) to 7 (debug); 0 to 3 are stored as `error`, 4 as `warning` |
| `unit` | the systemd unit, or the program's name |
| `pid` | |
| `message` | the text, terminal colour codes removed |

Stored: warnings and worse, and Rust panics at any priority. Routine noise (`src/sources/routine.rs`)
is stored as `info`.

### `auditd`: audit records

| Column | |
|---|---|
| `audit_id`, `ts` | |
| `event_type` | the record type (`USER_AUTH`, `ANOM_ABEND`, …), or for a hit on one of our rules its key (`bb_modules`, `bb_identity`, `bb_boot`) |
| `pid`, `uid` | |
| `executable` | the program, when the record names one |

### `sysstat`: the system, every 10 s (sampler)

`sysstat_id`, `ts`, `cpu_pct`, `mem_pct`, `disk_io_kb` (KB/s), `load_avg`, and `gpu_pct`,
`gpu_mem_pct`, `gpu_temp` (every third sample, while the GPU is awake). An alert only past a
threshold: memory 85% (95% is an error), CPU 90%, load above twice the cores, GPU 85 °C or 90% of
its memory.

### `power`: power, every 10 s (sampler)

| Column | |
|---|---|
| `power_id`, `ts` | |
| `on_battery` | 1 discharging, 0 not, NULL on a machine without a battery |
| `battery_pct` | |
| `cpu_watt` | the CPU package, from RAPL; NULL until `deploy/setup-sensors.sh` has run |
| `gpu_watt` | from `nvidia-smi`, NULL between its polls and while the GPU sleeps |
| `battery_watt` | the whole machine's draw, only while discharging |

### `sensors`: every hwmon reading, every 10 s (sampler)

| Column | |
|---|---|
| `sensor_id`, `ts` | |
| `kind` | `temp` (°C), `fan` (rpm) or `power` (W) |
| `name` | `chip/label`, e.g. `coretemp/Package id 0`, `rapl/psys`; chips that share a name get their device in brackets, `nvme[nvme0]/Composite` |
| `value` | never NULL: a reading that is not a number is not stored |

### `procstat`: the top processes by memory, every 30 s (sampler)

`procstat_id`, `ts`, `pid`, `comm`, `rss_kb`, and `pss_kb` (the accurate share, NULL when it
could not be read). Five rows per sample.

### `boot`: boots (boot)

| Column | |
|---|---|
| `boot_id`, `ts` | |
| `kind` | `start` (this boot began, `info`) or `unclean_end` (the previous one stopped without a clean shutdown, `error`) |
| `kernel_boot` | the kernel's boot id of the boot the row is about |
| `kernel` | the kernel release, for `start` |
| `note` | for `unclean_end`: the last thing that boot logged |

`(kernel_boot, kind)` is unique: a boot is recorded once, however often the service restarts.

### `packages`: package manager transactions (packages)

| Column | |
|---|---|
| `package_id`, `ts` | when the transaction began |
| `command` | the package manager's command line, when its history has it |
| `changes` | one line per package: `upgraded linux (6.9-1 -> 6.10-1)`, `installed`, `removed`, `downgraded`, `reinstalled` |

Every transaction is listed as `info`; its summary starts with the manager's name and lists the
packages that most often break a machine first: `pacman: upgraded 83 · linux 7.2.4 → 7.2.6, …`.

### `state`: the recorder's own memory

`key`, `value`: small values the service keeps between runs, such as `journald.cursor.warnings`
(where the follower is in the journal) and `journald.routine` (which routine rules have been
applied to stored lines). Not a time table: never trimmed.

## Questions it answers

```sql
-- the last hour's problems, newest first
SELECT datetime(ts, 'localtime'), source, severity, summary FROM messages
WHERE severity != 'info' AND ts > datetime('now', '-1 hour') ORDER BY ts DESC;

-- what was upgraded in the week before a crash
SELECT p.ts, p.changes FROM packages p, custom c
WHERE c.custom_id = 4242 AND p.ts BETWEEN datetime(c.ts, '-7 days') AND c.ts;

-- the hottest the CPU got each day
SELECT date(ts), MAX(value) FROM sensors
WHERE kind = 'temp' AND name LIKE 'coretemp/Package%' GROUP BY 1;

-- which programs crash most
SELECT comm, COUNT(*) FROM custom WHERE exit_code & 127 != 0 GROUP BY comm ORDER BY 2 DESC;
```

## Changing the schema

New tables and indexes are `CREATE … IF NOT EXISTS` in `SCHEMA`, so an existing database gets them
on the next start. A new column in an existing table needs `ensure_column` in
`src/database/connection.rs` too, which adds it to old databases (`custom.exe` and the GPU columns
of `sysstat` came that way). A new table that has a `ts` goes into `TIME_TABLES`, or the ring buffer
never trims it. See [adding-a-source.md](adding-a-source.md).
