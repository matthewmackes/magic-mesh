#!/usr/bin/env bash
# DATACENTER-23 — disaster-recovery BACKUP of the no-fixed-center substrate.
#
# The substrate has no single recoverable center: the OpenTofu IaC state and the
# mesh secrets live in the replicated etcd store. This dumps the RECOVERABLE
# subset of that store — enough to rebuild the world from bare metal — into a
# single age-encrypted manifest:
#
#   /tofu/state/*       the OpenTofu states (what infra exists + its IDs)
#   /mcnf/secret/*      the mesh secrets (ALREADY age-encrypted in etcd)
#   /mcnf/age-recipient the public recipient (so a restore can re-encrypt)
#   the Nebula CA       ca.crt + ca.key (DATACENTER-23) — the root of the mesh's
#                       identity, so a rebirth can re-found the same Nebula and
#                       re-issue every peer cert under the ORIGINAL CA.
#
# Read-only on etcd: pulls the v3 range API exactly like mcnf-secret.sh
# (base64 keys/values), assembles a JSON manifest, and age-encrypts the whole
# manifest to the mesh recipient. The on-disk artifact is therefore double-safe
# for the secrets (age-in-age) and single-age for the tofu state + CA. The CA
# private key is the most sensitive thing in the file; it is ONLY ever written
# age-encrypted to the mesh recipient (whose private identity lives out-of-band),
# so the artifact at rest never exposes a usable CA key.
#
# Off-fleet push (DATACENTER-23): when MCNF_DR_OFFFLEET is set the produced
# artifact is copied off-fleet (so the loss of the whole LAN doesn't take the DR
# backup with it). MCNF_DR_OFFFLEET is an scp/rsync destination
# (e.g. user@host:/backups/); MCNF_DR_OFFFLEET_CMD is a generic escape hatch —
# a command run with the artifact path appended as its final argument
# (e.g. "rclone copyto", "b2 upload-file my-bucket").
#
# CAVEAT (printed at the end): the mesh age IDENTITY (private key) can NOT be
# recovered from this file — the master key cannot live only inside the thing it
# decrypts. Back the age key up SEPARATELY and securely; with it, this single
# artifact now restores the tofu state, the secrets, AND the Nebula CA.
#
# Env:
#   MCNF_ETCD             etcd v3 gateway (http://172.20.145.192:2379)
#   MCNF_AGE_KEY          mesh age identity, used to derive the recipient (/root/.mcnf-age-key)
#   MCNF_DR_DIR           output directory ($HOME/mcnf-dr-backups)
#   MCNF_CA_DIR           Nebula CA dir holding ca.crt + ca.key (/var/lib/mackesd/nebula-ca)
#   MCNF_DR_OFFFLEET      optional scp/rsync destination for the artifact
#   MCNF_DR_OFFFLEET_CMD  optional generic push command (artifact path appended)
set -euo pipefail

ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"
KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
DR_DIR="${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}"
CA_DIR="${MCNF_CA_DIR:-/var/lib/mackesd/nebula-ca}"

b64() { base64 -w0; }

# Dump every key under a prefix via the v3 range API. range_end is the prefix
# with its last byte incremented (the "<prefix>0" trick mcnf-secret.sh uses for
# the trailing '/'), giving the open-ended prefix scan. Emits the raw JSON
# {"kvs":[{"key":..,"value":..},...]} with base64 keys/values left as-is.
range_prefix() { # <prefix> <range_end>
  local s e
  s=$(printf %s "$1" | b64)
  e=$(printf %s "$2" | b64)
  curl -s -X POST "$ETCD/v3/kv/range" \
    -d "{\"key\":\"$s\",\"range_end\":\"$e\"}"
}

# Single-key range (exact get), same base64 envelope.
range_key() { # <key>
  local k
  k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$k\"}"
}

TS="$(date -u +%Y%m%dT%H%M%SZ)"
RECIP="$(age-keygen -y "$KEY" 2>/dev/null)"

# Assemble the manifest from the three etcd sources. Python merges the kvs
# arrays, keeping only key+value (base64, as etcd returns them) so a restore can
# re-put them verbatim.
TOFU_JSON="$(range_prefix "/tofu/state/" "/tofu/state0")"
SECRET_JSON="$(range_prefix "/mcnf/secret/" "/mcnf/secret0")"
RECIP_JSON="$(range_key "/mcnf/age-recipient")"

