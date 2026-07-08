#!/usr/bin/env bash
# test-lighthouse-replace.sh — staged lighthouse destroy/replace drill.
#
# Builds a 4-node snapshot-reset mesh from the candidate RPM:
#   A = founding lighthouse + CA holder
#   B = server
#   C = additional lighthouse, then destroyed
#   D = replacement lighthouse
#
# The drill proves the INCIDENT-WEDGE lesson in a non-production stage: losing a
# lighthouse must not wedge surviving nodes, and a replacement lighthouse can join
# the same etcd-backed coordination plane. It deliberately does not destroy the
# founding CA holder; live CA-holder replacement remains an operator release drill.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TESTBED="$HERE/farm-testbed.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
DOM0="${MCNF_TESTBED_DOM0:-172.20.0.9}"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${1:-$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)}"
[ -n "$RPM" ] && [ -f "$RPM" ] || { echo "no RPM (run xcp-build.sh rpm first)" >&2; exit 1; }

A="172.20.0.60"
B="172.20.0.61"
C="172.20.0.62"
D="172.20.0.63"
PASS=0
FAIL=0

on() { ssh -i "$KEY" $SSHO "mm@$1" "${@:2}"; }
root_dom0() { ssh -i "$KEY" $SSHO "root@$DOM0" "$@"; }
check() {
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "  PASS  $d"; PASS=$((PASS+1))
  else
    echo "  FAIL  $d"; FAIL=$((FAIL+1))
  fi
}
cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT

wait_ssh() {
  local ip="$1"
  for _ in $(seq 1 36); do
    timeout 3 bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null && return 0
    sleep 5
  done
  return 1
}

install_rpm() {
  local ip="$1"
  wait_ssh "$ip"
  scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1 || return 1
  if timeout 600 ssh -i "$KEY" $SSHO "mm@$ip" "sudo dnf install -y /tmp/mm.rpm" >/dev/null 2>&1; then
    return 0
  fi
  echo "  DIAG  RPM install failed or timed out on $ip:"
  on "$ip" "rpm -q magic-mesh || true; ps -eo pid,ppid,stat,etime,cmd | grep -E 'dnf|rpm' | grep -v grep || true; sudo tail -80 /var/log/dnf.log /var/log/dnf.rpm.log 2>/dev/null || true" || true
  return 1
}

join_node() {
  local ip="$1" role="$2" token="$3"
  local out ok=0
  out="$(on "$ip" "sudo mackesd join '$token' --role '$role' 2>&1" | tr -d '\r' || true)"
  printf '%s\n' "$out" | grep -q 'joined `' && ok=1
  if [ "$ok" -ne 1 ]; then
    echo "  DIAG  join output for $ip/$role:"
    printf '%s\n' "$out" | sed -n '1,80p'
    return 1
  fi
}

mint_token() {
  local role="$1" out token
  out="$(on "$A" "sudo mackesd add-peer --lighthouse ${A} --role '$role' 2>&1" | tr -d '\r' || true)"
  token="$(printf '%s\n' "$out" | grep -E '^mesh:' | tail -1)"
  if [ -z "$token" ]; then
    echo "  DIAG  add-peer output for $role:"
    printf '%s\n' "$out" | sed -n '1,80p'
    return 1
  fi
  printf '%s\n' "$token"
}

wait_overlay() {
  local ip="$1"
  for _ in $(seq 1 20); do
    on "$ip" "ip -4 addr show | grep -q 10.42" && return 0
    sleep 3
  done
  return 1
}

setup_etcd_init() {
  on "$A" "sudo /usr/libexec/mackesd/setup-etcd --init --listen 10.42.0.1"
}

setup_etcd_join() {
  local ip="$1" overlay="$2" anchors="$3"
  local out ok=0
  out="$(on "$ip" "sudo /usr/libexec/mackesd/setup-etcd --join 10.42.0.1 --listen '$overlay' --anchors '$anchors' 2>&1" | tr -d '\r' || true)"
  for _ in $(seq 1 12); do
    on "$ip" "ETCDCTL_API=3 etcdctl --endpoints=http://$overlay:2379 endpoint health 2>/dev/null | grep -q healthy" && ok=1 && break
    sleep 5
  done
  if [ "$ok" -ne 1 ]; then
    echo "  DIAG  setup-etcd join output for $ip/$overlay:"
    printf '%s\n' "$out" | sed -n '1,100p'
    echo "  DIAG  etcd state on $ip:"
    on "$ip" "systemctl --no-pager -l status etcd 2>&1 | sed -n '1,120p'; sudo cat /etc/etcd/etcd.env 2>&1; ls -l /etc/mackesd/etcd-endpoints 2>&1; cat /etc/mackesd/etcd-endpoints 2>&1" || true
    return 1
  fi
}

