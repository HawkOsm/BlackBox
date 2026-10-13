# 0005: A supervisor restarts a source that panics

**Status:** in use.

## Problem

Each source ran on a plain thread. If it panicked, Rust printed the message once, the thread was
gone, and the rest of the recorder carried on without it. A sampler panic (it called `.unwrap()` on
the clock) would have meant no CPU, memory or temperature row for the rest of the run, and nothing
saying so after the first message. For a recorder whose whole value is completeness, a silent gap
is the worst failure.

## Options

1. **Remove every panic.** Necessary anyway, but it cannot be proven complete.
2. **Restart the whole service** on any panic (abort, and let systemd restart it). Simple, but one
   failing source would take the five working ones down with it, again and again.
3. **Supervise each source:** catch its panic, report it, run it again.

## Decision

Option 3, `sources/supervise.rs`, plus option 1 where a panic was found (the clock `unwrap`
became `database::now()`). Each source runs on a thread named after it. A source that returns
(boot) is done; one that panics runs again, with the state it had.

The first version restarted every 30 s forever. A review pointed out that a source that always
panics would then log about 2,880 panics a day, and the journald follower would store them back
into the database. So the wait doubles while it keeps panicking, up to 30 min, and starts over at
30 s once the source has run fine for 10 min.

## Consequences

- One failing source no longer silences itself, nor takes the others with it.
- Restarting with the old state can repeat the panic; the backoff bounds that.
- A followed process is stopped and reaped when its source panics, so restarts leave no
  `journalctl` or `tail` behind. This was found by the same review.
- The collector is not supervised: it runs on the main thread, and when its netlink socket fails
  the process exits (after draining the writer's queue) and systemd starts it again.
