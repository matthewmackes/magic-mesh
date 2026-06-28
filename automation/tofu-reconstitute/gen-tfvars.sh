#!/usr/bin/env bash
# gen-tfvars.sh — DAR-33 (DEVOPS-AUTOMATION-REBUILD §2.8): generate the four Tofu
# roots' INPUTS (per-mesh *.auto.tfvars) AND their backend config (*.backend.hcl)
# from the mesh identity, so no hand-edit of HCL points the roots at a new mesh.
#
# WHY this exists + HOW it COMPOSES (does NOT duplicate):
# Two halves of "per-mesh tofu inputs" already have owners; this script SEQUENCES
# them, it re-implements neither:
#   • the TFVARS half  → automation/lib/mcnf-config.sh render (DAR-4): renders
#     mesh_id / project / regions / domain / lighthouse-roster / golden-template
#     into each root's *.auto.tfvars from the non-secret /mcnf/backoffice/config doc.
#   • the BACKEND half → automation/state-backend/gen-backend-config.sh (DAR-8):
#     the SINGLE producer of each root's <root>.backend.hcl from infra/tofu/
#     backend.tf.tmpl. We pass it the control overlay IP and let it own the address
#     half. We do NOT re-emit backend HCL here (that would be a second source of the
#     literal the design forbids).
# This script's own job is only: resolve the control overlay IP from the identity,
# then drive both generators with consistent inputs, and (for the acceptance) PRINT
# what would be produced. NO secret is ever written into any generated file.
#
# Per-mesh control overlay IP resolution (DAR-17 chain, NEVER the dead .192):
#   explicit --control-ip / MCNF_CONTROL_IP > the /mcnf/site control-overlay-ip doc
#   > the peer directory > this node's overlay iface.
#
# Usage:
#   gen-tfvars.sh [--control-ip <overlay-ip>] [--tofu-dir <infra/tofu>]
#                 [--roots "r1 r2 ..."] [--print] [--selftest]
#     (default)  render *.auto.tfvars (mcnf-config) + write *.backend.hcl (gen-backend-config)
#     --print    additionally print every generated file's path + body to stdout
#                (the DAR-33 acceptance reads this); NO secret appears.
#     --selftest offline self-test (mocked etcd; NO live store, NO real tofu)
#
# Env: MCNF_ETCD (resolved by DAR-1b), MCNF_CONTROL_IP (the reconstitute arm uses
# .192). MCNF_GENTFVARS_SELFTEST routes etcd I/O to a mock dir.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"            # automation/
ROOT="$(cd "$REPO/.." && pwd)"            # repo root
LIB="$REPO/lib"
MCNF_CONFIG="$LIB/mcnf-config.sh"
GEN_BACKEND="$REPO/state-backend/gen-backend-config.sh"

# shellcheck source=../lib/control-host.sh
. "$LIB/control-host.sh"

CONTROL_IP="${MCNF_CONTROL_IP:-}"
TOFU_DIR="$ROOT/infra/tofu"
ROOTS="xen-xapi zone1-do edgeos control-vm"
PRINT=0
SELFTEST=0

while [ $# -gt 0 ]; do case "$1" in
  --control-ip) CONTROL_IP="$2"; shift 2;;
  --tofu-dir)   TOFU_DIR="$2"; shift 2;;
  --roots)      ROOTS="$2"; shift 2;;
  --print)      PRINT=1; shift;;
  --selftest)   SELFTEST=1; shift;;
  -h|--help)    sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "gen-tfvars: unknown arg '$1'" >&2; exit 2;;
esac; done

