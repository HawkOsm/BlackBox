# Adding a source

A source reads one part of the machine and hands what it finds to the database writer. This is the
whole recipe, worked through with a real example: memory pressure from `/proc/pressure/memory`,
read every 10 s, with an alert when the machine is thrashing. Every piece of code below was built,
tested and run against a throwaway database before it was written here.

Read [architecture.md](architecture.md), "Sources and the writer", first: it explains why the code
is shaped this way. The decisions behind it are in [decisions/](decisions/).

## 1. Pick the group

Sources are grouped by how their data arrives, and each group already has its loop:

| The data… | Group | You implement | Example |
|---|---|---|---|
| is pushed by the kernel | `kernel/` | the whole loop (there is no shared one yet) | `collector` |
| arrives as lines from a long-running process | `follow/` | `Follower`: `command`, `line`, `ended` | `journald`, `auditd` |
| is read again every few seconds | `poll/` | `Poller`: `every`, `poll` | `sampler`, `packages` |
| is read once at start-up | `once/` | a function that runs and returns | `boot` |

Memory pressure is a file that only means something when read again: `poll/`.

If your source fits no group, say so before building it. A second kernel source, for example,
would be the moment to give `kernel/` a shared loop like the other groups have.

## 2. The files

One folder per source, and a file of the same name beside it (Rust's 2018 layout, no `mod.rs`).
Name the other files by their job, never `utils` or `helpers`:

```
src/sources/poll/pressure.rs           what it is, its files, start()
src/sources/poll/pressure/poller.rs    reading the machine
src/sources/poll/pressure/store.rs     the record and its SQL
```

`pressure.rs`: the table of contents, and `start()`:

```rust
//! Memory pressure (PSI): the share of time tasks were stalled waiting for memory, every 10 s.
//!
//! - `poller.rs`: the `Poller` that reads `/proc/pressure/memory`.
//! - `store.rs`: `Pressure` and its SQL.

mod poller;
mod store;

use super::poll;
use crate::sources::supervise;
use poller::PressurePoller;

pub fn start() {
    let mut pressure = PressurePoller;
    supervise("pressure", move || poll(&mut pressure));
}
```

`supervise` runs it on a thread named `pressure`. If it panics, the supervisor reports it and runs
it again with backoff instead of letting it go silent.

## 3. The record and its SQL (`store.rs`)

A record is a struct that knows how to store itself: it implements `Record`. Only the writer thread
calls `store`, inside its open transaction, so everything one `store` writes commits together.

```rust
use crate::database::{Record, Severity, index, ts_text};
use rusqlite::{Connection, params};

/// One memory-pressure reading.
pub struct Pressure {
    pub ts: i64,
    /// % of the last 10 s in which at least one task was stalled waiting for memory.
    pub some_avg10: f64,
}

/// Above this the machine is thrashing: worth an alert.
const ALERT_PCT: f64 = 20.0;

impl Record for Pressure {
    fn store(&self, c: &Connection) -> rusqlite::Result<()> {
        let ts = ts_text(self.ts);
        c.execute(
            "INSERT INTO pressure (ts, some_avg10) VALUES (?1, ?2)",
            params![ts, self.some_avg10],
        )?;
        let alert = (self.some_avg10 >= ALERT_PCT).then(|| {
            let text = format!("memory pressure {:.0}%", self.some_avg10);
            (Severity::Warning, text)
        });
        index(
            c,
            "pressure",
            &ts,
            alert.as_ref().map(|(s, t)| (*s, t.as_str())),
        )
    }
}
```

The rules this follows:

- **Owned data.** A record crosses into the writer's thread, so it holds `String`, not `&str`.
- **`index` after the row.** `index` writes the `messages` row (the alert the viewer lists) and
  takes its `ref_id` from the last insert, so call it right after the row it points at. No alert:
  pass `None`, and the row is stored without one.
- **`Severity`, never text.** `Severity::Info`, `Warning` or `Error`; a misspelt one does not
  compile.
- **Time.** `ts` is epoch seconds, stored as UTC text with `ts_text`. When the data has no time of
  its own, use `database::now()`.
- **A record is what must commit together.** If two rows only make sense together (journald's row
  and its cursor, the sampler's four tables of one tick), they are one record.
- **De-duplication goes in `store`**, with `database::exists`, if a restart can read the same data
  again (packages, journald). `exists` returns an error rather than "no" when the database fails, so
  a failing check never stores a duplicate.

## 4. Reading the machine (`poller.rs`)

```rust
use super::store::Pressure;
use crate::database;
use crate::sources::poll::Poller;
use std::time::Duration;

pub(super) struct PressurePoller;

impl Poller for PressurePoller {
    fn every(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn poll(&mut self) {
        let Ok(text) = std::fs::read_to_string("/proc/pressure/memory") else {
            return; // a kernel without PSI: nothing to record
        };
        if let Some(some_avg10) = avg10(&text, "some") {
            database::write(Pressure {
                ts: database::now(),
                some_avg10,
            });
        }
    }
}

/// `avg10` of one line of a PSI file: `some avg10=1.23 avg60=… total=…`.
fn avg10(text: &str, line: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with(line))?
        .split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))?
        .parse()
        .ok()
}
```

The rules:

- **Never write the database yourself.** Hand records to `database::write`; it returns at once.
- **Keep parsing separate from reading**, as `avg10` is here: a pure function of text is testable
  without the machine. auditd's `classify` and the collector's `decode` follow the same split.
- **A missing input is not an error.** Return and look again next time; the poll loop calls you
  forever. Only panic for a bug: the supervisor restarts you, but a panic is still a bug.
- **Resuming after a restart** (follow and poll sources that read a history): read where you
  stopped from the database once, at start-up (`database::last_ts`, `get_state`, or your own query
  through `database::read`, which is read-only). After that, keep the position in memory: a record
  you just sent may still be on its way to the database.

A followed source is the same with `Follower` (see `follow/auditd/follower.rs`): `command`
returns the process to run, set up to resume, `line` handles one line, `ended` decides what an end
means. The loop restarts the process, and stops and reaps it if your code panics.

## 5. Wire it in

Four places outside your folder:

1. **The group's list,** `src/sources/poll.rs`: `pub mod pressure;`
2. **`sources::run()`,** `src/sources.rs`: `poll::pressure::start();`
3. **The table,** `src/database/scheme.rs`, in `SCHEMA` (a new table is created on the next start,
   in old databases too):

   ```sql
   CREATE TABLE IF NOT EXISTS pressure (
       pressure_id INTEGER PRIMARY KEY,
       ts          TEXT    NOT NULL,
       some_avg10  REAL    NOT NULL   -- % of the last 10 s some task waited for memory
   );
   CREATE INDEX IF NOT EXISTS idx_pressure_ts ON pressure(ts)
   ```

   A column added to a table that already exists needs `ensure_column` in
   `database/connection.rs`, the way `custom.exe` got there.
4. **`TIME_TABLES`,** in the same file. Easy to forget, and nothing fails at once: a table missing
   here is never trimmed by the ring buffer, and grows until the disk is full.

## 6. Tests

Next to the code, so they can test private functions:

```rust
// poller.rs: the parsing, without the machine
#[test]
fn reads_the_ten_second_average() {
    let text = "some avg10=1.50 avg60=0.20 avg300=0.00 total=11\nfull avg10=0.40 avg60=0.00 avg300=0.00 total=5\n";
    assert_eq!(avg10(text, "some"), Some(1.5));
    assert_eq!(avg10(text, "full"), Some(0.4));
    assert_eq!(avg10("", "some"), None);
}

// store.rs: the record against an in-memory database, opened the way the service opens one
#[test]
fn high_pressure_is_stored_with_an_alert() {
    let c = open(":memory:");
    Pressure { ts: 1, some_avg10: 3.0 }.store(&c).unwrap();
    Pressure { ts: 2, some_avg10: 45.0 }.store(&c).unwrap();
    assert_eq!(count(&c, "SELECT COUNT(*) FROM pressure"), 2);
    assert_eq!(count(&c, "SELECT COUNT(*) FROM messages WHERE source = 'pressure'"), 1);
}
```

(`open` and `count` come from `crate::database`, for tests only.)

Then see each test fail: break the line it guards (raise `ALERT_PCT` above 45, make `avg10` read
`avg60=`), run it, watch it go red, put the line back. A test you have not seen fail may test
nothing. See [development.md](development.md).

If the source can be faked from outside, add it to `tests/end_to_end.rs`: point its input at a
fixture in the test's temporary folder (through an environment variable, like `BLACKBOX_AUDIT_LOG`)
and check its rows appear and survive a restart without duplicates.

## 7. The viewer

The viewer (`ui/`) lists only the sources it knows. Until it is told, the new rows are stored but
not shown:

- `ui/data.py`, `TABLES`: the table and its id column, `"pressure": "pressure_id"`.
- `ui/consts.py`, `SOURCES`: the source and its label in the filter, `("pressure", "Pressure")`.

## 8. Check it on the machine, and write it down

```bash
cargo fmt && cargo clippy --all-targets && cargo test
cargo build --release
BLACKBOX_DB=/tmp/try.db timeout 25 ./target/release/blackbox
sqlite3 /tmp/try.db "SELECT COUNT(*), MAX(some_avg10) FROM pressure"
```

With the example: two rows in the first 14 s, one at once and one 10 s later.

Last, the docs: a section under "Components" in [architecture.md](architecture.md) (what it
reads, what it stores, when it alerts, measured numbers), a row in the overview table in
`src/sources.rs`, the table in [database.md](database.md), and a file in [decisions/](decisions/)
if building it meant choosing between real alternatives.
