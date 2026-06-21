#!/usr/bin/env bash
# do-mesh-lighthouses.sh — stand up a SINGLE MCNF mesh with up to 3 DigitalOcean
# lighthouses (AI_GOVERNANCE §8: LH1 = founding CA holder; LH2/LH3 join as
# additional lighthouses). This is the multi-lighthouse counterpart to
# do-lighthouse-up.sh (which founds one isolated mesh per droplet).
#
# Flow:
#   1. Found LH1 via do-lighthouse-up.sh (mints the CA + the /enroll endpoint).
#   2. For each additional lighthouse: `mackesd add-peer --role lighthouse` on
#      LH1 mints a single-use v3 token, then a droplet boots the join cloud-init
#      (`mackesd join '<token>' --role lighthouse`) into the SAME mesh.
#
# Prereqs (operator box): doctl authenticated (`doctl auth init`), a DO-registered
# SSH key whose private half is local (to reach LH1 and mint join tokens).
#
# Usage:
#   do-mesh-lighthouses.sh <mesh-id> [--count 3] [--region nyc3] \
#       [--size s-1vcpu-1gb] [--image fedora-42-x64] [--ssh-key <id>] \
#       [--repo-baseurl <u>] [--rpm-url <u>] [--enroll-port 4243] [--tag magic-lighthouse]
#
# FRONTIER: LH2/LH3 join as the lighthouse *tier* and the CA signs them, but
# distributing their public IPs to every peer as additional Nebula anchors/relays
# (+ etcd quorum) is the SUBSTRATE-V2/HA multi-anchor roster work — verify with
# `mackesd peers` on LH1 before relying on LH2/LH3 as redundant public anchors.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UP="$HERE/do-lighthouse-up.sh"
JOIN_TEMPLATE="$HERE/do-lighthouse-join-cloudinit.sh"
[ -x "$UP" ] || { echo "missing $UP" >&2; exit 1; }
[ -f "$JOIN_TEMPLATE" ] || { echo "missing $JOIN_TEMPLATE" >&2; exit 1; }

COUNT=3; REGION="nyc3"; SIZE="s-1vcpu-1gb"; IMAGE="fedora-42-x64"
REPO_BASEURL="https://matthewmackes.github.io/magic-mesh"; RPM_URL=""
ENROLL_PORT="4243"; TAG="magic-lighthouse"; SSH_KEYS=()

[ $# -ge 1 ] || { sed -n '15,22p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
MESH_ID="$1"; shift
while [ $# -gt 0 ]; do case "$1" in
  --count) COUNT="$2"; shift 2;; --region) REGION="$2"; shift 2;; --size) SIZE="$2"; shift 2;;
  --image) IMAGE="$2"; shift 2;; --ssh-key) SSH_KEYS+=("$2"); shift 2;;
  --repo-baseurl) REPO_BASEURL="$2"; shift 2;; --rpm-url) RPM_URL="$2"; shift 2;;
  --enroll-port) ENROLL_PORT="$2"; shift 2;; --tag) TAG="$2"; shift 2;;
  *) echo "unknown option: $1" >&2; exit 1;;
esac; done
command -v doctl >/dev/null || { echo "doctl not found — install + 'doctl auth init'" >&2; exit 1; }
[ "$COUNT" -ge 1 ] && [ "$COUNT" -le 3 ] || { echo "--count must be 1..3 (§8)" >&2; exit 1; }
log() { echo "==> mesh-lighthouses: $*"; }

SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o BatchMode=yes)
UP_ARGS=(--region "$REGION" --size "$SIZE" --image "$IMAGE" --repo-baseurl "$REPO_BASEURL"
         --enroll-port "$ENROLL_PORT" --tag "$TAG")
[ -n "$RPM_URL" ] && UP_ARGS+=(--rpm-url "$RPM_URL")
for k in "${SSH_KEYS[@]:-}"; do [ -n "$k" ] && UP_ARGS+=(--ssh-key "$k"); done