restart_mackesd() {
  local ip="$1"
  on "$ip" "sudo systemctl restart mackesd"
}

healthy_endpoints() {
  local ip="$1" eps="$2" want="$3"
  on "$ip" "test \$(ETCDCTL_API=3 etcdctl --endpoints='$eps' endpoint health 2>/dev/null | grep -c healthy) -ge '$want'"
}

no_wedge() {
  local ip="$1"
  for _ in $(seq 1 6); do
    if on "$ip" "load=\$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1);
              [ \${load:-0} -lt 10 ] &&
              [ \$(ps -eo stat | grep -c '^D') -eq 0 ] &&
              test -z \"\$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null)\""; then
      return 0
    fi
    sleep 5
  done
  echo "  DIAG  survivor wedge sample on $ip:"
  on "$ip" "echo load=\$(cat /proc/loadavg); findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null || true; ps -eo pid,stat,wchan:24,comm | awk '\$2 ~ /^D/ || NR == 1 {print}'" || true
  return 1
}

destroy_vm() {
  local name="$1"
  root_dom0 "U=\$(xe vm-list name-label='$name' --minimal);
             [ -n \"\$U\" ] &&
             xe vm-shutdown uuid=\$U force=true >/dev/null 2>&1 || true"
}

retire_legacy_substrate() {
  local ip="$1"
  on "$ip" "sudo bash -s" <<'NODE'
set +e
for c in /sys/fs/fuse/connections/*/abort; do [ -e "$c" ] && echo 1 > "$c" 2>/dev/null; done
mount | awk '/mfs#/{print $3}' | sort -r | while read -r m; do fusermount -uz "$m" 2>/dev/null; umount -l "$m" 2>/dev/null; done
ln -sf /dev/null /etc/systemd/system/qnm-shared.service
systemctl disable --now qnm-shared.service lizardfs-master.service lizardfs-chunkserver.service 2>/dev/null
pkill -9 mfschunkserver mfsmaster mfsmount 2>/dev/null
for b in mfschunkserver mfsmaster mfssetgoal; do
  p="$(command -v "$b" 2>/dev/null)"
  [ -n "$p" ] && [ ! -e "$p.cutover-disabled" ] && mv -f "$p" "$p.cutover-disabled" 2>/dev/null
done
rm -f /var/lib/mfs/.mfschunkserver.lock 2>/dev/null
systemctl daemon-reload 2>/dev/null
systemctl restart mackesd.service 2>/dev/null || true
test -z "$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null)"
NODE
}

remove_etcd_member_by_peer_ip() {
  local peer_ip="$1"
  on "$A" "member=\$(ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 member list 2>/dev/null | awk -F, '/http:\\/\\/$peer_ip:2380/{print \$1; exit}');
           [ -n \"\$member\" ] &&
           ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 member remove \"\$member\" >/dev/null"
}

echo "== Lighthouse replace drill — staged 4-node mesh from $(basename "$RPM") =="
"$TESTBED" down >/dev/null 2>&1 || true
"$TESTBED" up 4 >/dev/null

for ip in "$A" "$B" "$C" "$D"; do
  check "install candidate RPM on $ip" install_rpm "$ip"
done

check "found mesh on A (founding lighthouse)" \
  on "$A" "sudo mackesd found l4replace --external-addr ${A}:4242 --role lighthouse"
on "$A" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
check "A overlay is up" wait_overlay "$A"

SERVER_TOKEN="$(mint_token server || true)"
check "mint server join token" test -n "$SERVER_TOKEN"
check "B joins as server" join_node "$B" server "$SERVER_TOKEN"
on "$B" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
check "B overlay is up" wait_overlay "$B"

LH_TOKEN="$(mint_token lighthouse || true)"
check "mint lighthouse join token" test -n "$LH_TOKEN"
check "C joins as lighthouse" join_node "$C" lighthouse "$LH_TOKEN"
on "$C" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
check "C overlay is up" wait_overlay "$C"

for _ in $(seq 1 30); do
  on "$B" "ping -c1 -W2 10.42.0.1 >/dev/null 2>&1" && \
  on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1" && \
  on "$A" "ping -c1 -W2 10.42.0.3 >/dev/null 2>&1" && break
  sleep 2
done
check "A/B/C overlay reachability before lighthouse loss" \
  on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1 && ping -c1 -W2 10.42.0.3 >/dev/null 2>&1"

check "A bootstraps etcd" setup_etcd_init
check "B joins etcd as member" setup_etcd_join "$B" "10.42.0.2" "10.42.0.1"
check "C joins etcd as lighthouse member" setup_etcd_join "$C" "10.42.0.3" "10.42.0.1,10.42.0.2"
for ip in "$A" "$B" "$C"; do restart_mackesd "$ip" >/dev/null 2>&1 || true; done
for _ in $(seq 1 20); do
  healthy_endpoints "$A" "http://10.42.0.1:2379,http://10.42.0.2:2379,http://10.42.0.3:2379" 3 && break
  sleep 3
done
check "three-member etcd cluster healthy before loss" \
  healthy_endpoints "$A" "http://10.42.0.1:2379,http://10.42.0.2:2379,http://10.42.0.3:2379" 3
check "A retires legacy QNM/LizardFS before chaos" retire_legacy_substrate "$A"
check "B retires legacy QNM/LizardFS before chaos" retire_legacy_substrate "$B"
check "C retires legacy QNM/LizardFS before chaos" retire_legacy_substrate "$C"

echo "-- destroy additional lighthouse C --"
destroy_vm mcnf-test-2
sleep 25
check "survivor A not wedged after lighthouse C loss" no_wedge "$A"
check "survivor B not wedged after lighthouse C loss" no_wedge "$B"
check "remaining etcd members A/B healthy after C loss" \
  healthy_endpoints "$A" "http://10.42.0.1:2379,http://10.42.0.2:2379" 2
check "B still reaches founding lighthouse A after C loss" \
  on "$B" "ping -c1 -W2 10.42.0.1 >/dev/null 2>&1"
check "dead lighthouse C removed from etcd membership" remove_etcd_member_by_peer_ip "10.42.0.3"

echo "-- enroll replacement lighthouse D --"
REPL_TOKEN="$(mint_token lighthouse || true)"
check "mint replacement lighthouse join token" test -n "$REPL_TOKEN"
check "D joins as replacement lighthouse" join_node "$D" lighthouse "$REPL_TOKEN"
on "$D" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
check "D overlay is up" wait_overlay "$D"
for _ in $(seq 1 30); do on "$A" "ping -c1 -W2 10.42.0.4 >/dev/null 2>&1" && break; sleep 2; done
check "replacement lighthouse D reachable over overlay" \
  on "$A" "ping -c1 -W2 10.42.0.4 >/dev/null 2>&1"
check "D joins etcd as replacement member" setup_etcd_join "$D" "10.42.0.4" "10.42.0.1,10.42.0.2"
restart_mackesd "$D" >/dev/null 2>&1 || true
check "D retires legacy QNM/LizardFS after replacement join" retire_legacy_substrate "$D"
for _ in $(seq 1 20); do
  healthy_endpoints "$A" "http://10.42.0.1:2379,http://10.42.0.2:2379,http://10.42.0.4:2379" 3 && break
  sleep 3
done
check "replacement etcd endpoints A/B/D healthy" \
  healthy_endpoints "$A" "http://10.42.0.1:2379,http://10.42.0.2:2379,http://10.42.0.4:2379" 3
check "A/B/D have no FUSE/LizardFS mounts" \
  bash -c "for ip in $A $B $D; do ssh -i '$KEY' $SSHO mm@\$ip 'test -z \"\$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null)\"' || exit 1; done"

outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
echo "== Lighthouse replace drill: $PASS passed, $FAIL failed -> $outcome =="
command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/lighthouse-replace" --body-flag \
  "{\"tier\":\"L4-lighthouse-replace\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL}" 2>/dev/null || true
[ "$FAIL" -eq 0 ]
