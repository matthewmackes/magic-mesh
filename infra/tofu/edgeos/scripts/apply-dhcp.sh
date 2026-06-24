#!/usr/bin/env bash
# Converge EdgeOS DHCP static-mappings to EDGEOS_DESIRED (a JSON map
# name => {mac, ip}) via direct config edit + reload over SSH.
#
# Idempotent: fetches the current mappings, computes the diff, and only opens a
# config session (configure / set|delete / commit / save) when something must
# change. Password is read from EDGEOS_CRED_FILE via sshpass -f — never argv.
set -euo pipefail

HOST="${EDGEOS_HOST:?need EDGEOS_HOST}"
USER_="${EDGEOS_USER:?need EDGEOS_USER}"
CRED="${EDGEOS_CRED_FILE:?need EDGEOS_CRED_FILE}"
NET="${EDGEOS_NETWORK:?need EDGEOS_NETWORK}"
SUBNET="${EDGEOS_SUBNET:?need EDGEOS_SUBNET}"
DESIRED="${EDGEOS_DESIRED:?need EDGEOS_DESIRED}"
[[ -r "$CRED" ]] || { echo "edgeos-dhcp: cred file $CRED not readable" >&2; exit 1; }

ssh_e() {
  sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${USER_}@${HOST}" "$@"
}

# --- safety: never silently wipe every reservation -------------------------
desired_n=$(jq 'length' <<<"$DESIRED")
if [[ "$desired_n" -eq 0 && "${EDGEOS_ALLOW_EMPTY:-0}" != "1" ]]; then
  echo "edgeos-dhcp: REFUSING — desired set is empty (would remove ALL reservations)." >&2
  echo "             set EDGEOS_ALLOW_EMPTY=1 to force." >&2
  exit 1
fi

# --- fetch current static-mappings: name<TAB>ip<TAB>mac ---------------------
current=$(ssh_e 'bash -l -c "/opt/vyatta/bin/vyatta-op-cmd-wrapper show configuration commands 2>/dev/null | grep static-mapping"' 2>/dev/null \
  | grep -E "static-mapping .* (ip-address|mac-address)" \
  | sed -E "s/.*static-mapping ([^ ]+) (ip-address|mac-address) '?([^']*)'?.*/\1|\2|\3/" \
  | awk -F'|' '{k=$1; if($2=="ip-address")ip[k]=$3; else mac[k]=$3}
               END{for(k in ip)printf "%s\t%s\t%s\n",k,ip[k],mac[k]}')

BASE="service dhcp-server shared-network-name $NET subnet $SUBNET static-mapping"
cmds=$(mktemp); trap 'rm -f "$cmds"' EXIT

# adds / updates: desired entry whose (ip,mac) differs from current -> re-set
while IFS=$'\t' read -r name ip mac; do
  [[ -z "$name" ]] && continue
  cur=$(awk -F'\t' -v n="$name" '$1==n{print $2"\t"$3}' <<<"$current")
  if [[ "$cur" != "${ip}"$'\t'"${mac}" ]]; then
    echo "delete $BASE $name"
    echo "set $BASE $name ip-address $ip"
    echo "set $BASE $name mac-address '$mac'"
  fi
done < <(jq -r 'to_entries[] | "\(.key)\t\(.value.ip)\t\(.value.mac)"' <<<"$DESIRED") >>"$cmds"

# removes: current name absent from desired -> delete
while IFS=$'\t' read -r name ip mac; do
  [[ -z "$name" ]] && continue
  if ! jq -e --arg n "$name" 'has($n)' <<<"$DESIRED" >/dev/null; then
    echo "delete $BASE $name" >>"$cmds"
  fi
done <<<"$current"

if [[ ! -s "$cmds" ]]; then
  echo "edgeos-dhcp: already converged (${desired_n} reservations) — no change."
  exit 0
fi

if [[ "${EDGEOS_DRY_RUN:-0}" == "1" ]]; then
  echo "edgeos-dhcp: DRY-RUN — would apply $(wc -l <"$cmds") config line(s) to ${HOST}:"
  sed 's/^/  /' "$cmds"
  exit 0
fi

echo "edgeos-dhcp: applying $(wc -l <"$cmds") config line(s) to ${HOST}…"
out=$( { echo 'source /opt/vyatta/etc/functions/script-template'
         echo 'configure'
         cat "$cmds"
         echo 'commit'
         echo 'save'
         echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )

if grep -qiE 'commit failed|set failed|delete failed|error' <<<"$out"; then
  echo "edgeos-dhcp: APPLY ERROR:" >&2
  grep -iE 'fail|error' <<<"$out" | head >&2
  exit 1
fi
echo "edgeos-dhcp: converged to ${desired_n} reservations."