# ---- 1. Found LH1 ---------------------------------------------------------
log "founding LH1 for mesh '$MESH_ID'"
UP_OUT="$("$UP" "$MESH_ID" "${UP_ARGS[@]}" 2>&1)" || { echo "$UP_OUT" >&2; echo "LH1 found failed" >&2; exit 1; }
echo "$UP_OUT"
LH1_IP="$(printf '%s\n' "$UP_OUT" | sed -n 's/^[[:space:]]*ip[[:space:]]*:[[:space:]]*\([0-9.]*\).*/\1/p' | head -1)"
[ -n "$LH1_IP" ] || { echo "could not parse LH1 IP from do-lighthouse-up.sh output" >&2; exit 1; }
log "LH1 up at $LH1_IP"
LH_IPS=("$LH1_IP")

# Resolve SSH key arg for the join droplets (default: all account keys).
if [ ${#SSH_KEYS[@]} -eq 0 ]; then
  mapfile -t SSH_KEYS < <(doctl compute ssh-key list --format ID --no-header)
fi
SSH_KEY_ARG="$(IFS=,; echo "${SSH_KEYS[*]}")"

# ---- 2. Join LH2..LHn -----------------------------------------------------
for i in $(seq 2 "$COUNT"); do
  log "minting a lighthouse join token on LH1 for LH$i"
  TOKEN="$(ssh "${SSH_OPTS[@]}" "root@$LH1_IP" \
    "mackesd add-peer --role lighthouse --lighthouse $LH1_IP --enroll-port $ENROLL_PORT" 2>/dev/null | head -1)"
  case "$TOKEN" in mesh:*) :;; *) echo "add-peer did not return a token for LH$i (got: ${TOKEN:-<none>})" >&2; exit 1;; esac

  USERDATA="$(mktemp)"
  sed -e "s|@MESH_ID@|$MESH_ID|g" -e "s|@ROLE@|lighthouse|g" \
      -e "s|@REPO_BASEURL@|$REPO_BASEURL|g" -e "s|@RPM_URL@|${RPM_URL:-@RPM_URL@}|g" \
      -e "s|@ENROLL_PORT@|$ENROLL_PORT|g" -e "s|@JOIN_TOKEN@|$TOKEN|g" \
      "$JOIN_TEMPLATE" > "$USERDATA"

  DROPLET="lh-${MESH_ID}-${i}-$(date +%s)"
  log "creating LH$i droplet '$DROPLET' ($SIZE, $IMAGE, $REGION)"
  IP="$(doctl compute droplet create "$DROPLET" \
      --region "$REGION" --size "$SIZE" --image "$IMAGE" \
      --ssh-keys "$SSH_KEY_ARG" --user-data-file "$USERDATA" \
      --tag-name "$TAG" --wait --format PublicIPv4 --no-header)"
  rm -f "$USERDATA"
  [ -n "$IP" ] || { echo "LH$i droplet create returned no IP" >&2; exit 1; }
  log "LH$i droplet up at $IP — waiting for it to join…"

  STATUS=""
  for _ in $(seq 1 60); do
    STATUS="$(ssh "${SSH_OPTS[@]}" "root@$IP" 'cat /root/mesh-join-status.txt 2>/dev/null' 2>/dev/null || true)"
    case "$STATUS" in OK\ *) break;; FAILED:*) break;; esac
    sleep 5
  done
  case "$STATUS" in
    OK\ *) log "LH$i joined the mesh ($IP)"; LH_IPS+=("$IP");;
    *) echo "!! LH$i did not finish joining (status: ${STATUS:-<none>}) — inspect: ssh root@$IP 'tail -50 /var/log/cloud-init-output.log'" >&2;;
  esac
done

# ---- 3. Summary -----------------------------------------------------------
cat <<EOF

✅ Mesh '$MESH_ID' lighthouses (${#LH_IPS[@]}/$COUNT):
$(n=1; for ip in "${LH_IPS[@]}"; do echo "   LH$n : $ip$([ $n -eq 1 ] && echo '   (founding CA holder)')"; n=$((n+1)); done)

Verify the roster on LH1:
   ssh root@$LH1_IP 'mackesd peers'      # LH2/LH3 should show role=lighthouse
   ssh root@$LH1_IP 'mackesd enroll-token --mesh-id $MESH_ID'   # mint peer tokens

FRONTIER: confirm LH2/LH3 are distributed as public Nebula anchors/relays before
relying on them for redundancy (SUBSTRATE-V2/HA multi-anchor roster).
EOF
