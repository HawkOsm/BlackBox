# 0003: Sources grouped by how their data arrives

**Status:** in use.

## Problem

Every source had a `start()` that spawned a thread with a loop, which looked like a pattern worth
abstracting.

## Options

1. **One `Source` trait for all six** (Strategy), with `start()` or `run()`.
2. **Group them by how their data arrives**, with one shared loop per group:
   - `kernel/`: the kernel pushes events (collector)
   - `follow/`: a long-running process's lines (journald, auditd)
   - `poll/`: read again every few seconds (sampler, packages)
   - `once/`: read once at start-up (boot)

## Decision

Option 2.

Option 1 would only have renamed `start()`. The six loops have nothing in common beneath
"spawn a thread": a timer, a followed process, a blocking socket, a single run. A trait with
nothing to share earns nothing. The test that decided it: name the change the pattern makes
easier. For option 1 there was none.

Within a group, the loops really were the same. journald and auditd each had their own copy of
"start a process, read its lines, restart it when it ends"; the sampler and the package readers
both "look, store, sleep". So each group got one loop, and a trait for what differs (`Follower`:
`command`, `line`, `ended`; `Poller`: `every`, `poll`).

## Consequences

- The follow loop exists once. Fixing it once (stopping and reaping the process after a panic)
  fixed both followed sources.
- Grouping by how data arrives, not by what it is about: auditd reads a file but behaves like
  journald; the sampler reads kernel files but behaves like the package readers. That surprised
  us, and it is the right grouping because the behaviour is what decides the code.
- `kernel/` and `once/` hold one source each and no shared loop. A second kernel source (netlink
  uevents, for hotplug) would be the time to give `kernel/` one.
