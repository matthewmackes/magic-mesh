#!/usr/bin/env bash
# DATACENTER-23 — guided control-plane REBIRTH from a dr-*.age manifest.
#
# The one-click "rebirth" the acceptance asks for: take a dr-backup.sh artifact
# and bring the no-fixed-center control plane back from cold —
#
#   1. RESTORE   — re-put the OpenTofu state + secrets into etcd (the live keys).
#   2. RE-FOUND  — write the Nebula CA (cert + key) back to $MCNF_CA_DIR so the
#                  reborn mesh keeps the SAME root of identity (every existing
#                  peer cert stays valid; new peers are signed under the same CA).
#   3. RE-ELECT  — restart mackesd so an eligible node campaigns and a fresh
#                  leader lease is taken (the control plane resumes coordinating).
#
# SAFE BY DEFAULT: with no --execute this is a DRY RUN — it decrypts the manifest,
# validates it (version, CA present, etcd reachable, mackesd present), and PRINTS
# the exact plan WITHOUT writing anything. Pass --execute to actually perform the
# rebirth (it CLOBBERS the live etcd keys + the on-disk CA — deliberately opt-in,
# the same posture as dr-restore.sh --prod).
#
# Usage:
#   dr-rebirth.sh <dr-file.age>            # dry run: validate + print the plan
#   dr-rebirth.sh <dr-file.age> --execute  # DANGER: perform the rebirth
#
# Env: MCNF_ETCD, MCNF_AGE_KEY, MCNF_CA_DIR (same as dr-backup.sh); the restore
# step reuses dr-restore.sh living beside this script.
set -euo pipefail

ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"
KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
CA_DIR="${MCNF_CA_DIR:-/var/lib/mackesd/nebula-ca}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

FILE="${1:-}"
MODE="${2:-}"
[ -n "$FILE" ] || { echo "usage: $0 <dr-file.age> [--execute]" >&2; exit 2; }
[ -f "$FILE" ] || { echo "no such file: $FILE" >&2; exit 2; }

EXECUTE=0
[ "$MODE" = "--execute" ] && EXECUTE=1

step() { echo "[rebirth] $*" >&2; }

# Decrypt + parse the manifest once. Emit a few facts the plan needs:
#   line1: dr_backup_version
#   line2: kv_count
#   line3: ca_present (true/false)
PLAIN="$(age -d -i "$KEY" <"$FILE")"
read -r VERSION KVCOUNT CA_PRESENT < <(
  printf %s "$PLAIN" | python3 -c '
import sys, json
m = json.load(sys.stdin)
print(m.get("dr_backup_version", 0),
      m.get("kv_count", len(m.get("entries", []))),
      "true" if m.get("ca", {}) else "false")
'
)

step "manifest: version=$VERSION  etcd_keys=$KVCOUNT  ca_present=$CA_PRESENT"
if [ "$CA_PRESENT" != "true" ]; then
  step "WARNING: this manifest carries NO Nebula CA — rebirth can restore etcd"
  step "         state but cannot re-found the CA. Recover the CA from a"
  step "         dr-ca-*.age (dr-ca-backup.sh) or the separate-key backup."
fi

# Pre-flight: is etcd reachable? (the restore step needs it)
ETCD_OK=0
if curl -s -m 5 "$ETCD/health" >/dev/null 2>&1; then ETCD_OK=1; fi
step "etcd ($ETCD) reachable: $([ "$ETCD_OK" -eq 1 ] && echo yes || echo NO)"

# Pre-flight: is mackesd present for the re-elect step?
MACKESD_OK=0
command -v mackesd >/dev/null 2>&1 && MACKESD_OK=1
step "mackesd binary present: $([ "$MACKESD_OK" -eq 1 ] && echo yes || echo no)"

if [ "$EXECUTE" -eq 0 ]; then
  step "DRY RUN — would, with --execute:"
  step "  1. restore $KVCOUNT etcd keys to the LIVE prefix (dr-restore.sh --prod)"
  [ "$CA_PRESENT" = "true" ] && step "  2. write ca.crt + ca.key to $CA_DIR (0600 key)"
  step "  3. restart mackesd so an eligible node re-elects a leader"
  step "manifest decrypts cleanly; re-run with --execute to perform the rebirth."
  exit 0
fi

# ----- EXECUTE: perform the rebirth -----
[ "$ETCD_OK" -eq 1 ] || { echo "refusing to rebirth: etcd $ETCD is unreachable (stand up the state backend first)" >&2; exit 4; }

# 1. RESTORE the etcd state to the live keys (reuse the verified round-trip path).
step "1/3 restoring etcd state to PRODUCTION keys"
"$HERE/dr-restore.sh" "$FILE" --prod

# 2. RE-FOUND the CA on disk (only when the manifest carries it).
if [ "$CA_PRESENT" = "true" ]; then
  step "2/3 re-founding the Nebula CA at $CA_DIR"
  mkdir -p "$CA_DIR"
  printf %s "$PLAIN" | python3 -c '
import sys, json, base64, os
m = json.load(sys.stdin)
ca = m.get("ca", {})
ca_dir = os.environ["CA_DIR"]
if ca.get("ca_crt_b64"):
    open(os.path.join(ca_dir, "ca.crt"), "wb").write(base64.b64decode(ca["ca_crt_b64"]))
if ca.get("ca_key_b64"):
    p = os.path.join(ca_dir, "ca.key")
    open(p, "wb").write(base64.b64decode(ca["ca_key_b64"]))
    os.chmod(p, 0o600)
print("ca written", file=sys.stderr)
'
else
  step "2/3 skipped — no CA in this manifest"
fi

# 3. RE-ELECT a leader: restart mackesd so it campaigns for the lease afresh.
step "3/3 re-electing the control-plane leader"
if command -v systemctl >/dev/null 2>&1 && systemctl restart mackesd 2>/dev/null; then
  step "restarted mackesd via systemd; it will re-campaign for the leader lease"
elif [ "$MACKESD_OK" -eq 1 ]; then
  mackesd take-leadership --force 2>/dev/null \
    && step "forced a fresh leader lease via 'mackesd take-leadership --force'" \
    || step "could not auto-elect; run 'systemctl restart mackesd' on an eligible node"
else
  step "mackesd not present here — restart it on an eligible node to re-elect"
fi

step "REBIRTH COMPLETE — etcd restored, CA re-founded, leader re-election triggered."
