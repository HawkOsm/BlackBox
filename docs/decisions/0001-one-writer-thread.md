# 0001: One writer thread commits for every source

**Status:** in use.

## Problem

Every source wrote the database itself: its own `add_*` call took the shared connection's lock and
committed. The write policy (durability, batching, de-duplication) was spread over six threads, and
under `synchronous=FULL` every call was its own fsync: a first-run import of 162 pacman
transactions was 162 fsyncs, and every sampler tick four (power, sensors, sysstat, procstat).

## Options

1. **Leave it.** Steady-state writes are rare (one tick every 10 s), so the cost is small.
2. **Batch on a timer** ("commit every 500 ms"). Few commits, simple.
3. **One writer thread** (producer–consumer): sources queue what to store, one thread commits.

## Decision

Option 3, committing everything already queued in one transaction, without waiting for more. A
lone event commits at once, as before.

Option 2 was rejected because of what BlackBox is for: a flight recorder exists for the last
seconds before a freeze, and a timer would hold up to half a second of them in memory.

## Consequences

- Measured on a fresh database, 45 s each, counting fsyncs: **207 before, 37 to 47 after**, the
  same rows stored. A tick is one commit where it was four.
- A source that resumes from the database can read a position older than what it has just queued,
  and store it twice. Fixed by sources keeping their position in memory after reading it once.
  The first fix was a `flush()` barrier; a review called it a patch on the symptom, and it was
  replaced.
- What is still queued when the process is killed is lost: the writes of one commit, about what was
  lost before when a source was killed mid-write. Ending on its own, the process drains the queue.
- **Honest cost:** about 345 more lines than before, and the gain in steady state is small. The
  estimate before building it was 100 to 150 lines; it was wrong. What made it worth it in the end
  was less the fsyncs than what it enabled: one place for the write policy, records testable on
  their own (0002), and the database knowing no source.
