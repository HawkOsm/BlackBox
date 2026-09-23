#!/usr/bin/env bash
# User-level install, no sudo.   deploy/install.sh [install | uninstall [--purge]]
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

case "$ACTION" in
install)
    (cd "$REPO" && cargo build --release)
    mkdir -p "$BIN" "$DATA/ui" "$APPS" "$ICONS" "$UNITS"
    install -m755 "$REPO/target/release/blackbox" "$BIN/blackbox.new"
    mv -f "$BIN/blackbox.new" "$BIN/blackbox"
    install -m644 "$REPO/ui/data.py" "$DATA/ui/data.py"
    install -m755 "$REPO/ui/blackbox_app.py" "$DATA/ui/blackbox_app.py"
    install -m644 "$REPO/deploy/blackbox.service" "$UNITS/blackbox.service"
    install -m644 "$REPO/deploy/io.github.hawkosm.Blackbox.svg" "$ICONS/io.github.hawkosm.Blackbox.svg"
    sed "s|@HOME@|$HOME|g" "$REPO/deploy/io.github.hawkosm.Blackbox.desktop" > "$APPS/io.github.hawkosm.Blackbox.desktop"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
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
