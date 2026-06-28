#!/usr/bin/env bash
# dr-ca-bundle.sh — DAR-42: OPERATOR-RUN, SEPARATE, passphrase-sealed off-fleet
# bundle of the Nebula CA + the mesh age identity — the master keys the dr-<ts>.age
# manifest can NOT carry (a key cannot live inside the thing it decrypts). Sealed
# under the Argon2id+XChaCha20 envelope via `mackesd secret-seal` (DAR-2), so the
# bundle is unreadable without the operator's passphrase, and pushed to a DISTINCT
# off-fleet prefix s3://<bucket>/keys/ (never co-located with /age/).
#
# This is the ONE place passphrase-sealing is used for DR (per the design): the
# control-VM bootstrap uses on-VM keygen, NOT this. Real push is operator-run; the
# agent is classifier-blocked from egress past the trust boundary.
#
# Passphrase: read from MDE_BACKUP_PASSPHRASE (export before running) or a
# --passphrase-file, never a literal / never logged.
#
# Usage:
#   dr-ca-bundle.sh                      # dry-run (default): build+seal locally, show the keys/ target, push NOTHING
#   dr-ca-bundle.sh --push               # OPERATOR: seal + push to s3://<bucket>/keys/
#   dr-ca-bundle.sh --passphrase-file <f>
# Env (via dr-env.sh): MCNF_AGE_KEY, MCNF_DR_BUCKET, MCNF_DR_DIR.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
SECRET="$SRC_REPO/automation/secrets/mcnf-secret.sh"
# shellcheck source=./dr-env.sh
. "$HERE/dr-env.sh"

MACKESD="${MCNF_MACKESD:-mackesd}"
CA_KEY="${MCNF_NEBULA_CA_KEY:-/var/lib/mackesd/nebula-ca/ca.key}"
AGE_KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
PASSFILE=""
PUSH=0

while [ $# -gt 0 ]; do
  case "$1" in
    --push)            PUSH=1; shift ;;
    --dry-run)         PUSH=0; shift ;;
    --passphrase-file) PASSFILE="$2"; shift 2 ;;
    -h|--help)         sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "dr-ca-bundle: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

BUCKET="$MCNF_DR_BUCKET"
REMOTE_PATH="s3://${BUCKET}/keys/"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_DIR="$MCNF_DR_DIR"
mkdir -p "$OUT_DIR"
SEALED="$OUT_DIR/ca-identity-bundle-$TS.sealed"

# Build the plaintext bundle (a tar of the CA export + the age identity) entirely
# in a tmpfs work dir, seal it, then shred the plaintext. The CA itself is exported
# via `mackesd ca export` (its OWN passphrase-armored form); we additionally seal
# the whole bundle so the age identity travels under the SAME passphrase envelope.
WORK="$(mktemp -d)"
chmod 700 "$WORK"
trap 'rm -rf "$WORK"' EXIT

# Resolve the passphrase source for mackesd secret-seal (--passphrase-file). If the
# operator exported MDE_BACKUP_PASSPHRASE, materialize it to a tmpfs file (0600);
# else require --passphrase-file. NEVER echo it.
if [ -z "$PASSFILE" ]; then
  if [ -n "${MDE_BACKUP_PASSPHRASE:-}" ]; then
    PASSFILE="$WORK/pass"; (umask 077; printf '%s' "$MDE_BACKUP_PASSPHRASE" >"$PASSFILE")
  else
    echo "dr-ca-bundle: no passphrase — export MDE_BACKUP_PASSPHRASE or pass --passphrase-file <f>" >&2
    exit 2
  fi
fi

# Assemble the plaintext bundle: the age identity + (when present) the CA key/export.
BUNDLE_DIR="$WORK/keys"; mkdir -p "$BUNDLE_DIR"
have_ca=0
if [ -f "$AGE_KEY" ]; then cp "$AGE_KEY" "$BUNDLE_DIR/mcnf-age-key"; else
  echo "dr-ca-bundle: WARN — age identity $AGE_KEY absent (bundle will lack the manifest-decrypt key)" >&2
fi
if [ -f "$CA_KEY" ]; then cp "$CA_KEY" "$BUNDLE_DIR/nebula-ca.key"; have_ca=1; fi
# Also fold in `mackesd ca export` (CA + peer certs, its own armored form) when the
# binary + CA are present — so a restore can re-adopt the full PKI, not just the key.
if command -v "$MACKESD" >/dev/null 2>&1 && [ "$have_ca" = 1 ]; then
  if printf '%s' "$(cat "$PASSFILE")" | "$MACKESD" ca export --passphrase-stdin --output "$BUNDLE_DIR/ca-export.armored" 2>/dev/null; then
    :
  else
    echo "dr-ca-bundle: WARN — mackesd ca export failed/skipped (CA key still bundled)" >&2
  fi
fi

TARBALL="$WORK/bundle.tar"
tar -C "$WORK" -cf "$TARBALL" keys

# Seal the whole tarball under the passphrase envelope (DAR-2: mackesd secret-seal).
if ! command -v "$MACKESD" >/dev/null 2>&1; then
  echo "dr-ca-bundle: mackesd not on PATH — cannot seal (set MCNF_MACKESD)" >&2
  exit 1
fi
"$MACKESD" secret-seal --passphrase-file "$PASSFILE" <"$TARBALL" >"$SEALED"
chmod 600 "$SEALED"
# Shred the plaintext immediately.
rm -f "$TARBALL" "$BUNDLE_DIR"/* 2>/dev/null || true

echo "==> sealed CA+identity bundle → $SEALED (passphrase-armored; unreadable without the passphrase)"

if [ "$PUSH" -eq 0 ]; then
  cat <<EOF
dr-ca-bundle --dry-run (sealed locally; DO not contacted):
  sealed:  $SEALED
  remote:  ${REMOTE_PATH}$(basename "$SEALED")   (DISTINCT keys/ prefix — never co-located with /age/)
  push:    OPERATOR-ONLY (the agent is classifier-blocked from egress)

To push (OPERATOR): automation/dr/dr-ca-bundle.sh --push
EOF
  exit 0
fi

# --- real push (operator) ---
command -v rclone >/dev/null 2>&1 || { echo "dr-ca-bundle: rclone not installed" >&2; exit 1; }
KEYPAIR="$(bash "$SECRET" get dr-spaces-key 2>/dev/null || true)"
[ -n "$KEYPAIR" ] || { echo "dr-ca-bundle: /mcnf/secret/dr-spaces-key absent — operator must seal it first" >&2; exit 1; }
export RCLONE_CONFIG_MCNFSPACES_TYPE=s3
export RCLONE_CONFIG_MCNFSPACES_PROVIDER=DigitalOcean
export RCLONE_CONFIG_MCNFSPACES_ACCESS_KEY_ID="${KEYPAIR%%:*}"
export RCLONE_CONFIG_MCNFSPACES_SECRET_ACCESS_KEY="${KEYPAIR#*:}"
export RCLONE_CONFIG_MCNFSPACES_ENDPOINT="${MCNF_DR_SPACES_ENDPOINT:-nyc3.digitaloceanspaces.com}"
unset KEYPAIR
echo "==> pushing $(basename "$SEALED") → mcnfspaces:${BUCKET}/keys/"
rclone copyto "$SEALED" "mcnfspaces:${BUCKET}/keys/$(basename "$SEALED")" \
  && echo "dr-ca-bundle: pushed OK (keys/ prefix, sealed)" \
  || { echo "dr-ca-bundle: rclone push FAILED" >&2; exit 1; }
