#!/usr/bin/env bash
# Run with: sudo bash deploy/setup-auditd.sh
#       or: bash deploy/setup-auditd.sh --print-rules   (show the rules this machine gets; no root)
# Installs auditd if it is missing, loads the blackbox audit rules, caps the audit log at ~2 GB
# (4 x 500 MB), and makes the log readable by your admin group (wheel, adm or sudo, whichever you are
# in; set BLACKBOX_GROUP to choose) so the collector runs without root.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/distro.sh
. "$HERE/distro.sh"

# The rules, minus watches on paths this machine does not have: blackbox.rules lists the boot files
# of every distribution (mkinitcpio, dracut, initramfs-tools, GRUB, systemd-boot), and a watch on a
# missing directory fails to load. A path that appears later is covered by running this again.
rules() {
    local line path
    while IFS= read -r line; do
        if [[ "$line" =~ ^-w[[:space:]]+([^[:space:]]+) ]]; then
            path="${BASH_REMATCH[1]}"
            [ -e "$path" ] || continue
        fi
        printf '%s\n' "$line"
    done < "$HERE/blackbox.rules"
}

if [ "${1:-}" = "--print-rules" ]; then
    rules
    exit 0
fi
[ "$(id -u)" = 0 ] || { echo "run as root: sudo bash $0" >&2; exit 1; }

GROUP="$(bb_admin_group "${SUDO_USER:-}")" || exit 1

if ! command -v augenrules >/dev/null; then
    echo "installing the audit daemon ($(bb_audit_package))"
    bb_install "$(bb_audit_package)"
fi

mkdir -p /etc/audit/rules.d
rules > /etc/audit/rules.d/blackbox.rules
chmod 0640 /etc/audit/rules.d/blackbox.rules

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
# an auditd that was already running (the default on Fedora, RHEL and Debian) rereads its config
auditctl --signal reload >/dev/null 2>&1 || pkill -HUP -x auditd || true
# starting auditd already loaded the rules: drop ours first, so reloading does not fail on duplicates
for key in bb_identity bb_modules bb_boot; do auditctl -D -k "$key" >/dev/null 2>&1 || true; done
augenrules --load

mkdir -p /var/log/audit
chgrp "$GROUP" /var/log/audit
chmod 750 /var/log/audit
chgrp "$GROUP" /var/log/audit/audit.log* 2>/dev/null || true
chmod 640 /var/log/audit/audit.log* 2>/dev/null || true

echo "auditd is running, its log readable by group $GROUP. Check as your normal user:"
echo "  tail -n 3 /var/log/audit/audit.log"
echo "  sudo auditctl -l | grep bb_"
[ -n "${SUDO_USER:-}" ] && ! bb_in_group "$SUDO_USER" "$GROUP" &&
    echo "note: $SUDO_USER is not in group $GROUP yet: sudo usermod -aG $GROUP $SUDO_USER, then log in again"
exit 0
