#!/bin/bash
# do-lighthouse-cloudinit.sh — DigitalOcean cloud-init user-data that turns a
# fresh Fedora droplet into a MCNF founding lighthouse on first boot
# (Option A: doctl + cloud-init). Honors AI_GOVERNANCE §8: one founding
# lighthouse per mesh (each droplet is its own complete, isolated mesh).
#
# It is a TEMPLATE: `do-lighthouse-up.sh` substitutes the @PLACEHOLDERS@ below
# and passes the result as the droplet's --user-data. DO runs it as root once.
#
# Steps: detect the droplet's public IP from the DO metadata service →
# install magic-mesh-lighthouse (+ nebula) → `mackesd found` the mesh → open the
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

# 2. Install the dedicated thin lighthouse package. Do not substitute the
#    full `magic-mesh` or `magic-mesh-server` package: those variants carry
#    desktop/compute and media/file-sync payloads outside the DO role.
if [ -n "$RPM_URL" ] && [ "$RPM_URL" != "@RPM_URL@" ]; then
    # Direct RPM (e.g. the portable build for an older-glibc DO image).
    log "installing thin lighthouse RPM from $RPM_URL"
    dnf install -y --setopt=install_weak_deps=False --setopt=tsflags=nodocs \
        "$RPM_URL" || fail "dnf install of thin lighthouse RPM $RPM_URL failed"
else
    # The gh-pages dnf channel, keyed to THIS droplet's Fedora releasever.
    RELEASEVER="$(rpm -E %fedora)"
    log "installing magic-mesh-lighthouse from $REPO_BASEURL (fedora-$RELEASEVER)"
    cat >/etc/yum.repos.d/magic-mesh.repo <<EOF
[magic-mesh]
name=MCNF
baseurl=$REPO_BASEURL/fedora-$RELEASEVER-x86_64/
enabled=1
gpgcheck=1
gpgkey=$REPO_BASEURL/RPM-GPG-KEY-magic-mesh
EOF
    # The dedicated variant is control-plane-only; keep weak dependencies
    # disabled as a second guard against future optional additions.
    dnf install -y --setopt=install_weak_deps=False --setopt=tsflags=nodocs \
        magic-mesh-lighthouse || fail "dnf install magic-mesh-lighthouse failed (publish the thin variant for fedora-$RELEASEVER or pass --rpm-url a thin RPM)"
fi
command -v mackesd >/dev/null || fail "mackesd not on PATH after install"
PROFILE_HELPER=/usr/libexec/mackesd/configure-small-lighthouse
if [ ! -x "$PROFILE_HELPER" ]; then
    # Older published RPMs predate the thin-profile helper. Fetch the exact
    # repository helper so a lighthouse never silently boots without its
    # cgroup/swap/optional-service guardrails.
    curl --fail --proto '=https' --tlsv1.2 --location --max-time 30 \
        'https://raw.githubusercontent.com/matthewmackes/magic-mesh/master/install-helpers/configure-small-lighthouse.sh' \
        -o "$PROFILE_HELPER" || fail "could not fetch the thin lighthouse profile helper"
    chmod 0755 "$PROFILE_HELPER"
fi

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

# The smallest DO Basic Droplet is the supported stock lighthouse target.  Apply
# its resource/optional-service profile after found has pinned the role and
# started the control plane; the helper is idempotent and restart-safe.
"$PROFILE_HELPER" small \
    || fail "could not apply the small lighthouse resource profile"

# 5b. Optional broker/Netdata/shell setup is intentionally NOT started here:
#     configure-small-lighthouse applied the control-plane-only profile and
#     disabled these memory-heavy first-boot fetches.
log "small profile: optional broker, Netdata and shell setup remain disabled"

echo "OK $PUBLIC_IP $ENROLL_PORT" >"$STATUS_FILE"
log "lighthouse ready. Add a peer with:  mackesd join '$JOIN_TOKEN'"
