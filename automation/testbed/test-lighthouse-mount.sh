#!/usr/bin/env bash
# test-lighthouse-mount.sh — LH-JOIN-QNM-1: L1.5 founding-lighthouse mount acceptance.
#
# Codifies the 2026-06-29 manual VM-bed verify into a reusable regression test for
# the LH-JOIN-QNM-1 fix (the wedge-proof qnm-mount loop + the source-side stray-write
# guards). The bug: a fresh lighthouse left /mnt/mesh-storage a WEDGED FUSE mount
# (never mounted) because workers wrote strays into the unmounted mountpoint and the
# mount loop could neither clear nor survive the wedge. This test asserts a fresh
# founding lighthouse, built from the shipped RPM, ends with /mnt/mesh-storage a LIVE
# FUSE mount — not wedged, no reboot. (Audit Pass-7: a shared-mount feature needs a
# real-substrate assertion, not a tempdir mock — this is that assertion.)
#
# Hermetic: spins a clean VM from MDE-VM-golden, installs, founds QNM-Shared, asserts
# the mount, ALWAYS tears down. Result printed + published to the Bus.
#
# SCOPE: this exercises the founding-lighthouse path (local master) — the mount
# mechanism + wedge-proof loop + lizardfs install. The original bug's JOIN-against-a-
# remote-WAN-master race needs a live DO node and is NOT covered here (see WORKLIST
# LH-JOIN-QNM-1). The test widens mfsexports to the test node's own /16 because the
# VM bed has no Nebula overlay (setup-qnm-shared writes the 10.42.0.0/16 overlay ACL).
#
# Usage:  test-lighthouse-mount.sh [path/to.rpm]   (defaults to newest in $ARTIFACTS)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TESTBED="$HERE/farm-testbed.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${1:-$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)}"
[ -n "$RPM" ] && [ -f "$RPM" ] || { echo "no RPM (pass path/to.rpm or set MCNF_BUILD_ARTIFACTS)" >&2; exit 1; }

PASS=0; FAIL=0
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then echo "  PASS  $d"; PASS=$((PASS+1)); else echo "  FAIL  $d"; FAIL=$((FAIL+1)); fi; }
report() {
  local outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
  echo "== LH-JOIN-QNM mount: $PASS passed, $FAIL failed → $outcome =="
  command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/lighthouse-mount" --body-flag \
    "{\"tier\":\"L1.5-lh-mount\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL,\"rpm\":\"$(basename "$RPM")\"}" 2>/dev/null || true
  [ "$FAIL" -eq 0 ]
}
cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== LH-JOIN-QNM founding-lighthouse mount acceptance — $(basename "$RPM") =="
"$TESTBED" down >/dev/null 2>&1 || true
ip="$("$TESTBED" up 1 | tail -1 | awk '{print $2}')"
[ -n "$ip" ] || { echo "testbed didn't come up" >&2; exit 1; }
echo "founding lighthouse @ $ip"
RUN() { ssh -i "$KEY" $SSHO "mm@$ip" "$@"; }
SUBNET="$(echo "$ip" | cut -d. -f1-2).0.0/16"   # the test bed's LAN /16 (no overlay)

scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1
check "RPM installs cleanly"                       RUN "sudo dnf install -y /tmp/mm.rpm"
check "lizardfs lighthouse set installs"           RUN "sudo /usr/libexec/mackesd/mesh-install-lizardfs lighthouse && command -v mfsmaster mfschunkserver mfsmount"
check "master+chunkserver provision + activate"    RUN "sudo /usr/libexec/mackesd/setup-qnm-shared --master --chunkserver --master-ip $ip --listen $ip --goal 1 && systemctl is-active lizardfs-master lizardfs-chunkserver"
# Widen the export to the test node's subnet (the VM bed has no 10.42.x overlay).
RUN "grep -q '$SUBNET' /etc/mfs/mfsexports.cfg || echo '$SUBNET    /    rw,alldirs,maproot=0' | sudo tee -a /etc/mfs/mfsexports.cfg >/dev/null; sudo systemctl restart lizardfs-master" >/dev/null 2>&1
sleep 4
RUN "sudo /usr/libexec/mackesd/setup-qnm-shared --client --master-ip $ip" >/dev/null 2>&1
# Give the qnm-mount loop a few iterations to establish.
for _ in 1 2 3 4 5 6 7 8; do RUN "mountpoint -q /mnt/mesh-storage" >/dev/null 2>&1 && break; sleep 5; done
check "/mnt/mesh-storage is a live FUSE mount (not wedged)"  RUN "mountpoint -q /mnt/mesh-storage && mount | grep -q 'mesh-storage.*fuse'"
check "the mount is writable (round-trip a probe)"          RUN "sudo sh -c 'echo lhjoin-ok > /mnt/mesh-storage/.probe && grep -q lhjoin-ok /mnt/mesh-storage/.probe && rm -f /mnt/mesh-storage/.probe'"
check "qnm-shared.service is active"                        RUN "systemctl is-active qnm-shared.service"
check "qnm-mount wedge-proof self-test passes"              RUN "sudo /usr/libexec/mackesd/qnm-mount --self-test"
report
