#!/bin/bash
# do-lighthouse-up.sh — provision an on-demand Magic Mesh lighthouse on
# DigitalOcean (Option A: doctl + cloud-init). One command stands up a fresh
# Fedora droplet that founds its own mesh (§8: one founding lighthouse per
# mesh) and exposes a ready-to-paste join token.
#
# Prereqs on the operator box:
#   * doctl, authenticated  (doctl auth init)
#   * an SSH key registered with DO  (doctl compute ssh-key list) whose private
#     half is on this box (used to fetch the token after boot)
#   * a published dnf channel for the droplet's Fedora releasever, OR a portable
#     RPM URL via --rpm-url (older DO Fedora images need this — see ONBOARD-7)
#
# Usage:
#   do-lighthouse-up.sh <mesh-id> [options]
# Options (defaults in []):
#   --region <r>        DO region            [nyc3]
#   --size <s>          droplet size slug    [s-1vcpu-1gb]
#   --image <img>       DO image slug        [fedora-42-x64]
#   --ssh-key <id>      DO ssh-key id/fingerprint (repeatable; default: all)
#   --repo-baseurl <u>  dnf channel base     [https://matthewmackes.github.io/magic-mesh]
#   --rpm-url <u>       direct RPM URL (overrides the channel)
#   --enroll-port <p>   /enroll HTTPS port   [4243]
#   --role <r>          role to pin          [lighthouse]
#   --tag <t>           droplet+firewall tag [magic-lighthouse]
#   --keep-on-fail      don't destroy the droplet if bootstrap fails
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TEMPLATE="$HERE/do-lighthouse-cloudinit.sh"
[ -f "$TEMPLATE" ] || { echo "missing $TEMPLATE" >&2; exit 1; }

# ---- defaults -------------------------------------------------------------
REGION="nyc3"; SIZE="s-1vcpu-1gb"; IMAGE="fedora-42-x64"
REPO_BASEURL="https://matthewmackes.github.io/magic-mesh"; RPM_URL=""
ENROLL_PORT="4243"; ROLE="lighthouse"; TAG="magic-lighthouse"
SSH_KEYS=(); KEEP_ON_FAIL=0

[ $# -ge 1 ] || { sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
MESH_ID="$1"; shift
while [ $# -gt 0 ]; do
    case "$1" in
        --region) REGION="$2"; shift 2;;
        --size) SIZE="$2"; shift 2;;
        --image) IMAGE="$2"; shift 2;;
        --ssh-key) SSH_KEYS+=("$2"); shift 2;;
        --repo-baseurl) REPO_BASEURL="$2"; shift 2;;
        --rpm-url) RPM_URL="$2"; shift 2;;
        --enroll-port) ENROLL_PORT="$2"; shift 2;;
        --role) ROLE="$2"; shift 2;;
        --tag) TAG="$2"; shift 2;;
        --keep-on-fail) KEEP_ON_FAIL=1; shift;;
        *) echo "unknown option: $1" >&2; exit 1;;
    esac
done

command -v doctl >/dev/null || { echo "doctl not found — install + 'doctl auth init'" >&2; exit 1; }

DROPLET="lh-${MESH_ID}-$(date +%s 2>/dev/null || echo run)"
log() { echo "==> $*"; }

# 1. Render the cloud-init user-data from the template.
USERDATA="$(mktemp)"; trap 'rm -f "$USERDATA"' EXIT
sed -e "s|@MESH_ID@|$MESH_ID|g" \
    -e "s|@ROLE@|$ROLE|g" \
    -e "s|@REPO_BASEURL@|$REPO_BASEURL|g" \
    -e "s|@RPM_URL@|${RPM_URL:-@RPM_URL@}|g" \
    -e "s|@ENROLL_PORT@|$ENROLL_PORT|g" \
    "$TEMPLATE" >"$USERDATA"

