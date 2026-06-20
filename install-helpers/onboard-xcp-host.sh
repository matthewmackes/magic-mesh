#!/bin/bash
# onboard-xcp-host.sh — XCP-1: onboard an XCP-ng host (dom0) into the mesh.
#
# An XCP-ng dom0 is a locked-down CentOS-7-based control domain — it CANNOT run
# the Fedora `magic-mesh` RPM (glibc/distro mismatch). But it CAN run the static
# `nebula` Go binary (it has `/dev/net/tun`), so an XCP host joins the encrypted
# overlay the same way every node does — just provisioned manually instead of via
# the Fedora `network-enroll` flow:
#
#   1. mint a node cert on the CA lighthouse (`nebula-cert sign`),
#   2. push the static `nebula` binary + ca.crt + the node cert/key + a rendered
#      `config.yml` (pointing at the lighthouses) to dom0,
#   3. install + start `nebula.service` on dom0,
#   4. verify overlay reachability (dom0 ↔ a lighthouse over 10.42.x).
#
# This gets the XCP host onto the mesh as a reachable member (Server-tier intent).
# Advertising its VMs as mesh compute (XAPI → the directory) is the follow-on
# (XCP-6, the `mackes-xcp` agent) — out of scope here; this is the JOIN method.
#
# Run from a box with SSH to BOTH the CA lighthouse and the target XCP host.
#
# Usage:
#   onboard-xcp-host.sh --host <xcp-mgmt-ip> --name <label> --overlay-ip <10.42.0.N>
#                       [--ca-host <lighthouse-ip>] [--ca-overlay <10.42.0.1>]
#                       [--lh-pub <pub-ip:4242,...>] [--xcp-user root]
#
# Defaults target the live mesh: CA = LH-01 (45.55.33.179, overlay 10.42.0.1),
# both DO lighthouses as the static_host_map.
set -euo pipefail

XCP_HOST=""; NAME=""; OVERLAY_IP=""; XCP_USER="root"
CA_HOST="45.55.33.179"; CA_OVERLAY="10.42.0.1"
LH2_PUB="159.65.183.51"; LH2_OVERLAY="10.42.0.2"
OVERLAY_CIDR_BITS="17"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
# dom0 needs a STATICALLY-linked nebula (CentOS-7 glibc 2.17) — see step 2.
NEBULA_VERSION="${NEBULA_VERSION:-v1.10.3}"

while [ $# -gt 0 ]; do case "$1" in
  --host) XCP_HOST="$2"; shift 2;;
  --name) NAME="$2"; shift 2;;
  --overlay-ip) OVERLAY_IP="$2"; shift 2;;
  --ca-host) CA_HOST="$2"; shift 2;;
  --ca-overlay) CA_OVERLAY="$2"; shift 2;;
  --xcp-user) XCP_USER="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ -z "$XCP_HOST$NAME$OVERLAY_IP" ] && { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
[ -z "$NAME" ] && { echo "--name required" >&2; exit 1; }
[ -z "$OVERLAY_IP" ] && { echo "--overlay-ip required (e.g. 10.42.0.20)" >&2; exit 1; }

log() { echo "==> onboard-xcp: $*"; }
# SSH to the CA lighthouse uses the mesh key; SSH to dom0 uses sshpass (XCP root pw)
# OR the key if present. The XCP password rides $XCP_PW (kept out of argv).
lh()  { ssh -o StrictHostKeyChecking=no -o ConnectTimeout=15 -i "$SSH_KEY" "root@$CA_HOST" "$@"; }
xcp() {
  if [ -n "${XCP_PW:-}" ]; then
    sshpass -p "$XCP_PW" ssh -o StrictHostKeyChecking=no -o ConnectTimeout=15 "$XCP_USER@$XCP_HOST" "$@"
  else
    ssh -o StrictHostKeyChecking=no -o ConnectTimeout=15 "$XCP_USER@$XCP_HOST" "$@"
  fi
}
xcp_put() {
  if [ -n "${XCP_PW:-}" ]; then
    sshpass -p "$XCP_PW" scp -o StrictHostKeyChecking=no "$1" "$XCP_USER@$XCP_HOST:$2"
  else
    scp -o StrictHostKeyChecking=no "$1" "$XCP_USER@$XCP_HOST:$2"
  fi
}

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# 1. Mint the node cert on the CA lighthouse + collect ca.crt + the static nebula.
log "minting cert for '$NAME' @ $OVERLAY_IP/$OVERLAY_CIDR_BITS on the CA ($CA_HOST)"
lh "set -e
  cd /var/lib/mackesd/nebula-ca 2>/dev/null || cd /etc/nebula
  nebula-cert sign -ca-crt ca.crt -ca-key ca.key -name '$NAME' -ip '$OVERLAY_IP/$OVERLAY_CIDR_BITS' \
    -out-crt /tmp/$NAME.crt -out-key /tmp/$NAME.key
  echo '---CA---'; cat ca.crt
  echo '---CRT---'; cat /tmp/$NAME.crt
  echo '---KEY---'; cat /tmp/$NAME.key
  rm -f /tmp/$NAME.crt /tmp/$NAME.key" > "$STAGE/bundle" || {
    echo "cert mint failed on $CA_HOST (CA key present? nebula-cert installed?)" >&2; exit 1; }
