#!/usr/bin/env bash
# User-level install; the one optional sudo step is the sensor permissions below.
#   deploy/install.sh [install | uninstall [--purge]]
#   BLACKBOX_SYSTEM_SETUP=yes|no answers that step without asking (default: ask on a terminal)
#   collector: always-on systemd user service (starts at boot, lingering is enabled)
#   viewer:    "Blackbox" app in the application menu, runs only while its window is open
set -euo pipefail

ACTION="${1:-install}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$HOME/.local/bin"
DATA="$HOME/.local/share/blackbox"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
UNITS="$HOME/.config/systemd/user"

# The CPU energy counters are root-only by default (since Linux 5.10). deploy/setup-sensors.sh opens
# them to group wheel. Temperatures, fans and DRAM sensors need nothing: hwmon is world-readable.
sensors_setup() {
    local blocked=0 f
    for f in /sys/class/powercap/intel-rapl:*/energy_uj; do
        [ -e "$f" ] || continue # no Intel RAPL on this machine
        [ -r "$f" ] || blocked=1
    done
    [ "$blocked" = 1 ] || { echo "energy counters: already readable"; return 0; }
    local answer="${BLACKBOX_SYSTEM_SETUP:-}"
    if [ -z "$answer" ]; then
        if [ -t 0 ]; then
            read -r -p "Let blackbox log CPU watts? Runs 'sudo bash deploy/setup-sensors.sh' (a udev rule for group wheel). [y/N] " answer
        else
            answer=no
        fi
    fi
    case "$answer" in
    y | Y | yes) sudo bash "$REPO/deploy/setup-sensors.sh" || echo "note: setup-sensors.sh failed; CPU watts stay empty" ;;
    *) echo "skipped. To log CPU watts later: sudo bash $REPO/deploy/setup-sensors.sh" ;;
    esac
}

case "$ACTION" in
install)
    (cd "$REPO" && cargo build --release)
    mkdir -p "$BIN" "$DATA/ui" "$APPS" "$ICONS" "$UNITS"
    install -m755 "$REPO/target/release/blackbox" "$BIN/blackbox.new"
    mv -f "$BIN/blackbox.new" "$BIN/blackbox"
    # drop modules a previous version installed and this one no longer has
    rm -f "$DATA"/ui/*.py
    install -m644 "$REPO"/ui/*.py "$DATA/ui/"
    chmod 755 "$DATA/ui/blackbox_app.py"
    install -m644 "$REPO/deploy/blackbox.service" "$UNITS/blackbox.service"
    install -m644 "$REPO/deploy/io.github.hawkosm.Blackbox.svg" "$ICONS/io.github.hawkosm.Blackbox.svg"
    sed "s|@HOME@|$HOME|g" "$REPO/deploy/io.github.hawkosm.Blackbox.desktop" > "$APPS/io.github.hawkosm.Blackbox.desktop"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    # keep the user service running from boot, and after logging out
    loginctl enable-linger "$USER" 2>/dev/null || echo "note: could not enable lingering; the collector will only run while you are logged in"
    sensors_setup
    systemctl --user daemon-reload
    systemctl --user enable blackbox.service
    systemctl --user restart blackbox.service
    echo "installed. collector: $(systemctl --user is-active blackbox.service). Open 'Blackbox' from the app menu."
    ;;
uninstall)
    systemctl --user disable --now blackbox.service 2>/dev/null || true
    rm -f "$UNITS/blackbox.service" "$BIN/blackbox" "$APPS/io.github.hawkosm.Blackbox.desktop" "$ICONS/io.github.hawkosm.Blackbox.svg"
    rm -rf "$DATA/ui"
    systemctl --user daemon-reload
    if [ "${2:-}" = "--purge" ]; then rm -rf "$DATA"; echo "removed, including recorded data."; else echo "removed. recorded data kept in $DATA"; fi
    ;;
*)
    echo "usage: $0 [install | uninstall [--purge]]" >&2
    exit 1
    ;;
esac
