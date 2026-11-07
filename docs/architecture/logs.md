# Log sources and boots

## journald (`sources/follow/journald/`)

Two `journalctl --follow --output=json` processes:

| Follower | Filter | Stored as |
|---|---|---|
| warnings | `--priority=warning` | priority 0 to 3 `error`, 4 `warning` |
| panics | `--priority=notice..info --grep='panicked at'` | `error` (Rust panics are logged at info, below the warning filter) |

- **Exact resume.** Each stored entry saves its journal cursor in `state`, in the same transaction
  as the row. A restart continues with `--after-cursor`. With no cursor (first run) it resumes by
  time and checks only the overlapping second for duplicates, so identical messages logged in the
  same second are kept as real repeats. If journald vacuumed past the saved cursor, the follower
  forgets it and resumes by time.
- **Clean text.** Terminal colour codes some services log are stripped. Empty messages (the kernel
  logs bare continuation lines at error priority) are skipped.
- **Routine noise** (`sources/routine.rs`). Messages that look alarming but are routine on any Linux
  machine are stored as `info`: the kernel's "watchdog did not stop!" at shutdown, `sudo -n` probes
  (sudo logs "a password is required" at alert level), timed-out or cancelled password prompts, and
  similar. Each rule matches text the program always prints, never a user, host, device address or
  path. A fault of one machine is fixed there, not listed. When the list changes, stored rows are
  re-rated once at start-up. The first list moved 531 of 1,601 errors to `info` (31% of the last
  week's) on the development machine.
- systemd-coredump entries also name the matching crash in `custom` (see [kernel.md](kernel.md)),
  and the viewer shows their stack trace next to the crash.

## auditd (`sources/follow/auditd/`)

Follows the audit log with `tail -F` and keeps low-volume, high-value records:

| Record | Stored as |
|---|---|
| `USER_AUTH`, `USER_LOGIN`, `USER_ACCT` with `res=failed` | warning |
| successful `USER_AUTH`, `USER_LOGIN`, `USER_CMD` (sudo) | row in `auditd`, no alert |
| `SYSCALL` with a `bb_*` key (`deploy/blackbox.rules`) | warning; module loads in the first 3 minutes after boot are `info` (about 30 per boot) |
| `ANOM_*` (crashes, promiscuous mode) and `AVC` denials | error; promiscuous mode on a bridge port (Docker, Podman, libvirt, VM taps), or leaving it, is info |

Reading a line is two steps. `classify` says what the record is (`Found`: failed login, login, hit
on one of our rules, promiscuous mode, anomaly). `Found::alert` says how much it matters and what
the viewer shows. The 3-minute module-load rule is a `match` on `RuleHit { key: "bb_modules", .. }`
there.

**Resume.** On restart it binary-searches the log for the first record newer than the last stored
one and follows from there, so downtime events are picked up without reading the whole file.
Records that rotated into `audit.log.1` meanwhile are missed. If the log is missing or unreadable it
checks again every 30 s.

**Setup.** `deploy/setup-auditd.sh` installs the rules (only watches whose paths exist), caps the log
at 4 × 500 MB, and makes it readable by the admin group.

## Package changes (`sources/poll/packages/`)

One row per transaction: the command, every package changed, and a one-line summary. Packages that
most often break a machine (kernel, NVIDIA, Mesa, systemd, glibc, initramfs tools, GRUB, shim,
microcode, firmware) are listed first, with versions:

`pacman: upgraded 83, installed 1 · mesa 1:26.2.2-1 → 1:26.2.3-1, linux 7.2.4.arch1-2 → 7.2.6.arch2-1, …`

A reader runs for every history that exists. Each turns its format into the same change lines
(`upgraded name (old -> new)`):

| Reader | History | End of a transaction |
|---|---|---|
| `pacman.rs` | `/var/log/pacman.log` | `transaction completed` / `failed` |
| `apt.rs` | `/var/log/apt/history.log` (local time) | `End-Date:` |
| `zypper.rs` | `/var/log/zypp/history` (local time) | the next `command` line, or 5 min without a new line |
| `dnf.rs` | dnf5 `transaction_history.sqlite` or dnf4 `history.sqlite`, read-only | `dt_end` set |

- dnf5 writes no text log, and dnf4's `dnf.rpm.log` has no transaction boundaries, so dnf's history
  database is read. An upgrade is two rows there (new version and the one it replaced), paired back
  into one line.
- Text logs change only while the package manager runs, so they are checked every 30 s by size. The
  database is queried only when its modification time (or its `-wal`'s) changes.
- The first run imports the whole history (139 transactions on the development machine).
- After a restart each reader starts from the newest second already stored for its manager and skips
  any transaction already stored with the same time and changes. Two transactions in the same second
  are neither lost nor stored twice.
- The viewer lists transactions from the week before any event it shows.

## Boots (`sources/once/boot/`)

A freeze, kernel panic or power loss kills the collector with everything else, so nothing records it
at the time. Once per boot, at start-up, the recorder:

1. Records that the system started, with the kernel release.
2. Reads the previous boot's last journal entry.

A clean shutdown always ends with journald's "Journal stopped" (`MESSAGE_ID`
`d93fb3c9c24d451a97cea615ce59c00b`). Anything else is stored as an `error` at the moment the
previous boot stopped, so the viewer shows the minutes before it. A last entry from suspend
("Filesystems sync", "PM: suspend") is described as a suspend that never resumed. On the
development machine 66 of 73 earlier boots ended cleanly; of the other 7, two stopped while
suspending.

After a clean shutdown, errors and warnings from its last 30 s are relabelled `info` with "(during
shutdown)": programs being killed as the session is torn down, not failures (on 2026-09-24 Hyprland
and Spotify both aborted during a poweroff). This runs at every start and is idempotent.
