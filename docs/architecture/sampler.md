# Sampler and other timed sources

All of these are `Poller`s under `sources/poll/`.

## Sampler (`sampler/`)

Every 10 s it reads `/proc/stat`, `/proc/meminfo`, `/proc/loadavg` and `/proc/diskstats`. The
`sysstat` package is not used; the same numbers come from `/proc` with no dependency.

GPU data comes from `nvidia-smi` every 30 s. It is skipped while the card is runtime-suspended, so
it never wakes the GPU. It gets 5 s to answer and is then killed, so a hung driver cannot stall the
sampler. One call costs about 13 ms of CPU.

Each sample also keeps `cpu_mhz` (the fastest core's `scaling_cur_freq`, to see boosting) and
`gpu_mhz` (the graphics clock, from the same `nvidia-smi` call).

A `messages` row is written only when a threshold is crossed (`alert.rs`):

| Metric | Warning | Error |
|---|---|---|
| Memory used | 85% | 95% |
| CPU busy | 90% | – |
| Load average | above 2 × cores | – |
| GPU temperature | 85 C | – |
| GPU memory | 90% | – |

## Power (`sampler/power.rs`)

Each tick writes one `power` row: `on_battery`, `battery_pct`, `cpu_watt`, `gpu_watt`,
`battery_watt`.

- `cpu_watt`: difference of the RAPL package counter (`/sys/class/powercap/intel-rapl:0/energy_uj`)
  over the elapsed time, wrap-around included. A gap over 30 s (suspend) is dropped, not averaged in.
- The counter is root-only. `deploy/setup-sensors.sh` opens it to the admin group (`wheel`, `adm`
  or `sudo`). Without it `cpu_watt` stays NULL.
- `gpu_watt` is set only on ticks where `nvidia-smi` is polled.
- `battery_watt` is the whole machine's draw, set only while discharging.

## Sensors (`sampler/sensors.rs`)

Each tick reads all of `/sys/class/hwmon` (no root): temperatures (each CPU core and the package,
NVMe drives, DRAM modules, Wi-Fi, ACPI), fans, power inputs, and the RAPL domains the package row
does not cover (cores, uncore, DRAM, `psys` for the whole platform).

- One row per reading in `sensors(ts, kind, name, value)`, named `chip/label`. Chips that share a
  name (two NVMe drives) get their device in brackets.
- The viewer charts the CPU package, the hottest NVMe drive and platform power, and lists every
  reading under "Everything being logged".
- About 27 readings per tick on the development machine, a few hundred thousand rows a day.

## Power profiles (`profiles/`)

Every 10 s it asks power-profiles-daemon (`ActiveProfile`) and tccd, the TUXEDO Control Center
daemon (`GetActiveProfileJSON`, field `name`), over the system bus with `busctl --json`.
`powerprofilesctl` is not used: it is a Python script, about 100 ms a call.

A row goes into `profiles` only when a profile changes, as `info` (`tccd profile: Balanced →
Performance`). At start-up it reads the last stored profile of each daemon, so a restart stores
nothing new. A daemon that is not running is skipped.

## Temperature spikes (`spike/`)

Spikes come and go, so the 10 s sample and an after-the-fact `ps` both miss the cause.

**Detection.** Once a second it reads only the CPU package temperature (`coretemp` "Package id 0",
or `k10temp` "Tctl") and keeps the last 3 readings. Nothing is stored while it is calm. A reading is
a spike when it is:

- more than 15 C above the coolest of the 3 before it, or
- 80 C or more.

After a spike the next 60 readings are ignored, so a long hot stretch is captured about once a
minute.

**Capture.** Into `spikes` and `spike_procs`, with a `warning` in `messages`:

- The 5 busiest processes over the next second: user and system time from `/proc/<pid>/stat`, read
  twice, 1 s apart. `ps` shows an average over the process's whole life, which hides a burst in a
  long-running program. Then, over the next 0.5 s, context switches per second across all their
  threads (`/proc/<pid>/task/*/status`). Only for those five: switches are counted per thread, and
  reading every thread on the machine (~2,200 files here) cost ~25 ms a look.
- CPU package watts over that second (RAPL) and the fastest core's clock.
- AC or battery, the power-profiles-daemon and tccd profiles, Spotify's state (`playerctl`), and
  processes on the NVIDIA GPU (`nvidia-smi pmon`, skipped while the card sleeps).

The commands run after the measured second, so their CPU time is not in it.
