# Development

How to build BlackBox, test it, run it without touching your real recording, and prove a change
works. The structure of the code is in [architecture.md](architecture.md); adding a new source is
in [adding-a-source.md](adding-a-source.md).

## Build

Rust 1.88 or newer (`rust-version` in `Cargo.toml`, edition 2024), a C compiler and SQLite's
development files. The packages per distribution are in the [README](../README.md#install). The
viewer is Python with GTK 4 and libadwaita and is not built.

```bash
cargo build --release          # target/release/blackbox
./target/release/blackbox --check
```

Four crates, no more: `rusqlite`, `serde_json`, `chrono`, `libc`.

## Test

```bash
cargo fmt --check
cargo clippy --all-targets        # must print no warning
cargo test                         # about 80 tests; the end-to-end ones take ~16 s
cargo test --release trim_does_not_stall_writers -- --ignored --nocapture   # stress test
```

- **Unit tests** sit next to the code they test (`#[cfg(test)] mod tests`), so they reach private
  functions. A long test module can move into its own file (`database/tests.rs`) without losing
  that.
- **Records** are tested against an in-memory database opened the way the service opens one:
  `crate::database::open(":memory:")`, with `count` for `SELECT COUNT(*)`.
- **End to end,** `tests/end_to_end.rs` runs the real binary against fixture logs in a temporary
  folder. Its recording test needs the kernel's process events; without them (a container, a
  separate network namespace) it prints `skipped` and passes. To see that path:
  `unshare -rn target/debug/deps/end_to_end-<hash> --nocapture`.
- **The stress test** measures how long a write waits while the ring buffer trims. Its database is
  in `/tmp`, a tmpfs where fsync costs nothing; set `TMPDIR` to a real disk to see the cost of
  `synchronous=FULL`.

The viewer is linted with `ruff` (`ruff.toml`: errors and undefined names, not style).

## A test must be seen to fail

A new test only counts once it has failed for the right reason. Break the line it guards
(comment it out, invert the condition), run the test and see it go red, then put the line back
and see it pass. More than one test in this repository passed with the code it was meant to guard
removed; this is how they were found: the dnf test and the process-leak test were tightened, and
half of the `exists` test, which could not fail, was dropped.

## Run it without touching your recording

Every path the recorder reads or writes can be pointed elsewhere:

| Variable | Default |
|---|---|
| `BLACKBOX_DB` | `~/.local/share/blackbox/blackbox.db` |
| `BLACKBOX_MAX_MB` | `50000` (the ring buffer's budget) |
| `BLACKBOX_BATCH_MS` | `100` (how long the collector lets a burst pile up) |
| `BLACKBOX_AUDIT_LOG` | `/var/log/audit/audit.log` |
| `BLACKBOX_PACMAN_LOG`, `BLACKBOX_APT_LOG`, `BLACKBOX_ZYPPER_LOG` | each manager's own history |
| `BLACKBOX_DNF_DB` | dnf5's or dnf4's history database |

```bash
BLACKBOX_DB=/tmp/try.db timeout 45 ./target/release/blackbox
sqlite3 /tmp/try.db "SELECT source, severity, COUNT(*) FROM messages GROUP BY 1, 2"
```

Real events to check it with, while it runs:

```bash
sh -c 'kill -SEGV $$'                  # a crash: an error in `custom`
sh -c 'exit 3'                         # a failure: a warning, named after its parent
logger -p user.warning "test-$$"       # a journal warning
```

Then run it again on the same database: nothing may be stored twice (packages, journald, boot).

## Count commits

`synchronous=FULL` makes every commit an fsync, so a change that claims fewer commits is checked by
counting them. `rusqlite` links the system's SQLite, so a small preloaded library can count every
`fsync` and `fdatasync`:

```c
// fsynccount.c: cc -shared -fPIC -O2 -o fsynccount.so fsynccount.c -ldl
#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
static void mark(void) { write(2, "@@FSYNC\n", 8); }
int fsync(int fd) { static int (*real)(int); if (!real) real = dlsym(RTLD_NEXT, "fsync"); mark(); return real(fd); }
int fdatasync(int fd) { static int (*real)(int); if (!real) real = dlsym(RTLD_NEXT, "fdatasync"); mark(); return real(fd); }
```

```bash
rm -f /tmp/c.db*; BLACKBOX_DB=/tmp/c.db LD_PRELOAD=$PWD/fsynccount.so timeout 45 ./target/release/blackbox 2>err
grep -c @@FSYNC err
```

Compare with the build before the change, same length, on a fresh database each time. For the
writer thread this gave 207 before and 37 to 47 after. If the count is 0, the library did not load
(`ld.so` says so on stderr): that is a broken measurement, not a result.

## Code layout

- **A folder per subsystem, a file per job.** A folder `name/` has `name.rs` beside it (Rust's
  2018 layout; there is no `mod.rs` in this repository) holding only what the folder is for, the
  list of its files and what it offers. The code is in files named after what they do: `parse.rs`,
  `follower.rs`, `store.rs`. No `utils` or `helpers`: a name that says nothing collects everything.
- **Sources** are in `src/sources/<group>/<name>/`, grouped by how their data arrives; see
  [adding-a-source.md](adding-a-source.md).
- **The database knows no source.** Records and their SQL live in each source's `store.rs`; only
  the writer thread stores them.
- **Severity is `database::Severity`,** never text.

## CI

`.github/workflows/ci.yml`, on every push and pull request:

| Job | What |
|---|---|
| Rust | `cargo fmt --check`, `clippy`, `cargo test` |
| Rust 1.88 | the tests on the minimum supported version |
| Distributions | build and test inside Arch, Debian 13, Ubuntu 24.04, Fedora (dnf5), Rocky 9 (dnf4), openSUSE Tumbleweed, then `--check` must find that distribution's package history (`.github/scripts/distro-test.sh`, also runnable locally with podman) |
| Shell | `shellcheck` on the deploy scripts |
| Dependency audit | `cargo audit` |
| Python UI | `ruff` |

The distribution containers have no kernel process events, so the netlink socket test is skipped
there and the end-to-end recording test skips itself; the Rust job, on the runner itself, runs both.

## Claude Code

`.claude/skills/` holds the rules an AI assistant follows in this repository, adapted from the
`skysat-mlops` project: `working-principles` (simplicity, one job per file), `verify-recorder`
(the checks above, in order) and `project-guide`. They say the same as this file, for a reader that
is not a person.
