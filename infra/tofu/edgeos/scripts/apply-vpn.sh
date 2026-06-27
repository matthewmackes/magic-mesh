#!/usr/bin/env bash
# ROUTER-9 — converge EdgeOS/VyOS VPN endpoint config to EDGEOS_VPN_DESIRED via
# direct Vyatta config edit + reload over SSH. Sibling of apply-firewall.sh, but
# generalized: each entry is a MANAGED CONFIG ROOT (a VPN tunnel / interface /
# peer subtree) that is delete+recreated to exactly the desired leaves.
#
# EDGEOS_VPN_DESIRED is a JSON object keyed by the managed root config path:
#   { "<root path>": { "<leaf subpath>": "<val>", ... }, ... }
# e.g. a WireGuard endpoint:
#   { "interfaces wireguard wg0": {
#        "address": "10.50.0.1/24", "private-key": "/config/auth/wg0.key",
#        "port": "51820",
#        "peer SITE-B allowed-ips": "10.50.0.2/32",
#        "peer SITE-B endpoint": "203.0.113.7:51820",
#        "peer SITE-B persistent-keepalive": "25" } }
# or an IPsec peer: { "vpn ipsec site-to-site peer 203.0.113.7": { ... } }.
#
# Converge model: each managed root is delete+recreated (idempotent → exact).
# ADDITIVE (§6): only the roots present in the desired map are managed; VPN
# config the operator authored elsewhere is left untouched; empty = no-op.
# Applied inside `commit-confirm <min>` so a VPN change that breaks reachability
# auto-reverts. Password from EDGEOS_CRED_FILE via sshpass -f (never argv).
set -euo pipefail

HOST="${EDGEOS_HOST:?need EDGEOS_HOST}"
USER_="${EDGEOS_USER:?need EDGEOS_USER}"
CRED="${EDGEOS_CRED_FILE:?need EDGEOS_CRED_FILE}"
DESIRED="${EDGEOS_VPN_DESIRED:?need EDGEOS_VPN_DESIRED}"
CONFIRM_MIN="${EDGEOS_VPN_CONFIRM_MIN:-2}"
[[ -r "$CRED" ]] || { echo "edgeos-vpn: cred file $CRED not readable" >&2; exit 1; }

ssh_e() {
  sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${USER_}@${HOST}" "$@"
}

desired_n=$(jq 'length' <<<"$DESIRED")
if [[ "$desired_n" -eq 0 ]]; then
  echo "edgeos-vpn: no roots to manage (vpn_config empty) — no change."
  exit 0
fi

cmds=$(mktemp); trap 'rm -f "$cmds"' EXIT

# each managed root: delete + recreate its leaves
while IFS= read -r root; do
  [[ -z "$root" ]] && continue
  echo "delete $root"
  while IFS=$'\t' read -r leaf val; do
    [[ -z "$leaf" ]] && continue
    echo "set $root $leaf '$val'"
  done < <(jq -r --arg r "$root" '.[$r] | to_entries[] | "\(.key)\t\(.value)"' <<<"$DESIRED")
done < <(jq -r 'keys[]' <<<"$DESIRED") >>"$cmds"

if [[ ! -s "$cmds" ]]; then
  echo "edgeos-vpn: nothing to apply."
  exit 0
fi

if [[ "${EDGEOS_DRY_RUN:-0}" == "1" ]]; then
  echo "edgeos-vpn: DRY-RUN — would apply $(wc -l <"$cmds") config line(s) to ${HOST}:"
  sed 's/^/  /' "$cmds"
  exit 0
fi

echo "edgeos-vpn: applying $(wc -l <"$cmds") line(s) to ${HOST} (commit-confirm ${CONFIRM_MIN}m)…"
out=$( { echo 'source /opt/vyatta/etc/functions/script-template'
         echo 'configure'
         cat "$cmds"
         echo "commit-confirm ${CONFIRM_MIN}"
         echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
if grep -qiE 'commit failed|set failed|delete failed|^error|invalid' <<<"$out"; then
  echo "edgeos-vpn: APPLY ERROR (commit-confirm will auto-revert):" >&2
  grep -iE 'fail|error|invalid' <<<"$out" | head >&2
  exit 1
fi

sleep 3
if ssh_e 'true' 2>/dev/null; then
  conf=$( { echo 'source /opt/vyatta/etc/functions/script-template'
            echo 'configure'; echo 'confirm'; echo 'save'; echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
  if grep -qiE 'fail|error' <<<"$conf"; then
    echo "edgeos-vpn: confirm/save reported an issue:" >&2; echo "$conf" | head >&2; exit 1
  fi
  echo "edgeos-vpn: converged ${desired_n} VPN root(s) — confirmed + saved."
else
  echo "edgeos-vpn: LOST REACHABILITY after apply — NOT confirming; commit-confirm auto-reverts in ${CONFIRM_MIN}m." >&2
  exit 1
fi
