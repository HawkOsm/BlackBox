---
name: verify-recorder
description: Prove a BlackBox change works on the real machine before calling it done — tests that are shown to fail when the code is broken, the real binary on a throwaway database, row counts, fsync counts, a real crash and journal line, a restart. Use before reporting any change complete, or when asked "does it work?".
---

# Verifying the recorder

Adapted from skysat-mlops `verify-pipeline`. "It should work" is not a result, and an LLM reviewing
its own code shares its own blind spots: every check below gets its answer from the compiler, the
tests or the machine, not from an opinion.

## Gates, in order

1. **Build and lint:** `cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), `cargo test`.
2. **Tests can fail.** For each new or changed test, break the code it guards (comment out the
   line, invert the condition), run the test, paste the failure, restore, run again.
3. **Real binary, throwaway database.** Never the installed service's database:
   ```bash
   BLACKBOX_DB=$SCRATCH/t.db timeout 45 ./target/release/blackbox
   sqlite3 $SCRATCH/t.db "select source, count(*) from messages group by 1"
   ```
   Compare the row counts with the previous build on the same run length.
4. **Real events.** While it runs: `sh -c 'kill -SEGV $$'` (a crash) and
   `logger -p user.warning bbtest-$$` (a journal warning). Both must be stored.
5. **Restart.** Run it again on the same database: no row may be stored twice (packages, journald,
   boot).
6. **Cost, when the change claims a cost win.** Count fsyncs with an `LD_PRELOAD` shim that wraps
   `fsync`/`fdatasync` (strace is not installed); before and after, same run length.
7. **`blackbox --check`** still reports every source.

CI's distribution job (Arch, Debian, Ubuntu, Fedora, Rocky, openSUSE) is the only check for apt,
dnf and zypper on real systems: it runs on push.

## Report

```
Verified:    <commands run>
Result:      <key output lines>
Not checked: <what was skipped and why>
```
