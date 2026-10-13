#!/usr/bin/env bash
# CI: builds and tests blackbox on one distribution, as root inside that distribution's container,
# then checks end to end that the recorder reads this distribution's package history.
# Runs the same way locally, for example:
#   podman run --rm -v "$PWD:/src:Z" -w /src -e CARGO_TARGET_DIR=/tmp/target fedora:latest \
#       bash .github/scripts/distro-test.sh
set -euo pipefail

cd "$(dirname "$0")/../.."
# shellcheck source=deploy/distro.sh
. deploy/distro.sh
family="$(bb_family)"
target="${CARGO_TARGET_DIR:-target}"
group() { echo "::group::$*"; }
endgroup() { echo "::endgroup::"; }

group "build dependencies ($family)"
case "$family" in
pacman)
    pacman -Sy --noconfirm >/dev/null
    bb_install base-devel sqlite curl
    ;;
apt)
    apt-get update -qq
    bb_install build-essential libsqlite3-dev curl ca-certificates
    ;;
dnf) bb_install gcc sqlite-devel ;;
zypper) bb_install gcc sqlite3-devel curl ;;
*) echo "unsupported distribution" >&2 && exit 1 ;;
esac
if ! command -v cargo >/dev/null; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
fi
rustc --version
endgroup

group "tests"
# The kernel's process connector only answers in the host's network namespace, and a container has
# its own (GitHub cannot give a container job the host network). The Rust job, which runs on the
# runner itself, covers that test.
cargo test --locked -- --skip collector::socket::tests::test_socket
endgroup

group "build"
# the test profile's dependencies are already built; a release build would compile them all again
cargo build --locked
endgroup

group "package history, end to end"
# a transaction made now must be the newest one the recorder finds, read from the real history
bb_install tree >/dev/null
report="$("$target/debug/blackbox" --check)"
echo "$report"
if ! grep -E '^  packages +[a-z]+ \(.*\): [0-9]+ transactions, newest .*\btree\b' <<<"$report"; then
    echo "the recorder did not find the 'tree' install in this distribution's package history" >&2
    exit 1
fi
endgroup

group "audit rules for this distribution"
rules="$(bash deploy/setup-auditd.sh --print-rules)"
grep -v '^#' <<<"$rules" | grep -c bb_
# every watch left in must name a path that exists here, or the rules would fail to load
while read -r _ path _; do
    [ -e "$path" ] || { echo "rule for missing path $path" >&2 && exit 1; }
done < <(grep '^-w ' <<<"$rules")
grep -q '^-a always,exit -F arch=b64' <<<"$rules"
# no user given: the first admin group that exists here
echo "admin group here: $(bb_admin_group "" 2>/dev/null || echo none)"
endgroup
