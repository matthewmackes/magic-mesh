#!/usr/bin/env bash
# rejoin-v11-mesh.sh — bring an old-mesh node onto the new v11 (SUBSTRATE-V2-bound)
# mesh in one shot. Run ON the node being rejoined (e.g. .13/eagle).
#
#   sudo ./rejoin-v11-mesh.sh [lighthouse-public-ip] [role] ['<join-token>']
#     lighthouse-public-ip  default 174.138.68.216 (lighthouse-01)
#     role                  default workstation
#     join-token            optional; if omitted, minted via ssh to the lighthouse
#
# One-liner:
#   curl -sL https://raw.githubusercontent.com/matthewmackes/magic-mesh/master/install-helpers/rejoin-v11-mesh.sh | sudo bash -s -- 174.138.68.216
set -uo pipefail
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)"; exit 1; }
LH="${1:-174.138.68.216}"; ROLE="${2:-workstation}"; TOKEN="${3:-}"

echo "==> [1/4] upgrade to 11.0.1 (FOUND-NEBULA fix)"
rm -f /etc/yum.repos.d/mackes-mirror-magic-mesh.repo             # dead file:// mirror → dnf error 37
. /etc/os-release
URL="https://github.com/matthewmackes/magic-mesh/releases/download/magic-mesh-v11.0.1/magic-mesh-11.0.1-1.fc${VERSION_ID}.x86_64.rpm"
dnf install -y --refresh "$URL" >/tmp/rejoin-dnf.log 2>&1 \
  && echo "    $(rpm -q magic-mesh)" \
  || { echo "    UPGRADE FAILED:"; tail -8 /tmp/rejoin-dnf.log; exit 1; }

echo "==> [2/4] obtain a single-use join token"
if [ -z "$TOKEN" ]; then
  TOKEN="$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "root@${LH}" \
            "mackesd add-peer --role ${ROLE}" 2>/dev/null | grep -m1 '^mesh:')"
fi
[ -n "$TOKEN" ] || { echo "    NO TOKEN — mint one on the lighthouse and pass it:"; \
  echo "      ssh root@${LH} 'mackesd add-peer --role ${ROLE}'"; \
  echo "      sudo $0 ${LH} ${ROLE} '<token>'"; exit 1; }
echo "    token: ${TOKEN:0:48}..."

echo "==> [3/4] leave the dead old mesh + join the new one"
systemctl stop mackesd 2>/dev/null || true
timeout 45 mackesd leave 2>/dev/null || echo "    (old master gone; local wipe applied)"
timeout 90 mackesd join "$TOKEN" --role "$ROLE" 2>&1 | tail -6

echo "==> [4/4] verify"
sleep 3
echo "    overlay: $(ip -4 -o addr show nebula1 2>/dev/null | awk '{print $4}')"
echo "    mackesd: $(systemctl is-active mackesd 2>/dev/null)  nebula: $(systemctl is-active nebula 2>/dev/null)"
ping -c2 -W2 10.42.0.1 >/dev/null 2>&1 && echo "    lighthouse 10.42.0.1: REACHABLE ✓" || echo "    lighthouse 10.42.0.1: not reachable yet (give nebula ~10s)"
