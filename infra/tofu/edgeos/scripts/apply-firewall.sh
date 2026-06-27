#!/usr/bin/env bash
# ROUTER-7 — converge EdgeOS/VyOS firewall rulesets to EDGEOS_FW_DESIRED via
# direct Vyatta config edit + reload over SSH. Sibling of apply-dhcp.sh.
#
# EDGEOS_FW_DESIRED is a JSON object keyed by ruleset name:
#   { "<ruleset>": {
#       "default-action": "drop|accept|reject",
#       "description":    "<text>",                 # optional
#       "rule": { "<num>": { "<attr>": "<val>", ... }, ... }
#   }, ... }
# Each rule's <attr>/<val> pairs are Vyatta firewall rule leaves, e.g.
#   "action":"accept", "protocol":"tcp", "description":"ssh",
#   "destination port":"22", "source address":"10.42.0.0/17",
#   "state established":"enable".  (A space in the attr => nested node.)
#
# Converge model: each desired ruleset is delete+recreated (idempotent → exact);
# rulesets on the device but absent from desired are deleted. ALL changes apply
# in ONE `commit-confirm <min>` window so a rule that locks us out auto-reverts
# (design lock #15) — the script re-checks SSH reachability after the commit and
# only `confirm`s (makes permanent) + `save`s if the box is still reachable.
#
# Password from EDGEOS_FW_CRED_FILE/EDGEOS_CRED_FILE via sshpass -f (never argv).
set -euo pipefail

HOST="${EDGEOS_HOST:?need EDGEOS_HOST}"
USER_="${EDGEOS_USER:?need EDGEOS_USER}"
CRED="${EDGEOS_CRED_FILE:?need EDGEOS_CRED_FILE}"
DESIRED="${EDGEOS_FW_DESIRED:?need EDGEOS_FW_DESIRED}"
CONFIRM_MIN="${EDGEOS_FW_CONFIRM_MIN:-2}"
[[ -r "$CRED" ]] || { echo "edgeos-fw: cred file $CRED not readable" >&2; exit 1; }

ssh_e() {
  sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${USER_}@${HOST}" "$@"
}

# --- additive scope (§6): manage ONLY the named rulesets in EDGEOS_FW_DESIRED.
# We never delete a ruleset the operator authored but didn't hand us — empty
# desired = manage nothing (no-op), NOT wipe-all.
desired_n=$(jq 'length' <<<"$DESIRED")
if [[ "$desired_n" -eq 0 ]]; then
  echo "edgeos-fw: no rulesets to manage (firewall_rulesets empty) — no change."
  exit 0
fi

BASE="firewall name"
cmds=$(mktemp); trap 'rm -f "$cmds"' EXIT

# desired rulesets: delete + recreate (converge to exactly the desired spec).
while IFS= read -r rs; do
  [[ -z "$rs" ]] && continue
  echo "delete $BASE $rs"
  da=$(jq -r --arg rs "$rs" '.[$rs]["default-action"] // empty' <<<"$DESIRED")
  [[ -n "$da" ]] && echo "set $BASE $rs default-action $da"
  desc=$(jq -r --arg rs "$rs" '.[$rs].description // empty' <<<"$DESIRED")
  [[ -n "$desc" ]] && echo "set $BASE $rs description '$desc'"
  # each rule's attribute leaves
  while IFS= read -r num; do
    [[ -z "$num" ]] && continue
    while IFS=$'\t' read -r attr val; do
      [[ -z "$attr" ]] && continue
      echo "set $BASE $rs rule $num $attr '$val'"
    done < <(jq -r --arg rs "$rs" --arg n "$num" \
      '.[$rs].rule[$n] | to_entries[] | "\(.key)\t\(.value)"' <<<"$DESIRED")
  done < <(jq -r --arg rs "$rs" '.[$rs].rule // {} | keys[]' <<<"$DESIRED")
done < <(jq -r 'keys[]' <<<"$DESIRED") >>"$cmds"

if [[ ! -s "$cmds" ]]; then
  echo "edgeos-fw: nothing to apply."
  exit 0
fi

if [[ "${EDGEOS_DRY_RUN:-0}" == "1" ]]; then
  echo "edgeos-fw: DRY-RUN — would apply $(wc -l <"$cmds") config line(s) to ${HOST}:"
  sed 's/^/  /' "$cmds"
  exit 0
fi

# --- apply inside a commit-confirm window (auto-revert on self-lockout) ------
echo "edgeos-fw: applying $(wc -l <"$cmds") line(s) to ${HOST} (commit-confirm ${CONFIRM_MIN}m)…"
out=$( { echo 'source /opt/vyatta/etc/functions/script-template'
         echo 'configure'
         cat "$cmds"
         echo "commit-confirm ${CONFIRM_MIN}"
         echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
if grep -qiE 'commit failed|set failed|delete failed|^error|invalid' <<<"$out"; then
  echo "edgeos-fw: APPLY ERROR (commit-confirm will auto-revert):" >&2
  grep -iE 'fail|error|invalid' <<<"$out" | head >&2
  exit 1
fi

# Still reachable after the change? If yes, make it permanent; else let the
# commit-confirm timer auto-revert (we never reach the confirm).
sleep 3
if ssh_e 'true' 2>/dev/null; then
  conf=$( { echo 'source /opt/vyatta/etc/functions/script-template'
            echo 'configure'; echo 'confirm'; echo 'save'; echo 'exit'; } | ssh_e 'bash -l' 2>&1 || true )
  if grep -qiE 'fail|error' <<<"$conf"; then
    echo "edgeos-fw: confirm/save reported an issue:" >&2; echo "$conf" | head >&2; exit 1
  fi
  echo "edgeos-fw: converged to ${desired_n} ruleset(s) — confirmed + saved."
else
  echo "edgeos-fw: LOST REACHABILITY after apply — NOT confirming; commit-confirm will auto-revert in ${CONFIRM_MIN}m." >&2
  exit 1
fi
