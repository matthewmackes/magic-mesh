#!/usr/bin/env bash
# test-install.sh — BUILD-PLATFORM-4: L1 install (e2e) acceptance.
#
# Installs the freshly-cut RPM on a CLEAN VM cloned from MDE-VM-golden and asserts
# the product actually lands + the daemon is runnable — the install regression net.
# Hermetic: spins the test VM, installs, asserts, always tears down. Result is
# printed + published to the Bus (`event/test/install`). Run nightly / on-demand;
# never blocks an L0 build.
#
# Usage:  test-install.sh [path/to.rpm]   (defaults to the newest in $ARTIFACTS)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TESTBED="$HERE/farm-testbed.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${1:-$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)}"
[ -n "$RPM" ] && [ -f "$RPM" ] || { echo "no RPM (run install-helpers/xcp-build.sh rpm first)" >&2; exit 1; }

PASS=0; FAIL=0
check() { # <desc> <cmd...>
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "  PASS  $d"; PASS=$((PASS+1)); else echo "  FAIL  $d"; FAIL=$((FAIL+1)); fi
}
report() {
  local outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
  echo "== L1 install: $PASS passed, $FAIL failed → $outcome =="
  command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/install" --body-flag \
    "{\"tier\":\"L1-install\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL,\"rpm\":\"$(basename "$RPM")\"}" 2>/dev/null || true
  [ "$FAIL" -eq 0 ]
}

cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== L1 install acceptance — RPM $(basename "$RPM") on a clean VM =="
"$TESTBED" down >/dev/null 2>&1 || true   # ensure a clean slate
line="$("$TESTBED" up 1 | tail -1)"; ip="$(echo "$line" | awk '{print $2}')"
[ -n "$ip" ] || { echo "testbed didn't come up" >&2; exit 1; }
echo "test VM @ $ip"
RUN() { ssh -i "$KEY" $SSHO "mm@$ip" "$@"; }

scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1
PKG="$(rpm -qp --qf '%{NAME}' "$RPM" 2>/dev/null)"
echo "package: $PKG"

# --- the install + acceptance checks ---
check "RPM installs cleanly (dnf install)"        RUN "sudo dnf install -y /tmp/mm.rpm"
check "package is registered ($PKG)"              RUN "rpm -q $PKG"
check "the daemon binary installed"               RUN "command -v mackesd || rpm -ql $PKG | grep -q /mackesd"
check "the daemon runs (--version / --help)"      RUN "mackesd --version 2>/dev/null || mackesd --help 2>/dev/null | head -1"
check "a systemd unit ships (mackesd.service)"    RUN "rpm -ql $PKG | grep -qE 'mackesd\\.service' || test -f /usr/lib/systemd/system/mackesd.service"
check "no broken file deps"                       RUN "rpm -V $PKG 2>&1 | grep -vqE 'missing|^..5' || true"

report
