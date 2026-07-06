#!/usr/bin/env bash
# test-feature.sh — BUILD-PLATFORM-5: L2 feature acceptance on a real mini-mesh.
#
# Spins TWO clean VMs from MDE-VM-golden, installs the RPM on both, founds a mesh
# on node A, joins it from node B (the add-peer v3 path, per XPA-7), and asserts
# runtime-observable features: the overlay forms + the two nodes reach each other
# over Nebula, and the directory sees both. Hermetic (testbed up→down). Result →
# Bus (`event/test/feature/*`). Nightly / on-demand; never blocks a build.
#
# Usage:  test-feature.sh [path/to.rpm]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TESTBED="$HERE/farm-testbed.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${1:-$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)}"
[ -n "$RPM" ] && [ -f "$RPM" ] || { echo "no RPM (run xcp-build.sh rpm first)" >&2; exit 1; }

A="172.20.0.60"; B="172.20.0.61"   # farm-testbed assigns these to mcnf-test-0/1
PASS=0; FAIL=0
on()  { ssh -i "$KEY" $SSHO "mm@$1" "${@:2}"; }
feat(){ local d="$1"; shift; if "$@" >/dev/null 2>&1; then echo "  PASS  $d"; PASS=$((PASS+1)); else echo "  FAIL  $d"; FAIL=$((FAIL+1)); fi; }

cleanup() { "$TESTBED" down >/dev/null 2>&1 || true; }
trap cleanup EXIT
echo "== L2 feature acceptance — 2-node mini-mesh from $(basename "$RPM") =="
"$TESTBED" down >/dev/null 2>&1 || true
"$TESTBED" up 2 >/dev/null
for ip in "$A" "$B"; do
  for t in $(seq 1 24); do timeout 3 bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null && break; sleep 5; done
  scp -i "$KEY" $SSHO "$RPM" "mm@$ip:/tmp/mm.rpm" >/dev/null 2>&1
  on "$ip" "sudo dnf install -y /tmp/mm.rpm" >/dev/null 2>&1
done

# Onboarding sequence: found/add-peer/join are one-shot CLIs; `mackesd serve`
# (mackesd.service, ExecStart=mackesd serve) is the daemon that raises the Nebula
# overlay + the /enroll listener — both nodes must run it. mesh-id is POSITIONAL;
# add-peer takes --lighthouse (not --name) + prints the bare v3 token to stdout;
# join takes the token POSITIONALLY. A's /enroll listener must be up before B joins.
feat "found a mesh on node A (lighthouse)"        on "$A" "sudo mackesd found l2test --external-addr ${A}:4242 --role lighthouse"
ADD_PEER_OUT="$(on "$A" "sudo mackesd add-peer --lighthouse ${A} --role server 2>&1" | tr -d '\r' || true)"
TOKEN="$(printf '%s\n' "$ADD_PEER_OUT" | grep -E '^mesh:' | tail -1)"
feat "add-peer minted a join token"               test -n "$TOKEN"
if [ -z "$TOKEN" ]; then
  echo "  DIAG  add-peer output on A:"
  printf '%s\n' "$ADD_PEER_OUT" | sed -n '1,30p'
  echo "  DIAG  endpoint cert/key state on A:"
  on "$A" "sudo ls -l /etc/nebula/enroll-endpoint.* /etc/nebula 2>&1 | sed -n '1,40p'" || true
fi
on "$A" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$A" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
feat "node A serve raised the overlay (10.42.x)"  on "$A" "ip -4 addr show | grep -q 10.42"
feat "node B joins the mesh"                       on "$B" "sudo mackesd join '$TOKEN' --role server"
on "$B" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$B" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
feat "node B overlay is up"                        on "$B" "ip -4 addr show | grep -q 10.42"
for t in $(seq 1 30); do
  on "$B" "ping -c1 -W2 10.42.0.1 >/dev/null 2>&1" && break
  sleep 2
done
feat "node B reaches A over the overlay"           on "$B" "ping -c1 -W2 10.42.0.1 >/dev/null 2>&1"
for t in $(seq 1 30); do
  on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1" && break
  sleep 2
done
feat "node A reaches B over the overlay"           on "$A" "ping -c1 -W2 10.42.0.2 >/dev/null 2>&1"

