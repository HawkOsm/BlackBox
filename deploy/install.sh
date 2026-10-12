#!/usr/bin/env bash
# User-level install on any systemd Linux; the one optional sudo step is the sensor permissions below.
#   deploy/install.sh [install | uninstall [--purge] | check]
#   BLACKBOX_SYSTEM_SETUP=yes|no answers that step without asking (default: ask on a terminal)
#   collector: always-on systemd user service (starts at boot, lingering is enabled)
#   viewer:    "Blackbox" app in the application menu, runs only while its window is open; skipped
#              on a machine without GTK 4 and libadwaita for Python, such as a server
#   check:     what the installed recorder can see on this machine (blackbox --check)
set -euo pipefail

ACTION="${1:-install}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=deploy/distro.sh
. "$REPO/deploy/distro.sh"
BIN="$HOME/.local/bin"
DATA="$HOME/.local/share/blackbox"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
UNITS="$HOME/.config/systemd/user"

# The CPU energy counters are root-only by default (since Linux 5.10). deploy/setup-sensors.sh opens
# them to your admin group. Temperatures, fans and DRAM sensors need nothing: hwmon is world-readable.
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
            read -r -p "Let blackbox log CPU watts? Runs 'sudo bash deploy/setup-sensors.sh' (a udev rule for your admin group). [y/N] " answer
        else
            answer=no
        fi
    fi
    case "$answer" in
    y | Y | yes) sudo bash "$REPO/deploy/setup-sensors.sh" || echo "note: setup-sensors.sh failed; CPU watts stay empty" ;;
    *) echo "skipped. To log CPU watts later: sudo bash $REPO/deploy/setup-sensors.sh" ;;
    esac
}

# What to install for the build and for the viewer, in this distribution's package names.
build_deps() {
    case "$(bb_family)" in
    pacman) echo "sudo pacman -S --needed rust sqlite" ;;
    apt) echo "sudo apt install build-essential libsqlite3-dev   (and Rust from https://rustup.rs: the packaged one is too old)" ;;
    dnf) echo "sudo dnf install cargo gcc sqlite-devel   (or Rust from https://rustup.rs if dnf's is older than $MSRV)" ;;
    zypper) echo "sudo zypper install cargo gcc sqlite3-devel" ;;
    *) echo "Rust $MSRV or newer (https://rustup.rs), a C compiler and SQLite's development files" ;;
    esac
}
viewer_deps() {
    case "$(bb_family)" in
    pacman) echo "sudo pacman -S --needed python-gobject gtk4 libadwaita" ;;
    apt) echo "sudo apt install python3-gi gir1.2-gtk-4.0 gir1.2-adw-1" ;;
    dnf) echo "sudo dnf install python3-gobject gtk4 libadwaita" ;;
    zypper) echo "sudo zypper install python3-gobject typelib-1_0-Gtk-4_0 typelib-1_0-Adw-1" ;;
    *) echo "PyGObject with GTK 4 and libadwaita" ;;
    esac
}

MSRV="$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$REPO/Cargo.toml")"

# Stops early, with the install command for this distribution, instead of failing deep in a build.
check_build_deps() {
    local have
    if ! command -v cargo >/dev/null || ! command -v cc >/dev/null; then
        echo "missing Rust (cargo) or a C compiler. On this system: $(build_deps)" >&2
        exit 1
    fi
    have="$(rustc --version | awk '{print $2}')"
    if [ "$(printf '%s\n%s\n' "$MSRV" "$have" | sort -V | head -n1)" != "$MSRV" ]; then
        echo "Rust $have is older than the $MSRV this needs. Update it (rustup update) or: $(build_deps)" >&2
        exit 1
    fi
}

has_viewer_deps() {
    python3 -c 'import gi; gi.require_version("Gtk", "4.0"); gi.require_version("Adw", "1")' 2>/dev/null
}

install_viewer() {
    mkdir -p "$DATA/ui" "$APPS" "$ICONS"
    # drop modules a previous version installed and this one no longer has
    rm -f "$DATA"/ui/*.py
    install -m644 "$REPO"/ui/*.py "$DATA/ui/"
    chmod 755 "$DATA/ui/blackbox_app.py"
    install -m644 "$REPO/deploy/io.github.hawkosm.Blackbox.svg" "$ICONS/io.github.hawkosm.Blackbox.svg"
    sed "s|@HOME@|$HOME|g" "$REPO/deploy/io.github.hawkosm.Blackbox.desktop" > "$APPS/io.github.hawkosm.Blackbox.desktop"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
}

case "$ACTION" in
install)
    check_build_deps
    if ! (cd "$REPO" && cargo build --release --locked); then
        echo "build failed. If the linker cannot find -lsqlite3: $(build_deps)" >&2
        exit 1
    fi
    mkdir -p "$BIN" "$DATA" "$UNITS"
    install -m755 "$REPO/target/release/blackbox" "$BIN/blackbox.new"
    mv -f "$BIN/blackbox.new" "$BIN/blackbox"
    install -m644 "$REPO/deploy/blackbox.service" "$UNITS/blackbox.service"
    if has_viewer_deps; then
        install_viewer
        viewer="Open 'Blackbox' from the app menu."
    else
        viewer="Viewer skipped: no GTK 4 and libadwaita for Python here. For the app: $(viewer_deps), then run this again."
    fi
    # keep the user service running from boot, and after logging out
    loginctl enable-linger "$USER" 2>/dev/null || echo "note: could not enable lingering; the collector will only run while you are logged in"
    sensors_setup
    systemctl --user daemon-reload
    systemctl --user enable blackbox.service
    systemctl --user restart blackbox.service
    echo "installed. collector: $(systemctl --user is-active blackbox.service). $viewer"
    echo
    "$BIN/blackbox" --check
    ;;
check)
    # an installed binary older than --check refuses it: fall back to this checkout's build
    "$BIN/blackbox" --check 2>/dev/null || (cd "$REPO" && cargo run --quiet --release --locked -- --check)
    echo "  service    $(systemctl --user is-active blackbox.service 2>/dev/null || echo "not running")"
    ;;
uninstall)
    systemctl --user disable --now blackbox.service 2>/dev/null || true
    rm -f "$UNITS/blackbox.service" "$BIN/blackbox" "$APPS/io.github.hawkosm.Blackbox.desktop" "$ICONS/io.github.hawkosm.Blackbox.svg"
    rm -rf "$DATA/ui"
    systemctl --user daemon-reload
    if [ "${2:-}" = "--purge" ]; then rm -rf "$DATA"; echo "removed, including recorded data."; else echo "removed. recorded data kept in $DATA"; fi
    ;;
*)
    echo "usage: $0 [install | uninstall [--purge] | check]" >&2
    exit 1
    ;;
esac
