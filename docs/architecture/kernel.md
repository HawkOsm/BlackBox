# Collector (`sources/kernel/collector/`)

Subscribes to the kernel's process connector over a raw netlink socket and receives exec and exit
events.

## What is stored

One rule, in `exit.rs`, shared by the kernel filter and the collector:

| Exit | Stored as |
|---|---|
| killed by SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV or SIGSYS (a crash) | error |
| exit status 101 (a Rust panic) | error |
| any other exit status except 0 and 1 | warning |
| status 0 or 1, SIGTERM, SIGHUP, SIGINT, SIGPIPE, SIGKILL, and a shell's 128+N report of those (129, 130, 137, 141, 143) | not stored |

Status 1 is what `grep`, `pgrep` and `modprobe` return for "not found". Of 3,270 exits recorded on
the development machine, status 0 and 1 were 99.7%, and every status-1 exit came from `pgrep` or
`modprobe`.

## Kernel-side filter

A classic BPF program on the socket applies the rule in the kernel. Only exec events (needed for
names) and exits worth storing wake the process. Fork events, thread exits and clean exits are
dropped before they are queued. A unit test runs the filter through a small BPF interpreter for all
65,536 wait statuses and checks it agrees with `exit::severity`.

## Batching

After each event the collector sleeps `BLACKBOX_BATCH_MS` (100 ms), then drains the queue. This
gives about 7 wakeups per second.

## Process names

The exit event has no name, and `/proc/<pid>` is usually gone by then. So:

1. At start-up and on every exec the collector stores pid → name (from `/proc/<pid>/comm`).
2. On exit it looks the name up.
3. If that fails, the summary uses the parent's name: `? (pid 4242, child of bash)`.
4. A crash that dumps core is named afterwards from systemd-coredump's journal entry
   (`COREDUMP_PID`, `COREDUMP_COMM`, `COREDUMP_EXE`), which also fills `custom.exe` with the full
   path.

If the kernel queue overflows (`ENOBUFS`) the collector logs it and carries on.

## In code

- `decode` reads the raw netlink message into an `Event` (`Exec` or `Exit`). It holds the one
  `unsafe` read.
- `remember` keeps the name of a new program.
- `exit_record` turns an exit worth keeping into an `Exit` record.

## Reading an exit code

`exit_code` is a raw wait status: the low 7 bits are the killing signal (0 if none), bit 7 means a
core was dumped, and `exit_code >> 8` is the normal exit status.

| Value | Meaning |
|---|---|
| 256 | exited with status 1 |
| 25856 | exited with status 101 (a Rust panic) |
| 11 | killed by SIGSEGV |
| 139 | SIGSEGV, core dumped |
| 15 | killed by SIGTERM |
| 36608 | exited with status 143 (a shell reporting that its child got SIGTERM) |