# 2. Ensure a DO Cloud Firewall (the real ingress gate) for the lighthouse
#    ports, bound to the tag. Idempotent — reuse one named magic-mesh-<tag>.
FW_NAME="magic-mesh-$TAG"
if ! doctl compute firewall list --format Name --no-header 2>/dev/null | grep -qx "$FW_NAME"; then
    log "creating DO Cloud Firewall '$FW_NAME' (22/tcp, $ENROLL_PORT/tcp, 443/tcp, 4242/udp)"
    doctl compute firewall create --name "$FW_NAME" --tag-names "$TAG" \
        --inbound-rules "protocol:tcp,ports:22,address:0.0.0.0/0,address:::/0 protocol:tcp,ports:$ENROLL_PORT,address:0.0.0.0/0,address:::/0 protocol:tcp,ports:443,address:0.0.0.0/0,address:::/0 protocol:udp,ports:4242,address:0.0.0.0/0,address:::/0" \
        --outbound-rules "protocol:tcp,ports:all,address:0.0.0.0/0,address:::/0 protocol:udp,ports:all,address:0.0.0.0/0,address:::/0 protocol:icmp,address:0.0.0.0/0,address:::/0" \
        >/dev/null
else
    log "reusing DO Cloud Firewall '$FW_NAME'"
fi

# 3. Resolve SSH keys (default: every key registered with the account, so the
#    operator can reach the box to fetch the token).
if [ ${#SSH_KEYS[@]} -eq 0 ]; then
    mapfile -t SSH_KEYS < <(doctl compute ssh-key list --format ID --no-header)
    [ ${#SSH_KEYS[@]} -gt 0 ] || { echo "no DO ssh-keys found — register one or pass --ssh-key" >&2; exit 1; }
fi
SSH_KEY_ARG="$(IFS=,; echo "${SSH_KEYS[*]}")"

# 4. Create the droplet with the cloud-init + tag, and wait for it active.
log "creating droplet '$DROPLET' ($SIZE, $IMAGE, $REGION)"
IP="$(doctl compute droplet create "$DROPLET" \
    --region "$REGION" --size "$SIZE" --image "$IMAGE" \
    --ssh-keys "$SSH_KEY_ARG" --user-data-file "$USERDATA" \
    --tag-name "$TAG" --wait \
    --format PublicIPv4 --no-header)"
[ -n "$IP" ] || { echo "droplet create returned no IP" >&2; exit 1; }
log "droplet up at $IP — waiting for cloud-init to found the mesh…"

# 5. Poll for the bootstrap status, then fetch the join token over SSH.
SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o BatchMode=yes)
STATUS=""; TOKEN=""
for _ in $(seq 1 60); do            # up to ~5 min (install + found can be slow)
    STATUS="$(ssh "${SSH_OPTS[@]}" "root@$IP" 'cat /root/mesh-found-status.txt 2>/dev/null' 2>/dev/null || true)"
    case "$STATUS" in
        OK\ *)  TOKEN="$(ssh "${SSH_OPTS[@]}" "root@$IP" 'cat /root/mesh-join-token.txt 2>/dev/null' 2>/dev/null || true)"; break;;
        FAILED:*) break;;
    esac
    sleep 5
done

if [ -z "$TOKEN" ]; then
    echo "!! lighthouse bootstrap did not complete (status: ${STATUS:-<none>})" >&2
    echo "   inspect: ssh root@$IP 'tail -50 /var/log/cloud-init-output.log'" >&2
    [ "$KEEP_ON_FAIL" -eq 1 ] || { echo "   destroying droplet (use --keep-on-fail to keep it)" >&2; doctl compute droplet delete "$DROPLET" --force >/dev/null 2>&1 || true; }
    exit 1
fi

cat <<EOF

✅ Lighthouse ready for mesh '$MESH_ID'
   droplet : $DROPLET
   ip      : $IP
   /enroll : https://$IP:$ENROLL_PORT

Add a peer (run on the joining box, which needs the new build):
  mackesd join '$TOKEN'

Or guided:  mde-enroll   (paste the token above)
EOF
