#!/usr/bin/env bash
# farm-sccache-proof.sh - verify the WL-BUILD-002 shared sccache contract.
#
# This is a proof/reporting helper, not an installer. It checks every canonical
# farm VM for the build-time pieces that make cross-node cache hits possible:
# a sccache binary, ~/.sccache.env, RUSTC_WRAPPER=sccache, CARGO_INCREMENTAL=0,
# and a non-empty S3 endpoint/bucket. It never prints access keys.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=farm-topology.sh
. "$HERE/farm-topology.sh"

KEY="${MCNF_FARM_KEY:-$FARM_KEY}"
USER_="${MCNF_BUILD_USER:-$FARM_SSH_USER}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=10)

usage() {
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Usage:
  install-helpers/farm-sccache-proof.sh [status]

Exit 0 means every canonical build VM has the shared-cache contract configured.
Exit 1 means at least one VM is missing a required piece or is unreachable.
EOF
}

remote_probe='
set -eu
host="$(hostname 2>/dev/null || echo unknown)"
has_sccache=no
command -v sccache >/dev/null 2>&1 && has_sccache=yes
has_env=no
[ -f "$HOME/.sccache.env" ] && has_env=yes
wrapper=""
incremental=""
endpoint=""
bucket=""
stats="not-run"
if [ "$has_env" = yes ]; then
  set +u
  . "$HOME/.sccache.env"
  set -u
  wrapper="${RUSTC_WRAPPER:-}"
  incremental="${CARGO_INCREMENTAL:-}"
  endpoint="${SCCACHE_ENDPOINT:-}"
  bucket="${SCCACHE_BUCKET:-}"
fi
if [ "$has_sccache" = yes ] && [ "$has_env" = yes ]; then
  stats="$(
    set +e
    sccache --show-stats 2>/dev/null \
      | awk -F: '"'"'
          /Compile requests|Cache hits|Cache misses|Non-cacheable/ {
            key=$1; value=$2
            gsub(/^[ \t]+|[ \t]+$/, "", key)
            gsub(/^[ \t]+|[ \t]+$/, "", value)
            if (value != "") {
              printf "%s=%s;", key, value
            }
          }
        '"'"'
  )"
  [ -n "$stats" ] || stats="unavailable"
fi
printf "%s|%s|%s|%s|%s|%s|%s|%s\n" "$host" "$has_sccache" "$has_env" "$wrapper" "$incremental" "$endpoint" "$bucket" "$stats"
'

print_header() {
  printf '| Node | Hostname | sccache | env | wrapper | incremental | endpoint | bucket | stats |\n'
  printf '| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n'
}

status() {
  local rc=0 i octet name ip out host has_sccache has_env wrapper incremental endpoint bucket stats
  print_header
  for i in "${!FARM_OCTETS[@]}"; do
    octet="${FARM_OCTETS[$i]}"
    name="${FARM_NAMES[$i]}"
    ip="172.20.0.$octet"
    if ! out="$("${SSH[@]}" "$USER_@$ip" "$remote_probe" 2>&1)"; then
      printf '| .%-3s | %s | no | no | - | - | - | - | unreachable: %s |\n' \
        "$octet" "$name" "$(printf '%s' "$out" | tr '\n' ' ' | sed 's/|/ /g')"
      rc=1
      continue
    fi
    IFS='|' read -r host has_sccache has_env wrapper incremental endpoint bucket stats <<<"$out"
    [ "$has_sccache" = yes ] || rc=1
    [ "$has_env" = yes ] || rc=1
    [ "$wrapper" = sccache ] || rc=1
    [ "$incremental" = 0 ] || rc=1
    [ -n "$endpoint" ] || rc=1
    [ -n "$bucket" ] || rc=1
    printf '| .%-3s | %s | %s | %s | %s | %s | %s | %s | %s |\n' \
      "$octet" "$host" "$has_sccache" "$has_env" \
      "${wrapper:-"-"}" "${incremental:-"-"}" "${endpoint:-"-"}" \
      "${bucket:-"-"}" "${stats:-"-"}"
  done
  return "$rc"
}

case "${1:-status}" in
  status) status ;;
  -h|--help|help) usage ;;
  *) echo "farm-sccache-proof.sh: unknown command: $1" >&2; usage >&2; exit 2 ;;
esac
