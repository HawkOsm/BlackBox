# Sourced by the deploy scripts: the few things that differ between Linux distributions.
# shellcheck shell=bash

# The package manager family from /etc/os-release, or from whichever manager is installed:
# pacman | apt | dnf | zypper | unknown
bb_family() {
    local id
    # shellcheck disable=SC1091
    for id in $(. /etc/os-release 2>/dev/null && echo "${ID:-} ${ID_LIKE:-}"); do
        case "$id" in
        arch) echo pacman && return ;;
        debian | ubuntu) echo apt && return ;;
        fedora | rhel | centos) echo dnf && return ;;
        suse | opensuse*) echo zypper && return ;;
        esac
    done
    local pm
    for pm in pacman apt-get dnf zypper; do
        command -v "$pm" >/dev/null && echo "${pm%-get}" && return
    done
    echo unknown
}

# Whether a user is in a group: bb_in_group user group
bb_in_group() {
    id -nG "$1" 2>/dev/null | tr ' ' '\n' | grep -qx "$2"
}

# The group that gets read access to the audit log and the energy counters: BLACKBOX_GROUP if set,
# else the first admin group the given user is in, else the first admin group that exists.
# Arch, Fedora and openSUSE use wheel; Debian and Ubuntu use sudo, and adm for reading logs.
# With none of them, says so on stderr and fails.
bb_admin_group() {
    local user="${1:-}" g
    [ -n "${BLACKBOX_GROUP:-}" ] && echo "$BLACKBOX_GROUP" && return
    if [ -n "$user" ] && [ "$user" != root ]; then
        for g in wheel adm sudo; do
            bb_in_group "$user" "$g" && echo "$g" && return
        done
    fi
    for g in wheel adm sudo; do
        getent group "$g" >/dev/null && echo "$g" && return
    done
    echo "no wheel, adm or sudo group here: set BLACKBOX_GROUP to the group blackbox reads with" >&2
    return 1
}

# Installs packages with the system's package manager. Run as root.
bb_install() {
    case "$(bb_family)" in
    pacman) pacman -S --needed --noconfirm "$@" ;;
    apt) DEBIAN_FRONTEND=noninteractive apt-get install -y "$@" ;;
    dnf) dnf install -y "$@" ;;
    zypper) zypper --non-interactive install "$@" ;;
    *) echo "unknown package manager: install $* yourself" >&2 && return 1 ;;
    esac
}

# The audit daemon's package name; the same everywhere except Debian and Ubuntu.
bb_audit_package() {
    case "$(bb_family)" in
    apt) echo auditd ;;
    *) echo audit ;;
    esac
}
