#!/usr/bin/env bash
# ROUTER-8 — converge EdgeOS/VyOS destination-NAT (port-forward) rules to
# EDGEOS_NAT_DESIRED via direct Vyatta config edit + reload over SSH. Sibling of
# apply-firewall.sh / apply-dhcp.sh.
#
# EDGEOS_NAT_DESIRED is a JSON object keyed by NAT rule number:
#   { "<num>": { "<attr>": "<val>", ... }, ... }
# where <attr>/<val> are Vyatta `service nat rule <num>` leaves, e.g.
#   "type":"destination", "inbound-interface":"eth0", "protocol":"tcp",
#   "description":"https→navidrome", "destination port":"443",
#   "inside-address address":"10.42.0.5", "inside-address port":"4533".
# (A space in the attr => a nested config node.)
#
# Converge model: each desired rule number is delete+recreated (idempotent →
# exact). ADDITIVE (§6): only the rule NUMBERS present in the desired map are
# managed; NAT rules the operator authored elsewhere are left untouched; empty
# map = manage nothing. Applied inside `commit-confirm <min>` so a forward that
# breaks reachability auto-reverts.
#
# Password from EDGEOS_CRED_FILE via sshpass -f (never argv).
set -euo pipefail

HOST="${EDGEOS_HOST:?need EDGEOS_HOST}"
USER_="${EDGEOS_USER:?need EDGEOS_USER}"
CRED="${EDGEOS_CRED_FILE:?need EDGEOS_CRED_FILE}"
DESIRED="${EDGEOS_NAT_DESIRED:?need EDGEOS_NAT_DESIRED}"
CONFIRM_MIN="${EDGEOS_NAT_CONFIRM_MIN:-2}"
[[ -r "$CRED" ]] || { echo "edgeos-nat: cred file $CRED not readable" >&2; exit 1; }

ssh_e() {
  sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${USER_}@${HOST}" "$@"
}

desired_n=$(jq 'length' <<<"$DESIRED")
if [[ "$desired_n" -eq 0 ]]; then
  echo "edgeos-nat: no rules to manage (nat_rules empty) — no change."
  exit 0
fi

BASE="service nat rule"
cmds=$(mktemp); trap 'rm -f "$cmds"' EXIT

# each desired NAT rule: delete + recreate its attribute leaves
while IFS= read -r num; do
  [[ -z "$num" ]] && continue
  echo "delete $BASE $num"
  while IFS=$'\t' read -r attr val; do
    [[ -z "$attr" ]] && continue
    echo "set $BASE $num $attr '$val'"
  done < <(jq -r --arg n "$num" '.[$n] | to_entries[] | "\(.key)\t\(.value)"' <<<"$DESIRED")
done < <(jq -r 'keys[]' <<<"$DESIRED") >>"$cmds"

if [[ ! -s "$cmds" ]]; then
  echo "edgeos-nat: nothing to apply."
  exit 0
fi

if [[ "${EDGEOS_DRY_RUN:-0}" == "1" ]]; then
  echo "edgeos-nat: DRY-RUN — would apply $(wc -l <"$cmds") config line(s) to ${HOST}:"
  sed 's/^/  /' "$cmds"
  exit 0
fi

echo "edgeos-nat: applying $(wc -l <"$cmds") line(s) to ${HOST} (commit-confirm ${CONFIRM_MIN}m)…"
out=$( { echo 'source /opt/vyatta/etc/functions/script-template'
         echo 'configure'
         cat "$cmds"
         echo "commit-confirm ${CONFIRM_MIN}"
         echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
if grep -qiE 'commit failed|set failed|delete failed|^error|invalid' <<<"$out"; then
  echo "edgeos-nat: APPLY ERROR (commit-confirm will auto-revert):" >&2
  grep -iE 'fail|error|invalid' <<<"$out" | head >&2
  exit 1
fi

sleep 3
if ssh_e 'true' 2>/dev/null; then
  conf=$( { echo 'source /opt/vyatta/etc/functions/script-template'
            echo 'configure'; echo 'confirm'; echo 'save'; echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
  if grep -qiE 'fail|error' <<<"$conf"; then
    echo "edgeos-nat: confirm/save reported an issue:" >&2; echo "$conf" | head >&2; exit 1
  fi
  echo "edgeos-nat: converged ${desired_n} NAT rule(s) — confirmed + saved."
else
  echo "edgeos-nat: LOST REACHABILITY after apply — NOT confirming; commit-confirm auto-reverts in ${CONFIRM_MIN}m." >&2
  exit 1
fi
