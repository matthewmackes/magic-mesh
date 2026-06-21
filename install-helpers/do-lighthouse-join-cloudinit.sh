#!/bin/bash
# do-lighthouse-join-cloudinit.sh — DigitalOcean cloud-init user-data that joins a
# fresh Fedora droplet to an EXISTING MCNF mesh as an additional lighthouse
# (LH2/LH3). Counterpart to do-lighthouse-cloudinit.sh (which *founds* LH1). Used
# by do-mesh-lighthouses.sh to stand up a 3-lighthouse single mesh per
# AI_GOVERNANCE §8 (up to 3 lighthouses: LH1 = founding CA holder; LH2/LH3 join).
#
# It is a TEMPLATE: do-mesh-lighthouses.sh substitutes the @PLACEHOLDERS@ and
# passes the result as the droplet's --user-data. DO runs it as root once.
#
# Steps: read the droplet's public IP from DO metadata → install magic-mesh →
# `mackesd join '<token>' --role lighthouse` (the token was minted on LH1 via
# `mackesd add-peer --role lighthouse`) → open the lighthouse ports → start the
# daemon + overlay.  All output also lands in /var/log/cloud-init-output.log.
#
# FRONTIER (verify after boot): the joining node pins the lighthouse *tier* and
# the CA signs it, but distributing this node's public IP to every peer as an
# additional Nebula anchor/relay is the SUBSTRATE-V2/HA multi-anchor roster work.
# Confirm `mackesd peers` on LH1 lists this node as a lighthouse and that other
# peers can relay through it before treating it as a redundant public anchor.
set -euo pipefail

# ---- substituted by do-mesh-lighthouses.sh --------------------------------
MESH_ID="@MESH_ID@"                 # informational; the real mesh-id is in the token
ROLE="@ROLE@"                       # lighthouse
REPO_BASEURL="@REPO_BASEURL@"       # gh-pages dnf channel base (no trailing /)
RPM_URL="@RPM_URL@"                 # optional direct RPM URL (overrides the repo)
ENROLL_PORT="@ENROLL_PORT@"         # /enroll HTTPS port (default 4243)
JOIN_TOKEN="@JOIN_TOKEN@"           # the v3 token minted on LH1 (add-peer --role lighthouse)
# ---------------------------------------------------------------------------

STATUS_FILE="/root/mesh-join-status.txt"
log() { echo "[magic-lighthouse-join] $*"; }
fail() { echo "FAILED: $*" >"$STATUS_FILE"; log "FATAL: $*"; exit 1; }

# 1. Public IP from the DO metadata service (link-local, no creds needed).
META="http://169.254.169.254/metadata/v1"
PUBLIC_IP="$(curl -fsS --max-time 10 "$META/interfaces/public/0/ipv4/address" || true)"
[ -n "$PUBLIC_IP" ] || fail "could not read the droplet public IP from DO metadata"
log "public IP: $PUBLIC_IP"

# 2. Install magic-mesh (+ the nebula control plane it Requires).
if [ -n "$RPM_URL" ] && [ "$RPM_URL" != "@RPM_URL@" ]; then
    log "installing magic-mesh from $RPM_URL"
    dnf install -y "$RPM_URL" || fail "dnf install of $RPM_URL failed"
else
    RELEASEVER="$(rpm -E %fedora)"
    log "installing magic-mesh from $REPO_BASEURL (fedora-$RELEASEVER)"
    cat >/etc/yum.repos.d/magic-mesh.repo <<EOF
[magic-mesh]
name=MCNF
baseurl=$REPO_BASEURL/fedora-$RELEASEVER-x86_64/
enabled=1
gpgcheck=1
gpgkey=$REPO_BASEURL/RPM-GPG-KEY-magic-mesh
EOF
    dnf install -y magic-mesh || fail "dnf install magic-mesh failed (need a fedora-$RELEASEVER channel dir, else pass --rpm-url)"
fi
command -v mackesd >/dev/null || fail "mackesd not on PATH after install"

# 3. Join the existing mesh as a lighthouse (network-enroll over the token's
#    pinned /enroll endpoint; no QNM pre-mount needed — MESH-1 fix).
[ -n "$JOIN_TOKEN" ] && [ "$JOIN_TOKEN" != "@JOIN_TOKEN@" ] || fail "no join token substituted"
log "joining mesh as $ROLE via the minted token"
JOIN_OUT="$(mackesd join "$JOIN_TOKEN" --role "$ROLE" 2>&1)" || fail "mackesd join failed: $JOIN_OUT"
echo "$JOIN_OUT"

# 4. Open the lighthouse ports (firewalld backstop; DO Cloud Firewall is the real gate).
if systemctl is-active --quiet firewalld 2>/dev/null; then
    firewall-cmd --quiet --permanent --add-port=4242/udp || true
    firewall-cmd --quiet --permanent --add-port="$ENROLL_PORT"/tcp || true
    firewall-cmd --quiet --permanent --add-port=443/tcp || true
    firewall-cmd --quiet --reload || true
    log "firewalld: opened 4242/udp, $ENROLL_PORT/tcp, 443/tcp"
fi

# 5. Start the services (boot-durable). The overlay comes up + the daemon
#    reconciles this node as a lighthouse-tier member.
systemctl enable --now nebula.service mackesd.service mesh-health.timer \
    || fail "could not start mesh services"
log "services up (boot-durable) on $PUBLIC_IP — joined as $ROLE"

echo "OK $PUBLIC_IP $ENROLL_PORT" >"$STATUS_FILE"
log "lighthouse joined. Verify on LH1:  mackesd peers   (this node should show role=lighthouse)"
