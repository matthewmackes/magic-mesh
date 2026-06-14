#!/bin/bash
# do-lighthouse-cloudinit.sh — DigitalOcean cloud-init user-data that turns a
# fresh Fedora droplet into a Magic Mesh founding lighthouse on first boot
# (Option A: doctl + cloud-init). Honors AI_GOVERNANCE §8: one founding
# lighthouse per mesh (each droplet is its own complete, isolated mesh).
#
# It is a TEMPLATE: `do-lighthouse-up.sh` substitutes the @PLACEHOLDERS@ below
# and passes the result as the droplet's --user-data. DO runs it as root once.
#
# Steps: detect the droplet's public IP from the DO metadata service →
# install magic-mesh (+ nebula) → `mackesd found` the mesh → open the
# lighthouse ports → start the daemon (which activates the /enroll listener) →
# drop the v3 join token at a well-known path the up-script fetches.
#
# All output also lands in /var/log/cloud-init-output.log for debugging.
set -euo pipefail

# ---- substituted by do-lighthouse-up.sh -----------------------------------
MESH_ID="@MESH_ID@"
ROLE="@ROLE@"                       # lighthouse|server|workstation
REPO_BASEURL="@REPO_BASEURL@"       # gh-pages dnf channel base (no trailing /)
RPM_URL="@RPM_URL@"                 # optional direct RPM URL (overrides the repo)
ENROLL_PORT="@ENROLL_PORT@"         # /enroll HTTPS port (default 4243)
# ---------------------------------------------------------------------------

TOKEN_FILE="/root/mesh-join-token.txt"
STATUS_FILE="/root/mesh-found-status.txt"
log() { echo "[magic-lighthouse] $*"; }
fail() { echo "FAILED: $*" >"$STATUS_FILE"; log "FATAL: $*"; exit 1; }

# 1. Public IP from the DO metadata service (link-local, no creds needed).
META="http://169.254.169.254/metadata/v1"
PUBLIC_IP="$(curl -fsS --max-time 10 "$META/interfaces/public/0/ipv4/address" || true)"
[ -n "$PUBLIC_IP" ] || fail "could not read the droplet public IP from DO metadata"
log "public IP: $PUBLIC_IP"

# 2. Install magic-mesh (+ the nebula control plane it Requires).
if [ -n "$RPM_URL" ] && [ "$RPM_URL" != "@RPM_URL@" ]; then
    # Direct RPM (e.g. the portable build for an older-glibc DO image).
    log "installing magic-mesh from $RPM_URL"
    dnf install -y "$RPM_URL" || fail "dnf install of $RPM_URL failed"
else
    # The gh-pages dnf channel, keyed to THIS droplet's Fedora releasever.
    RELEASEVER="$(rpm -E %fedora)"
    log "installing magic-mesh from $REPO_BASEURL (fedora-$RELEASEVER)"
    cat >/etc/yum.repos.d/magic-mesh.repo <<EOF
[magic-mesh]
name=Magic Mesh
baseurl=$REPO_BASEURL/fedora-$RELEASEVER-x86_64/
enabled=1
gpgcheck=1
gpgkey=$REPO_BASEURL/RPM-GPG-KEY-magic-mesh
EOF
    dnf install -y magic-mesh || fail "dnf install magic-mesh failed (is there a fedora-$RELEASEVER channel dir? else pass --rpm-url a portable build)"
fi
command -v mackesd >/dev/null || fail "mackesd not on PATH after install"

# 3. Found the mesh — mint the CA, self-sign, generate the /enroll endpoint
#    identity, and print the v3 join token (with the embedded cert fp).
log "founding mesh '$MESH_ID' on $PUBLIC_IP"
FOUND_OUT="$(mackesd found "$MESH_ID" --external-addr "$PUBLIC_IP" --role "$ROLE" --enroll-port "$ENROLL_PORT" 2>&1)" \
    || fail "mackesd found failed: $FOUND_OUT"
echo "$FOUND_OUT"

# Extract the `mackesd join '<token>'` line `found` printed.
JOIN_TOKEN="$(printf '%s\n' "$FOUND_OUT" | sed -n "s/.*mackesd join '\(mesh:[^']*\)'.*/\1/p" | head -1)"
[ -n "$JOIN_TOKEN" ] || fail "could not parse the join token from mackesd found output"
printf '%s\n' "$JOIN_TOKEN" >"$TOKEN_FILE"
chmod 600 "$TOKEN_FILE"
log "join token written to $TOKEN_FILE"

# 4. Open the lighthouse ports (firewalld if present; DO Cloud Firewall is the
#    real gate, applied by the up-script).
if systemctl is-active --quiet firewalld 2>/dev/null; then
    firewall-cmd --quiet --permanent --add-port=4242/udp || true   # Nebula data plane
    firewall-cmd --quiet --permanent --add-port="$ENROLL_PORT"/tcp || true  # /enroll
    firewall-cmd --quiet --permanent --add-port=443/tcp || true    # covert tunnel
    firewall-cmd --quiet --reload || true
    log "firewalld: opened 4242/udp, $ENROLL_PORT/tcp, 443/tcp"
fi

# 5. Start the services — the daemon (run_serve spawns the nebula-enroll-listener
#    so peers can `mackesd join` immediately), the overlay, and the health
#    watchdog. enable = boot-durable (ONBOARD-9 service manager).
systemctl enable --now nebula.service mackesd.service mesh-health.timer \
    || fail "could not start mesh services"
log "services up (boot-durable) — /enroll endpoint live on $PUBLIC_IP:$ENROLL_PORT"

echo "OK $PUBLIC_IP $ENROLL_PORT" >"$STATUS_FILE"
log "lighthouse ready. Add a peer with:  mackesd join '$JOIN_TOKEN'"
