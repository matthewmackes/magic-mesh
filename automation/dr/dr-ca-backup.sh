#!/usr/bin/env bash
# DATACENTER-23 — CA-only disaster-recovery backup of the Nebula CA.
#
# The full dr-backup.sh now folds the Nebula CA into the same age manifest as the
# tofu state + secrets. This is the FOCUSED counterpart: back up ONLY the Nebula
# CA (cert + key), age-encrypted to the mesh recipient, on its own cadence and to
# its own (ideally cold / out-of-band) destination — the separate-key discipline
# the platform documents. The CA is the root of the whole mesh's identity, so a
# rebirth needs it even when the etcd state is recoverable some other way.
#
# Output: ${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}/dr-ca-<UTC-timestamp>.age — an
# age-encrypted JSON {ca_crt_b64, ca_key_b64, created_utc, age_recipient}. The CA
# private key is ONLY ever written age-encrypted (the mesh identity that decrypts
# it lives out-of-band), so the artifact at rest never exposes a usable CA key.
#
# Off-fleet push: same MCNF_DR_OFFFLEET / MCNF_DR_OFFFLEET_CMD contract as
# dr-backup.sh.
#
# Env:
#   MCNF_AGE_KEY          mesh age identity, used to derive the recipient (/root/.mcnf-age-key)
#   MCNF_DR_DIR           output directory ($HOME/mcnf-dr-backups)
#   MCNF_CA_DIR           Nebula CA dir holding ca.crt + ca.key (/var/lib/mackesd/nebula-ca)
#   MCNF_DR_OFFFLEET      optional scp/rsync destination for the artifact
#   MCNF_DR_OFFFLEET_CMD  optional generic push command (artifact path appended)
set -euo pipefail

KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
DR_DIR="${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}"
CA_DIR="${MCNF_CA_DIR:-/var/lib/mackesd/nebula-ca}"

[ -f "$CA_DIR/ca.crt" ] || { echo "no CA cert at $CA_DIR/ca.crt — this node is not the CA holder" >&2; exit 3; }
[ -f "$CA_DIR/ca.key" ] || { echo "no CA key at $CA_DIR/ca.key — this node is not the CA holder" >&2; exit 3; }

TS="$(date -u +%Y%m%dT%H%M%SZ)"
RECIP="$(age-keygen -y "$KEY" 2>/dev/null)"
CA_CRT_B64="$(base64 -w0 <"$CA_DIR/ca.crt")"
CA_KEY_B64="$(base64 -w0 <"$CA_DIR/ca.key")"

MANIFEST="$(
  TS="$TS" RECIP="$RECIP" CA_CRT_B64="$CA_CRT_B64" CA_KEY_B64="$CA_KEY_B64" python3 - <<'PY'
import json, os
m = {
    "dr_ca_backup_version": 1,
    "created_utc": os.environ["TS"],
    "age_recipient": os.environ["RECIP"],
    "ca_crt_b64": os.environ["CA_CRT_B64"],
    "ca_key_b64": os.environ["CA_KEY_B64"],
}
print(json.dumps(m), end="")
PY
)"

mkdir -p "$DR_DIR"
OUT="$DR_DIR/dr-ca-$TS.age"
printf %s "$MANIFEST" | age -r "$RECIP" >"$OUT"
chmod 600 "$OUT"

# Optional off-fleet push (same contract as dr-backup.sh).
if [ -n "${MCNF_DR_OFFFLEET_CMD:-}" ]; then
  if $MCNF_DR_OFFFLEET_CMD "$OUT"; then echo "off-fleet: pushed via MCNF_DR_OFFFLEET_CMD" >&2
  else echo "WARNING: off-fleet push (MCNF_DR_OFFFLEET_CMD) failed; local copy kept at $OUT" >&2; fi
elif [ -n "${MCNF_DR_OFFFLEET:-}" ]; then
  if command -v rsync >/dev/null 2>&1; then PUSH=(rsync -a "$OUT" "$MCNF_DR_OFFFLEET")
  else PUSH=(scp -q "$OUT" "$MCNF_DR_OFFFLEET"); fi
  if "${PUSH[@]}"; then echo "off-fleet: pushed to $MCNF_DR_OFFFLEET" >&2
  else echo "WARNING: off-fleet push to $MCNF_DR_OFFFLEET failed; local copy kept at $OUT" >&2; fi
fi

echo "$OUT"
echo "NOTE: store the mesh age key ($KEY) separately — without it this CA backup is undecryptable." >&2
