#!/usr/bin/env bash
# Run with: sudo bash deploy/setup-sensors.sh
# Opens the CPU energy counters (package, cores, uncore, DRAM, whole platform) to group wheel, the
# group that already reads the audit log, so the collector can log watts without root. Since
# Linux 5.10 they are root-only because fine-grained energy readings can leak data ("PLATYPUS").
# Skip this script and the watt readings are simply left out. Temperatures and fans need nothing.
set -euo pipefail

GROUP="${BLACKBOX_GROUP:-wheel}"

cat > /etc/udev/rules.d/60-blackbox-rapl.rules <<RULES
# installed by blackbox deploy/setup-sensors.sh
ACTION=="add|change", SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", RUN+="/bin/sh -c 'chgrp $GROUP /sys%p/energy_uj; chmod 0440 /sys%p/energy_uj'"
RULES

udevadm control --reload
udevadm trigger --subsystem-match=powercap --action=add

echo "check as your normal user:"
echo "  cat /sys/class/powercap/intel-rapl:0/energy_uj"