# ── the producer: drive both generators with one consistent control overlay IP ──
# $1 = tofu dir, $2 = control ip. Echoes the generated file paths it touched.
_gen_tfvars_and_backend() { # <tofu-dir> <control-ip>
  local tofu_dir="$1" control_ip="$2"

  # 1) TFVARS half — compose mcnf-config.sh render (DAR-4). It reads the non-secret
  #    /mcnf/backoffice/config doc and writes each root's *.auto.tfvars. We do NOT
  #    re-implement the render — we call it.
  bash "$MCNF_CONFIG" render --tofu-dir "$tofu_dir"

  # 2) BACKEND half — compose gen-backend-config.sh (DAR-8), the SINGLE producer of
  #    each <root>.backend.hcl from backend.tf.tmpl. We hand it the control overlay
  #    IP; it owns the address half. No backend HCL is emitted here.
  bash "$GEN_BACKEND" --control-ip "$control_ip" --roots "$ROOTS" --tofu-dir "$tofu_dir"
}

# ── --print: dump every generated file (acceptance reads this) ──
_print_generated() { # <tofu-dir>
  local tofu_dir="$1" f
  for f in "$tofu_dir"/*/*.auto.tfvars "$tofu_dir"/*/*.backend.hcl; do
    [ -f "$f" ] || continue
    echo "===== $f ====="
    cat "$f"
    echo
  done
}

if [ "$SELFTEST" -eq 1 ]; then
  # Offline self-test: mock etcd (via mcnf-config's MCNF_CONFIG_SELFTEST), a throwaway
  # tofu dir with the four root dirs + a backend.tf.tmpl, and NO real tofu. Asserts:
  #   1. gen-tfvars produces *.auto.tfvars (mesh_id/regions/golden) AND *.backend.hcl,
  #   2. the backend.hcl address is http://<control-overlay-ip>:8390/state/<root> (NOT .192),
  #   3. NO secret/age key in any generated file,
  #   4. a DIFFERENT mesh-id/control-ip emits DIFFERENT values,
  #   5. the composition really shells out to BOTH owners (files only those produce).
  MOCK_DIR="$(mktemp -d)"; export MOCK_DIR
  work="$(mktemp -d)"; tofu="$work/infra/tofu"
  mkdir -p "$tofu/zone1-do" "$tofu/xen-xapi" "$tofu/edgeos" "$tofu/control-vm"
  # The real backend template (single source) — copy it into the throwaway tree.
  cp "$ROOT/infra/tofu/backend.tf.tmpl" "$tofu/backend.tf.tmpl"
  fail=0
  pass() { printf '  PASS %s\n' "$1"; }
  bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
  echo "gen-tfvars selftest (mocked etcd at $MOCK_DIR — NO live store, NO real tofu)"

  # Seed the identity doc (DAR-4 gen) for mesh 4533, control overlay 10.42.0.40.
  env MCNF_CONFIG_SELFTEST=1 MOCK_DIR="$MOCK_DIR" bash "$MCNF_CONFIG" gen 4533 \
      --project mcnf-fam --regions nyc3,fra1,sfo3 \
      --lighthouses 10.42.0.4,10.42.0.5,10.42.0.6 \
      --control-ip 10.42.0.40 --domain matthewmackes.com \
      --tofu-dir "$tofu" --no-render >"$work/seed.log" 2>&1

  # Run gen-tfvars against the seeded doc with the control overlay IP.
  out="$(env MCNF_CONFIG_SELFTEST=1 MCNF_GENTFVARS_SELFTEST=1 MOCK_DIR="$MOCK_DIR" \
      bash "$0" --control-ip 10.42.0.40 --tofu-dir "$tofu" --print 2>"$work/err.log")" || {
        echo "$out"; cat "$work/err.log" >&2; bad "gen-tfvars --print exited non-zero"; }
  printf '%s\n' "$out" >"$work/print.out"

  # 1. both halves produced their files.
  [ -f "$tofu/control-vm/mcnf-mesh.auto.tfvars" ] && grep -q 'mesh_id = "4533"' "$tofu/control-vm/mcnf-mesh.auto.tfvars" \
    && pass "tfvars half (mcnf-config) rendered mesh_id" || bad "tfvars half missing/wrong"
  [ -f "$tofu/xen-xapi/xen-xapi.backend.hcl" ] \
    && pass "backend half (gen-backend-config) wrote xen-xapi.backend.hcl" || bad "backend half missing"

  # 2. backend address is the control overlay IP (NOT .192).
  if grep -q 'address        = "http://10.42.0.40:8390/state/xen-xapi"' "$tofu/xen-xapi/xen-xapi.backend.hcl"; then
    pass "backend.hcl address = control overlay IP :8390/state/<root>"
  else
    bad "backend.hcl address wrong: $(grep '^address' "$tofu/xen-xapi/xen-xapi.backend.hcl" 2>/dev/null)"
  fi
  if grep -rq '172.20.145.192' "$tofu"/*/*.backend.hcl "$tofu"/*/*.auto.tfvars 2>/dev/null; then
    bad "a generated file contains the dead .192"
  else
    pass "no generated file contains 172.20.145.192"
  fi

  # 3. NO secret/age key in any generated file.
  if grep -RqiE 'AGE-SECRET-KEY|do[-_]token|password|secret[-_]key' "$tofu"/*/*.auto.tfvars "$tofu"/*/*.backend.hcl 2>/dev/null; then
    bad "a credential-shaped value leaked into a generated file"
  else
    pass "no secret appears in any generated tfvars/backend file"
  fi
  # The --print output likewise carries no secret.
  if grep -qiE 'AGE-SECRET-KEY' "$work/print.out"; then bad "a private key leaked into --print output"; else pass "no private key in --print output"; fi

  # 4. a DIFFERENT mesh-id + control-ip emits DIFFERENT values.
  MOCK2="$(mktemp -d)"
  env MCNF_CONFIG_SELFTEST=1 MOCK_DIR="$MOCK2" bash "$MCNF_CONFIG" gen 9001 \
      --project other --regions ams3 --lighthouses 10.99.0.4 \
      --control-ip 10.99.0.40 --tofu-dir "$tofu" --no-render >>"$work/seed.log" 2>&1
  env MCNF_CONFIG_SELFTEST=1 MCNF_GENTFVARS_SELFTEST=1 MOCK_DIR="$MOCK2" \
      bash "$0" --control-ip 10.99.0.40 --tofu-dir "$tofu" >>"$work/err.log" 2>&1
  if grep -q 'mesh_id = "9001"' "$tofu/control-vm/mcnf-mesh.auto.tfvars" \
     && grep -q 'http://10.99.0.40:8390/state/xen-xapi' "$tofu/xen-xapi/xen-xapi.backend.hcl"; then
    pass "a different mesh emits different tfvars + backend address"
  else
    bad "second mesh did not change the generated files"
  fi
  rm -rf "$MOCK2"

  rm -rf "$MOCK_DIR" "$work"
  if [ "$fail" -eq 0 ]; then echo "selftest: ALL PASS"; else echo "selftest: FAILURES" >&2; fi
  exit "$fail"
