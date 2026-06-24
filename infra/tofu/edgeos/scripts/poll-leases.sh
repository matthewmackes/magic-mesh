#!/usr/bin/env bash
# tofu external-data program: poll EdgeOS DHCP leases (read-only).
# Reads a JSON query {host,user,cred_file} on stdin, emits a flat JSON object
# ip => "mac|expiry|hostname" on stdout (the external-data contract requires a
# flat string=>string map). Password read from cred_file via sshpass -f.
set -euo pipefail

eval "$(jq -r '@sh "HOST=\(.host) USER_=\(.user) CRED=\(.cred_file)"')"
[[ -r "$CRED" ]] || { echo '{}'; exit 0; }

raw=$(sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
        -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${USER_}@${HOST}" \
        'bash -l -c "/opt/vyatta/bin/vyatta-op-cmd-wrapper show dhcp leases 2>/dev/null"' 2>/dev/null || true)

# Table rows look like: IP  MAC  YYYY/MM/DD HH:MM:SS  Pool  [hostname...]
flat=$(awk '
  $1 ~ /^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$/ {
    ip=$1; mac=$2; expiry=$3" "$4; name="";
    for (i=6; i<=NF; i++) name = name (i>6 ? " " : "") $i;
    printf "%s\t%s|%s|%s\n", ip, mac, expiry, name
  }' <<<"$raw")

jq -Rn '[inputs | select(length>0) | split("\t") | {(.[0]): .[1]}] | add // {}' <<<"$flat"
