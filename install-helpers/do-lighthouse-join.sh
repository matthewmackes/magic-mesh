#!/bin/bash
# do-lighthouse-join.sh — provision a DigitalOcean droplet that JOINS an existing
# MCNF mesh as a lighthouse (#13, the turn-key `mackesd lighthouse add` executor).
# Sister to do-lighthouse-up.sh, which FOUNDS a new mesh (§8); this takes a v3
# join token (from `mackesd add-peer --role lighthouse` on an existing lighthouse)
# and the new droplet `mackesd join --role lighthouse`s on first boot — becoming a
# full lighthouse of the EXISTING mesh (am_lighthouse + etcd voter + CA signer).
#
# Carries the SSH-key lockout invariant guard from do-lighthouse-up.sh VERBATIM
# (the recurring lighthouse-lockout — 3x observed).
#
# Usage:
#   do-lighthouse-join.sh <join-token> [options]
# Options (defaults in []):
#   --region <r>        DO region            [nyc3]
#   --size <s>          droplet size slug    [s-1vcpu-512mb-10gb]
#   --image <img>       DO image slug        [fedora-43-x64]
#   --ssh-key <id>      DO ssh-key id/fingerprint (repeatable; default: all)
#   --repo-baseurl <u>  dnf channel base     [https://matthewmackes.github.io/magic-mesh]
#   --rpm-url <u>       direct thin lighthouse RPM URL (overrides the channel)
#   --tag <t>           droplet+firewall tag [magic-lighthouse]
#   --keep-on-fail      don't destroy the droplet if the join fails
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TEMPLATE="$HERE/do-lighthouse-join-cloudinit.sh"
[ -f "$TEMPLATE" ] || { echo "missing $TEMPLATE" >&2; exit 1; }

# ---- defaults -------------------------------------------------------------
REGION="nyc3"; SIZE="s-1vcpu-512mb-10gb"; IMAGE="fedora-43-x64"
REPO_BASEURL="https://matthewmackes.github.io/magic-mesh"; RPM_URL=""
TAG="magic-lighthouse"; SSH_KEYS=(); KEEP_ON_FAIL=0

[ $# -ge 1 ] || { sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
JOIN_TOKEN="$1"; shift
while [ $# -gt 0 ]; do
    case "$1" in
        --region) REGION="$2"; shift 2;;
        --size) SIZE="$2"; shift 2;;
        --image) IMAGE="$2"; shift 2;;
        --ssh-key) SSH_KEYS+=("$2"); shift 2;;
        --repo-baseurl) REPO_BASEURL="$2"; shift 2;;
        --rpm-url) RPM_URL="$2"; shift 2;;
        --tag) TAG="$2"; shift 2;;
        --keep-on-fail) KEEP_ON_FAIL=1; shift;;
        *) echo "unknown option: $1" >&2; exit 1;;
    esac
done

command -v doctl >/dev/null || { echo "doctl not found — install + 'doctl auth init'" >&2; exit 1; }

DROPLET="lh-join-$(date +%s 2>/dev/null || echo run)"
log() { echo "==> $*"; }

# 1. Render the cloud-init user-data from the template.
USERDATA="$(mktemp)"; trap 'rm -f "$USERDATA"' EXIT
sed -e "s|@JOIN_TOKEN@|$JOIN_TOKEN|g" \
    -e "s|@REPO_BASEURL@|$REPO_BASEURL|g" \
    -e "s|@RPM_URL@|${RPM_URL:-@RPM_URL@}|g" \
    "$TEMPLATE" >"$USERDATA"

# 2. Ensure the DO Cloud Firewall for the lighthouse ports (idempotent, by tag).
FW_NAME="magic-mesh-$TAG"
if ! doctl compute firewall list --format Name --no-header 2>/dev/null | grep -qx "$FW_NAME"; then
    log "creating DO Cloud Firewall '$FW_NAME' (22/tcp, 443/tcp, 4242/udp)"
    doctl compute firewall create --name "$FW_NAME" --tag-names "$TAG" \
        --inbound-rules "protocol:tcp,ports:22,address:0.0.0.0/0,address:::/0 protocol:tcp,ports:443,address:0.0.0.0/0,address:::/0 protocol:udp,ports:4242,address:0.0.0.0/0,address:::/0" \
        --outbound-rules "protocol:tcp,ports:all,address:0.0.0.0/0,address:::/0 protocol:udp,ports:all,address:0.0.0.0/0,address:::/0 protocol:icmp,address:0.0.0.0/0,address:::/0" \
        >/dev/null
