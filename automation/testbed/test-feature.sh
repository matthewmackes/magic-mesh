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
TOKEN="$(on "$A" "sudo mackesd add-peer --lighthouse ${A} --role server 2>/dev/null" | tr -d '\r' | grep -E '^mesh:' | tail -1)"
feat "add-peer minted a join token"               test -n "$TOKEN"
on "$A" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$A" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
feat "node A serve raised the overlay (10.42.x)"  on "$A" "ip -4 addr show | grep -q 10.42"
feat "node B joins the mesh"                       on "$B" "sudo mackesd join '$TOKEN' --role server"
on "$B" "sudo systemctl enable --now mackesd" >/dev/null 2>&1
for t in $(seq 1 15); do on "$B" "ip -4 addr show | grep -q 10.42" && break; sleep 3; done
feat "node B overlay is up"                        on "$B" "ip -4 addr show | grep -q 10.42"
feat "A reaches B over the overlay"                on "$A" "ping -c2 -W2 \$(mackesd peers 2>/dev/null | grep -oE '10\\.42\\.[0-9.]+' | grep -v 10.42.0.1 | head -1)"
feat "the directory sees both nodes"               on "$A" "test \$(mackesd peers 2>/dev/null | grep -cE '10\\.42') -ge 2"

outcome="pass"; [ "$FAIL" -eq 0 ] || outcome="fail"
echo "== L2 feature: $PASS passed, $FAIL failed → $outcome =="
command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/feature/mesh" --body-flag \
  "{\"tier\":\"L2-feature\",\"outcome\":\"$outcome\",\"pass\":$PASS,\"fail\":$FAIL}" 2>/dev/null || true
[ "$FAIL" -eq 0 ]