awk '/^---CA---/{o="ca.crt";next} /^---CRT---/{o="host.crt";next} /^---KEY---/{o="host.key";next} o{print > "'"$STAGE"'/"o}' "$STAGE/bundle"
for f in ca.crt host.crt host.key; do [ -s "$STAGE/$f" ] || { echo "missing $f from mint" >&2; exit 1; }; done

# 2. Get a STATICALLY-linked nebula binary for dom0.
#    CRITICAL (verified 2026-06-19 against XCP-ng 8.3.0 dom0): the Fedora-packaged
#    `/usr/bin/nebula` is DYNAMICALLY linked against glibc 2.34 and will NOT run on
#    an XCP-ng dom0 (CentOS-7 base, glibc 2.17) — it dies with
#    `version 'GLIBC_2.34' not found`. The official SlackHQ release tarball ships a
#    statically-linked Go binary that runs on the old dom0 glibc. Prefer a locally
#    cached static binary ($NEBULA_STATIC); else download the pinned release.
if [ -n "${NEBULA_STATIC:-}" ] && [ -s "$NEBULA_STATIC" ]; then
  log "using cached static nebula: $NEBULA_STATIC"
  cp "$NEBULA_STATIC" "$STAGE/nebula"
else
  log "downloading the official static nebula $NEBULA_VERSION release"
  curl -fsSL -o "$STAGE/nebula.tgz" \
    "https://github.com/slackhq/nebula/releases/download/${NEBULA_VERSION}/nebula-linux-amd64.tar.gz"
  tar xzf "$STAGE/nebula.tgz" -C "$STAGE" nebula
fi
chmod +x "$STAGE/nebula"
# Refuse to push a dynamic binary — it would fail on dom0's old glibc.
if file "$STAGE/nebula" | grep -q 'dynamically linked'; then
  echo "onboard-xcp: the nebula binary is dynamically linked — it will NOT run on the XCP dom0 (glibc 2.17). Use the static SlackHQ release." >&2
  exit 1
fi

# 3. Render dom0's config.yml — a non-lighthouse member pointed at both lighthouses,
#    open-mesh firewall (AI_GOVERNANCE §8: a valid mesh cert reaches every peer).
cat > "$STAGE/config.yml" <<EOF
pki:
  ca: /etc/nebula/ca.crt
  cert: /etc/nebula/host.crt
  key: /etc/nebula/host.key
static_host_map:
  "$CA_OVERLAY": ["$CA_HOST:4242"]
  "$LH2_OVERLAY": ["$LH2_PUB:4242"]
lighthouse:
  am_lighthouse: false
  hosts:
    - "$CA_OVERLAY"
    - "$LH2_OVERLAY"
listen:
  host: 0.0.0.0
  port: 4242
punchy:
  punch: true
  respond: true
cipher: aes
tun:
  dev: nebula1
  mtu: 1300
firewall:
  outbound:
    - port: any
      proto: any
      host: any
  inbound:
    - port: any
      proto: any
      host: any
EOF

# 4. Install on dom0 + a systemd unit, then start.
log "pushing nebula + /etc/nebula to dom0 ($XCP_HOST)"
xcp 'mkdir -p /etc/nebula /opt/mcnf/bin'
xcp_put "$STAGE/nebula" /opt/mcnf/bin/nebula
xcp_put "$STAGE/ca.crt" /etc/nebula/ca.crt
xcp_put "$STAGE/host.crt" /etc/nebula/host.crt
xcp_put "$STAGE/host.key" /etc/nebula/host.key
xcp_put "$STAGE/config.yml" /etc/nebula/config.yml
xcp 'chmod +x /opt/mcnf/bin/nebula; chmod 600 /etc/nebula/host.key
  cat > /etc/systemd/system/nebula.service <<UNIT
[Unit]
Description=MCNF Nebula overlay (XCP host)
After=network-online.target
Wants=network-online.target
[Service]
ExecStart=/opt/mcnf/bin/nebula -config /etc/nebula/config.yml
Restart=always
RestartSec=5
[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable nebula.service
  systemctl restart nebula.service
  sleep 4
  systemctl is-active nebula.service && ip -o -4 addr show nebula1 2>/dev/null | awk "{print \$4}"'

# 5. Verify overlay reachability dom0 → lighthouse.
log "verifying overlay reachability ($OVERLAY_IP → $CA_OVERLAY)"
if xcp "ping -c2 -W3 $CA_OVERLAY >/dev/null 2>&1"; then
  log "SUCCESS — $NAME ($OVERLAY_IP) is on the mesh overlay + reaches the lighthouse"
else
  echo "onboard-xcp: nebula started but overlay ping to $CA_OVERLAY failed — check UDP 4242 egress from dom0 + the lighthouse static_host_map" >&2
  exit 1
fi