else
    log "reusing DO Cloud Firewall '$FW_NAME'"
fi

# 3. Resolve SSH keys (default: every key registered with the account).
if [ ${#SSH_KEYS[@]} -eq 0 ]; then
    mapfile -t SSH_KEYS < <(doctl compute ssh-key list --format ID --no-header)
    [ ${#SSH_KEYS[@]} -gt 0 ] || { echo "no DO ssh-keys found — register one or pass --ssh-key" >&2; exit 1; }
fi
SSH_KEY_ARG="$(IFS=,; echo "${SSH_KEYS[*]}")"

# 3b. INVARIANT (verbatim from do-lighthouse-up.sh): at least one selected DO key
#     must have its private half on THIS box, else the droplet is born locked-out
#     (the recurring lighthouse-lockout, 3x observed). ssh-keygen -y excludes
#     pubkey-only files (known_hosts / authorized_keys / *.pub).
local_fps="$(for k in "$HOME"/.ssh/*; do
        case "$k" in *.pub|*known_hosts*|*authorized_keys|*config) continue;; esac
        [ -f "$k" ] || continue
        ssh-keygen -y -P "" -f "$k" >/dev/null 2>&1 || continue   # private key we can read
        ssh-keygen -lf "$k" -E md5 2>/dev/null | awk '{print $2}' | sed 's/^MD5://'
    done | sort -u)"
sel_fps="$(doctl compute ssh-key list --format ID,FingerPrint --no-header 2>/dev/null \
    | awk -v ids="$SSH_KEY_ARG" 'BEGIN{n=split(ids,a,","); for(i=1;i<=n;i++) want[a[i]]=1} want[$1]{print $2}')"
if ! comm -12 <(printf '%s\n' "$local_fps") <(printf '%s\n' "$sel_fps" | sort -u) | grep -q .; then
    echo "!! none of the selected DO ssh-keys has a private half on this host." >&2
    echo "   the new lighthouse would be UNREACHABLE (the recurring lockout)." >&2
    echo "   register this host's key first, then re-run:" >&2
    echo "     doctl compute ssh-key import mcnf-buildhost-ops --public-key-file ~/.ssh/id_ed25519.pub" >&2
    echo "   (or pass --ssh-key <id> for a DO key whose private half is local)" >&2
    exit 1
fi

# 4. Create the droplet with the cloud-init + tag, and wait for it active.
log "creating droplet '$DROPLET' ($SIZE, $IMAGE, $REGION) — joins the EXISTING mesh"
IP="$(doctl compute droplet create "$DROPLET" \
    --region "$REGION" --size "$SIZE" --image "$IMAGE" \
    --ssh-keys "$SSH_KEY_ARG" --user-data-file "$USERDATA" \
    --tag-name "$TAG" --wait \
    --format PublicIPv4 --no-header)"
[ -n "$IP" ] || { echo "droplet create returned no IP" >&2; exit 1; }
log "droplet up at $IP — waiting for it to join the mesh…"

# 5. Poll the join status the cloud-init writes.
SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o BatchMode=yes)
STATUS=""
for _ in $(seq 1 60); do            # up to ~5 min (install + join can be slow)
    STATUS="$(ssh "${SSH_OPTS[@]}" "root@$IP" 'cat /root/mesh-join-status.txt 2>/dev/null' 2>/dev/null || true)"
    case "$STATUS" in
        OK*) break;;
        FAILED:*) break;;
    esac
    sleep 5
done

if [ "${STATUS%% *}" != "OK" ]; then
    echo "!! lighthouse join did not complete (status: ${STATUS:-<none>})" >&2
    echo "   inspect: ssh root@$IP 'tail -50 /var/log/cloud-init-output.log'" >&2
    [ "$KEEP_ON_FAIL" -eq 1 ] || { echo "   destroying droplet (use --keep-on-fail to keep it)" >&2; doctl compute droplet delete "$DROPLET" --force >/dev/null 2>&1 || true; }
    exit 1
fi

cat <<EOF

✅ Lighthouse joined the mesh
   droplet : $DROPLET
   ip      : $IP

The supervisor roster reconcile propagates it to every peer; verify with:
  mackesd peers
EOF
