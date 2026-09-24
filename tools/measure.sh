#!/usr/bin/env bash
# Measures the cost of every process in the blackbox systemd units over an interval:
# CPU (% of one core), memory (PSS, so shared pages are not counted twice) and
# context switches per second (a proxy for wakeups).
# Usage: tools/measure.sh [seconds] [unit-glob]
set -euo pipefail
SECS="${1:-60}"
GLOB="${2:-blackbox*.service}"
HZ=$(getconf CLK_TCK)

pids() {
    for unit in $(systemctl --user list-units --type=service --state=running --plain --no-legend "$GLOB" | awk '{print $1}'); do
        cat "/sys/fs/cgroup$(systemctl --user show -p ControlGroup --value "$unit")/cgroup.procs"
    done
}

snap() { # prints: cpu_ticks ctxt_switches
    local ticks=0 sw=0
    for p in $(pids); do
        read -r ut st < <(awk '{print $14, $15}' "/proc/$p/stat" 2>/dev/null || echo "0 0")
        ticks=$((ticks + ut + st))
        for t in /proc/$p/task/*/status; do
            sw=$((sw + $(awk '/ctxt_switches/ {s += $2} END {print s+0}' "$t" 2>/dev/null || echo 0)))
        done
    done
    echo "$ticks $sw"
}

read -r t0 s0 < <(snap)
sleep "$SECS"
read -r t1 s1 < <(snap)

printf '%-8s %-12s %8s\n' PID NAME PSS_KB
pss_total=0
for p in $(pids); do
    pss=$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null || echo 0)
    pss_total=$((pss_total + pss))
    printf '%-8s %-12s %8s\n' "$p" "$(cat /proc/$p/comm)" "$pss"
done
echo "processes:        $(pids | wc -l)"
echo "PSS total:        $((pss_total / 1024)) MB ($pss_total kB)"
awk -v d=$((t1 - t0)) -v hz="$HZ" -v s="$SECS" 'BEGIN {printf "CPU:              %.3f%% of one core\n", 100 * d / hz / s}'
awk -v d=$((s1 - s0)) -v s="$SECS" 'BEGIN {printf "context switches: %.1f /s\n", d / s}'