# DATACENTER-23 — read the Nebula CA (cert + key) if present; base64 so the JSON
# stays text. Empty string when a file is absent (a node that isn't the CA holder
# simply contributes no CA section — the manifest records that honestly).
CA_CRT_B64=""
CA_KEY_B64=""
[ -f "$CA_DIR/ca.crt" ] && CA_CRT_B64="$(base64 -w0 <"$CA_DIR/ca.crt")"
[ -f "$CA_DIR/ca.key" ] && CA_KEY_B64="$(base64 -w0 <"$CA_DIR/ca.key")"

MANIFEST="$(
  TS="$TS" RECIP="$RECIP" CA_CRT_B64="$CA_CRT_B64" CA_KEY_B64="$CA_KEY_B64" \
    python3 - "$TOFU_JSON" "$SECRET_JSON" "$RECIP_JSON" <<'PY'
import sys, json, os

def kvs(raw):
    try:
        d = json.loads(raw)
    except Exception:
        return []
    out = []
    for kv in (d.get("kvs") or []):
        out.append({"key": kv["key"], "value": kv.get("value", "")})
    return out

entries = []
for raw in sys.argv[1:]:
    entries.extend(kvs(raw))

# The Nebula CA (cert + key, base64). Present only when the running node holds
# the CA; recorded as a distinct section so a restore writes etcd keys but a
# rebirth alone re-founds the CA on disk.
ca = {}
crt = os.environ.get("CA_CRT_B64", "")
key = os.environ.get("CA_KEY_B64", "")
if crt:
    ca["ca_crt_b64"] = crt
if key:
    ca["ca_key_b64"] = key

manifest = {
    "dr_backup_version": 2,
    "created_utc": os.environ["TS"],
    "age_recipient": os.environ["RECIP"],
    "kv_count": len(entries),
    "entries": entries,        # each {"key": b64, "value": b64} verbatim from etcd
    "ca": ca,                  # {"ca_crt_b64":..,"ca_key_b64":..} when the CA is present
    "ca_present": bool(ca),
}
json.dump(manifest, sys.stdout)
PY
)"

mkdir -p "$DR_DIR"
OUT="$DR_DIR/dr-$TS.age"
printf %s "$MANIFEST" | age -r "$RECIP" >"$OUT"
chmod 600 "$OUT"

# DATACENTER-23 — optional off-fleet push so a LAN-wide loss can't take the only
# DR copy with it. Best-effort: a push failure warns but never fails the backup
# (the local artifact still exists).
if [ -n "${MCNF_DR_OFFFLEET_CMD:-}" ]; then
  # Generic escape hatch: run the command with the artifact path appended.
  if $MCNF_DR_OFFFLEET_CMD "$OUT"; then
    echo "off-fleet: pushed via MCNF_DR_OFFFLEET_CMD" >&2
  else
    echo "WARNING: off-fleet push (MCNF_DR_OFFFLEET_CMD) failed; local copy kept at $OUT" >&2
  fi
elif [ -n "${MCNF_DR_OFFFLEET:-}" ]; then
  # scp/rsync destination (user@host:/path/). Prefer rsync, fall back to scp.
  if command -v rsync >/dev/null 2>&1; then
    PUSH=(rsync -a "$OUT" "$MCNF_DR_OFFFLEET")
  else
    PUSH=(scp -q "$OUT" "$MCNF_DR_OFFFLEET")
  fi
  if "${PUSH[@]}"; then
    echo "off-fleet: pushed to $MCNF_DR_OFFFLEET" >&2
  else
    echo "WARNING: off-fleet push to $MCNF_DR_OFFFLEET failed; local copy kept at $OUT" >&2
  fi
else
  echo "off-fleet: no MCNF_DR_OFFFLEET target configured — local copy only at $OUT" >&2
fi

echo "$OUT"
echo "NOTE: the mesh age key ($KEY) must be backed up SEPARATELY/securely — the master key cannot live only inside the thing it decrypts. With it, this artifact now restores the tofu state, the secrets, AND the Nebula CA." >&2