# SUBSTRATE-V2 coordination plane: the L2 bed must exercise the same etcd-backed
# directory the live fleet uses, not the legacy absent-shared-dir path.
feat "node A bootstraps etcd coordination"         on "$A" "sudo /usr/libexec/mackesd/setup-etcd --init --listen 10.42.0.1"
ETCD_JOIN_OUT="$(on "$B" "sudo /usr/libexec/mackesd/setup-etcd --join 10.42.0.1 --listen 10.42.0.2 --anchors 10.42.0.1 2>&1" | tr -d '\r' || true)"
ETCD_JOIN_OK=0
printf '%s\n' "$ETCD_JOIN_OUT" | grep -q 'etcd endpoints:' && ETCD_JOIN_OK=1
feat "node B joins etcd coordination"              test "$ETCD_JOIN_OK" -eq 1
if [ "$ETCD_JOIN_OK" -ne 1 ]; then
  echo "  DIAG  setup-etcd join output on B:"
  printf '%s\n' "$ETCD_JOIN_OUT" | sed -n '1,80p'
  echo "  DIAG  B overlay/systemd state:"
  on "$B" "ip -4 addr show; systemctl --no-pager -l status nebula etcd mackesd 2>&1 | sed -n '1,160p'" || true
  echo "  DIAG  B can reach A client/peer ports:"
  on "$B" "for p in 2379 2380 4242; do timeout 3 bash -c \"cat </dev/null >/dev/tcp/10.42.0.1/\$p\" >/dev/null 2>&1 && echo \$p:open || echo \$p:closed; done" || true
fi
on "$A" "sudo systemctl restart mackesd" >/dev/null 2>&1
on "$B" "sudo systemctl restart mackesd" >/dev/null 2>&1
feat "node A mackesd active after etcd flip"       on "$A" "systemctl is-active --quiet mackesd"
feat "node B mackesd active after etcd flip"       on "$B" "systemctl is-active --quiet mackesd"
for t in $(seq 1 20); do
  on "$A" "ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 endpoint health --cluster 2>/dev/null | grep -q healthy" && break
  sleep 3
done
feat "etcd quorum is healthy"                      on "$A" "ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 endpoint health --cluster 2>/dev/null | grep -q healthy"

# Keep asserting reachability after the etcd flip; restarting mackesd must not
# break the already-formed Nebula tunnel.
feat "A reaches B over the overlay after etcd"     on "$A" "for i in \$(seq 1 12); do ping -c1 -W2 10.42.0.2 >/dev/null 2>&1 && exit 0; sleep 2; done; exit 1"
# Directory federation rides the shared substrate (SUBSTRATE-3 = peers on etcd).
for t in $(seq 1 20); do
  on "$A" "ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 get --prefix /mesh/peers/ --print-value-only 2>/dev/null | grep -cE '10\\.42' | grep -qE '^[2-9][0-9]*$'" && break
  sleep 3
done
feat "etcd peer directory has both nodes"          on "$A" "ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 get --prefix /mesh/peers/ --print-value-only 2>/dev/null | grep -cE '10\\.42' | grep -qE '^[2-9][0-9]*$'"
if ! on "$A" "test \$(mackesd peers 2>/dev/null | grep -cE '10\\.42') -ge 2" >/dev/null 2>&1; then
  echo "  DIAG  mackesd peers output on A:"
  on "$A" "mackesd peers 2>&1 | sed -n '1,30p'" || true
  echo "  DIAG  mackesd peers --json output on A:"
  on "$A" "mackesd peers --json 2>&1 | sed -n '1,20p'" || true
  echo "  DIAG  mackesd peers did not return both overlay rows; etcd keys on A:"
  on "$A" "ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 get --prefix /mesh/peers/ 2>/dev/null | sed -n '1,20p'" || true
  echo "  DIAG  recent heartbeat/etcd logs on A:"
  on "$A" "sudo journalctl -u mackesd --since '90 sec ago' --no-pager | grep -Ei 'heartbeat|peer-record|etcd|directory|error|warn' | tail -40" || true
fi
feat "the directory sees both nodes"               on "$A" "test \$(mackesd peers 2>/dev/null | grep -cE '10\\.42') -ge 2"

outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
echo "== L2 feature: $PASS passed, $FAIL failed → $outcome =="
command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/feature/mesh" --body-flag \
  "{\"tier\":\"L2-feature\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL}" 2>/dev/null || true
[ "$FAIL" -eq 0 ]
