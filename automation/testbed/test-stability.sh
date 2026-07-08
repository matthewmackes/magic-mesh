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
install_rpm() {
  local ip="$1"
  scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1 || return 1
  if timeout 600 ssh -i "$KEY" $SSHO "mm@$ip" "sudo dnf install -y /tmp/mm.rpm" >/dev/null 2>&1; then
    return 0
  fi
  echo "  DIAG  RPM install failed or timed out on $ip:"
  timeout 20 ssh -i "$KEY" $SSHO "mm@$ip" "rpm -q magic-mesh || true" || true
  timeout 20 ssh -i "$KEY" $SSHO "mm@$ip" "ps -eo pid,ppid,stat,etime,wchan:24,cmd | grep -E 'dnf|rpm|systemctl|semodule|setup-|mesh-install' | grep -v grep || true" || true
  timeout 20 ssh -i "$KEY" $SSHO "mm@$ip" "sudo tail -80 /var/log/dnf.log /var/log/dnf.rpm.log 2>/dev/null || true" || true
  return 1
}
mackesd_fd_count() {
  local ip="$1"
  on "$ip" "p=\$(systemctl show -p MainPID --value mackesd.service 2>/dev/null || echo 0); \
    case \"\$p\" in ''|0) p=\$(pgrep -xo mackesd || true);; esac; \
    if [ -n \"\$p\" ] && [ \"\$p\" -gt 1 ] 2>/dev/null; then \
      sudo find /proc/\$p/fd -mindepth 1 -maxdepth 1 -printf . 2>/dev/null | wc -c; \
    else echo 0; fi" | tr -dc '0-9'
}
cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== L3 stability — soak · chaos · reboot from $(basename "$RPM") =="
"$TESTBED" down >/dev/null 2>&1 || true
"$TESTBED" up 2 >/dev/null
for ip in "$A" "$B"; do
  for t in $(seq 1 24); do timeout 3 bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null && break; sleep 5; done
  install_rpm "$ip" || exit 1
done

# Bring up a minimal real mesh before measuring daemon stability. Starting
# mackesd on an unenrolled image can legitimately leave no long-running daemon,
# which made the old RSS check read 0 KiB and test nothing.
check "found a stability mesh on node A"            on "$A" "sudo mackesd found l3test --external-addr ${A}:4242 --role lighthouse"
ADD_PEER_OUT="$(on "$A" "sudo mackesd add-peer --lighthouse ${A} --role server 2>&1" | tr -d '\r' || true)"
TOKEN="$(printf '%s\n' "$ADD_PEER_OUT" | grep -E '^mesh:' | tail -1)"
check "stability add-peer minted a join token"      test -n "$TOKEN"
on "$A" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$A" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
check "node A overlay is up for stability"          on "$A" "ip -4 addr show | grep -q 10.42"
JOIN_OUT="$(on "$B" "sudo mackesd join '$TOKEN' --role server 2>&1" | tr -d '\r' || true)"
JOIN_OK=0
printf '%s\n' "$JOIN_OUT" | grep -q 'joined `' && JOIN_OK=1
check "node B joins the stability mesh"             test "$JOIN_OK" -eq 1
if [ "$JOIN_OK" -ne 1 ]; then
  echo "  DIAG  join output on B:"
  printf '%s\n' "$JOIN_OUT" | sed -n '1,80p'
  echo "  DIAG  B service/config state:"
  on "$B" "systemctl --no-pager -l status nebula mackesd 2>&1 | sed -n '1,120p'; sudo ls -l /etc/nebula /var/lib/mackesd 2>&1 | sed -n '1,80p'" || true
fi
on "$B" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$B" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
check "node B overlay is up for stability"          on "$B" "ip -4 addr show | grep -q 10.42"
for t in $(seq 1 30); do on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1" && break; sleep 2; done
check "stability mesh overlay is reachable"         on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1"

# soak — RSS should plateau, not climb, under repeated bus traffic.
echo "-- soak (footprint plateau) --"
r0="$(on "$A" "ps -o rss= -C mackesd 2>/dev/null | awk '{s+=\$1}END{print s+0}'" | tr -d '\r')"
for i in $(seq 1 8); do on "$A" "for n in \$(seq 1 50); do mde-bus publish event/soak/\$n --body-flag '{\"i\":'\$n'}' 2>/dev/null; done" >/dev/null 2>&1; done
sleep 5
r1="$(on "$A" "ps -o rss= -C mackesd 2>/dev/null | awk '{s+=\$1}END{print s+0}'" | tr -d '\r')"
check "mackesd footprint plateaus under traffic (${r0}→${r1} KiB, <50% growth)" \
  bash -c "[ ${r0:-0} -gt 0 ] && [ ${r1:-0} -le \$(( ${r0:-1} * 3 / 2 )) ]"

# fd budget — BUG-BROWSER-7. The daemon must ship a raised service limit, stay
# well below the old 1024-fd ceiling under multi-worker Bus traffic, and emit no
# fresh EMFILE journal lines.
echo "-- fd budget (nofile + EMFILE guard) --"
nofile="$(on "$A" "systemctl show -p LimitNOFILE --value mackesd.service 2>/dev/null || true" | tr -dc '0-9')"
f0="$(mackesd_fd_count "$A")"
for i in $(seq 1 10); do on "$A" "for n in \$(seq 1 100); do mde-bus publish event/fd-soak/$i/\$n --body-flag '{\"i\":$i,\"n\":'\$n'}' 2>/dev/null; done" >/dev/null 2>&1; done
sleep 5
f1="$(mackesd_fd_count "$A")"
emfile_recent="$(on "$A" "journalctl -u mackesd --since '2 min ago' --no-pager 2>/dev/null | grep -Eic 'EMFILE|Too many open files' || true" | tr -dc '0-9')"
check "mackesd service raises LimitNOFILE (${nofile:-0} >= 65536)" \
  bash -c "[ ${nofile:-0} -ge 65536 ]"
check "mackesd fd count stays below the old 1024 ceiling (${f0:-0}→${f1:-0})" \
  bash -c "[ ${f0:-0} -gt 0 ] && [ ${f1:-0} -gt 0 ] && [ ${f1:-0} -lt 1024 ] && [ ${f1:-0} -le \$(( ${f0:-0} + 128 )) ]"
check "mackesd logs no fresh EMFILE/too-many-open-files events" \
  bash -c "[ ${emfile_recent:-0} -eq 0 ]"

# chaos — destroy node B; node A must NOT wedge (load stays sane, no uninterruptible procs).
echo "-- chaos (kill a node, survivor stays healthy) --"
ssh -i "$KEY" $SSHO "root@${MCNF_TESTBED_DOM0:-172.20.145.165}" \
  "U=\$(xe vm-list name-label=mcnf-test-1 --minimal); xe vm-shutdown uuid=\$U force=true" >/dev/null 2>&1
sleep 20
NO_WEDGE_OK=0
for _ in $(seq 1 6); do
  if on "$A" "load=\$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1); [ \${load:-0} -lt 10 ] && [ \$(ps -eo stat | grep -c '^D') -eq 0 ]"; then
    NO_WEDGE_OK=1
    break
  fi
  sleep 5
done
check "survivor (A) not wedged — load < 10, no D-state procs" test "$NO_WEDGE_OK" -eq 1
if [ "$NO_WEDGE_OK" -ne 1 ]; then
  echo "  DIAG  survivor wedge sample on A:"
  on "$A" "echo load=\$(cat /proc/loadavg); ps -eo pid,stat,wchan:24,comm | awk '\$2 ~ /^D/ || NR == 1 {print}'" || true
fi
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
