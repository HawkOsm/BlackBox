#!/usr/bin/env bash
# Run with: sudo bash deploy/setup-auditd.sh
# Installs the blackbox audit rules, caps the audit log at ~2 GB (4 x 500 MB),
# and makes the log readable by the wheel group so the collector runs without root.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
GROUP="${BLACKBOX_GROUP:-wheel}"

install -m 0640 "$HERE/blackbox.rules" /etc/audit/rules.d/blackbox.rules

conf=/etc/audit/auditd.conf
set_conf() { # key value
    if grep -qE "^\s*$1\s*=" "$conf"; then
        sed -i -E "s|^\s*$1\s*=.*|$1 = $2|" "$conf"
    else
        echo "$1 = $2" >> "$conf"
    fi
}
set_conf log_group "$GROUP"
set_conf max_log_file 500
set_conf num_logs 4
set_conf max_log_file_action ROTATE

systemctl enable --now auditd
# starting auditd already loaded the rules: drop ours first, so reloading does not fail on duplicates
for key in bb_identity bb_modules bb_boot; do auditctl -D -k "$key" >/dev/null 2>&1 || true; done
augenrules --load

mkdir -p /var/log/audit
chgrp "$GROUP" /var/log/audit
chmod 750 /var/log/audit
chmod 640 /var/log/audit/audit.log* 2>/dev/null || true

echo "auditd is running. Check as your normal user:"
echo "  tail -n 3 /var/log/audit/audit.log"
echo "  sudo auditctl -l"
