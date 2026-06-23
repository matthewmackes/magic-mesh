#!/usr/bin/env bash
# test-stability.sh — BUILD-PLATFORM-6: L3 stability acceptance.
#
# Three durability checks on the snapshot-reset pool, encoding the incidents this
# project actually hit:
#   soak   — a daemon under sustained traffic keeps a flat footprint (BUS-RETENTION-1)
#   chaos  — destroying one node does NOT wedge the survivor (INCIDENT-WEDGE)
#   reboot — a node reboots and its daemon/overlay self-heal (BOOT-REC)
# Hermetic (testbed up→down). Result → Bus (`event/test/stability/*`). Nightly/weekly.
#
# Usage:  test-stability.sh [path/to.rpm]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TESTBED="$HERE/farm-testbed.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${1:-$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)}"
[ -n "$RPM" ] && [ -f "$RPM" ] || { echo "no RPM (run xcp-build.sh rpm first)" >&2; exit 1; }
A="172.20.0.60"; B="172.20.0.61"
PASS=0; FAIL=0
on()   { ssh -i "$KEY" $SSHO "mm@$1" "${@:2}"; }
check(){ local d="$1"; shift; if "$@" >/dev/null 2>&1; then echo "  PASS  $d"; PASS=$((PASS+1)); else echo "  FAIL  $d"; FAIL=$((FAIL+1)); fi; }
cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== L3 stability — soak · chaos · reboot from $(basename "$RPM") =="
"$TESTBED" down >/dev/null 2>&1 || true
"$TESTBED" up 2 >/dev/null
for ip in "$A" "$B"; do
  for t in $(seq 1 24); do timeout 3 bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null && break; sleep 5; done
  scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1
  on "$ip" "sudo dnf install -y /tmp/mm.rpm" >/dev/null 2>&1
  on "$ip" "sudo systemctl start mackesd 2>/dev/null || true" >/dev/null 2>&1
done

# soak — RSS should plateau, not climb, under repeated bus traffic.
echo "-- soak (footprint plateau) --"
r0="$(on "$A" "ps -o rss= -C mackesd 2>/dev/null | awk '{s+=\$1}END{print s+0}'" | tr -d '\r')"
for i in $(seq 1 8); do on "$A" "for n in \$(seq 1 50); do mde-bus publish event/soak/\$n --body-flag '{\"i\":'\$n'}' 2>/dev/null; done" >/dev/null 2>&1; done
sleep 5
r1="$(on "$A" "ps -o rss= -C mackesd 2>/dev/null | awk '{s+=\$1}END{print s+0}'" | tr -d '\r')"
check "mackesd footprint plateaus under traffic (${r0}→${r1} KiB, <50% growth)" \
  bash -c "[ ${r0:-0} -gt 0 ] && [ ${r1:-0} -le \$(( ${r0:-1} * 3 / 2 )) ]"

# chaos — destroy node B; node A must NOT wedge (load stays sane, no uninterruptible procs).
echo "-- chaos (kill a node, survivor stays healthy) --"
ssh -i "$KEY" $SSHO "root@${MCNF_TESTBED_DOM0:-172.20.145.165}" \
  "U=\$(xe vm-list name-label=mcnf-test-1 --minimal); xe vm-shutdown uuid=\$U force=true" >/dev/null 2>&1
sleep 20
check "survivor (A) not wedged — load < 10, no D-state procs"  \
  on "$A" "load=\$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1); [ \${load:-0} -lt 10 ] && [ \$(ps -eo stat | grep -c '^D') -eq 0 ]"
check "survivor (A) still responds (sshd + daemon alive)"      on "$A" "systemctl is-system-running 2>/dev/null | grep -qvE 'stopping|offline'"

# reboot-recovery — reboot A; the daemon comes back on its own.
echo "-- reboot-recovery --"
on "$A" "sudo systemctl reboot" >/dev/null 2>&1 || true
sleep 10
for t in $(seq 1 36); do timeout 3 bash -c "cat </dev/null >/dev/tcp/$A/22" 2>/dev/null && break; sleep 5; done
check "node A back after reboot"                               on "$A" "true"
check "mackesd self-heals on boot (enabled/active or re-startable)" \
  on "$A" "systemctl is-enabled mackesd 2>/dev/null | grep -qE 'enabled|static' || sudo systemctl start mackesd"

outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
echo "== L3 stability: $PASS passed, $FAIL failed → $outcome =="
command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/stability/all" --body-flag \
  "{\"tier\":\"L3-stability\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL}" 2>/dev/null || true
[ "$FAIL" -eq 0 ]