fi

# ── normal run: resolve the control overlay IP, then drive both generators ──
# DAR-17 chain; the reconstitute arm passes =172.20.145.192 explicitly.
CONTROL_IP="$(MCNF_CONTROL_IP="$CONTROL_IP" mcnf_resolve_control_host)"
[ -n "$CONTROL_IP" ] || {
  echo "gen-tfvars: cannot resolve the control overlay IP — pass --control-ip <ip>, " >&2
  echo "  set MCNF_CONTROL_IP, populate /mcnf/site/control-overlay-ip (mcnf-config.sh gen), " >&2
  echo "  or run on a node with a nebula/mde-neb overlay iface." >&2
  exit 1
}

[ -x "$MCNF_CONFIG" ] || { echo "gen-tfvars: missing $MCNF_CONFIG (DAR-4)" >&2; exit 1; }
[ -x "$GEN_BACKEND" ] || { echo "gen-tfvars: missing $GEN_BACKEND (DAR-8)" >&2; exit 1; }

_gen_tfvars_and_backend "$TOFU_DIR" "$CONTROL_IP"
[ "$PRINT" -eq 1 ] && _print_generated "$TOFU_DIR"
echo "gen-tfvars: rendered tfvars + backend config for [$ROOTS] at control overlay $CONTROL_IP"
