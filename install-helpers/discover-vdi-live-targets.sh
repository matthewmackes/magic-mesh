#!/usr/bin/env bash
# discover-vdi-live-targets.sh -- bounded, read-only VDI endpoint inventory.
#
# This is discovery only.  A listener is a candidate for a later real-worker
# framebuffer proof, never proof that a guest rendered or accepted input.  The
# helper deliberately accepts no password, ticket, or SSH-key argument, stores
# no probe output, and emits neither banners nor SSH diagnostics.

set -euo pipefail

readonly SCHEMA_VERSION=1
readonly REMOTE_LISTENERS='ss -H -ltn 2>/dev/null | awk "{ n=split(\$4, parts, \":\"); port=parts[n]; if (port == 3389 || (port >= 5900 && port <= 5909) || port == 5930) print port }" | sort -nu'

usage() {
  cat <<'EOF'
Usage:
  discover-vdi-live-targets.sh --seat NAME=HOST [--seat NAME=HOST ...]
  discover-vdi-live-targets.sh --self-test

Read-only inventory of explicit, operator-approved proof seats.  It uses the
caller's existing SSH agent/config in batch mode, with strict known-host
checking, and reports only listening VNC (:5900-:5909), SPICE (:5930), and RDP
(:3389) candidate endpoints.  It never accepts or emits credentials, tickets,
protocol banners, or raw SSH/probe logs.

Each --seat value must be a simple NAME=HOST pair.  HOST is passed only as an
SSH destination; use an approved hostname or IP address already present in the
operator's known_hosts.  JSON output is written to stdout.
EOF
}

die() {
  printf 'discover-vdi-live-targets: %s\n' "$*" >&2
  exit 2
}

json_string() {
  # Inputs are validated to this helper's restricted identifier grammar.
  printf '"%s"' "$1"
}

valid_name() {
  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$ ]]
}

valid_host() {
  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.:-]{0,252}$ ]]
}

parse_seat() {
  local raw=$1 name host
  [[ "$raw" == *=* ]] || die "--seat must be NAME=HOST"
  name=${raw%%=*}
  host=${raw#*=}
  valid_name "$name" || die "invalid seat name: $name"
  valid_host "$host" || die "invalid seat host: $host"
  printf '%s\t%s\n' "$name" "$host"
}

ports_from_listing() {
  local line port
  local -A seen=()
  while IFS= read -r line; do
    [[ "$line" =~ ^[0-9]{1,5}$ ]] || continue
    port=$line
    (( port >= 1 && port <= 65535 )) || continue
    case "$port" in
      3389|5930|590[0-9]) ;;
      *) continue ;;
    esac
    [[ -v "seen[$port]" ]] && continue
    seen[$port]=1
    printf '%s\n' "$port"
  done | sort -n
}

protocol_for_port() {
  case "$1" in
    3389) printf 'rdp' ;;
    5930) printf 'spice' ;;
    590[0-9]) printf 'vnc' ;;
    *) return 1 ;;
  esac
}

probe_seat() {
  local host=$1 listing
  # BatchMode and disabled password/KI auth keep discovery non-interactive. SSH
  # diagnostics are discarded rather than retained or placed in JSON.
  if ! listing=$(ssh \
    -o BatchMode=yes \
    -o PasswordAuthentication=no \
    -o KbdInteractiveAuthentication=no \
    -o StrictHostKeyChecking=yes \
    -o ConnectTimeout=5 \
    -- "$host" "$REMOTE_LISTENERS" 2>/dev/null); then
    return 1
  fi
  ports_from_listing <<<"$listing"
}

emit_endpoint_json() {
  local port=$1 protocol
  protocol=$(protocol_for_port "$port") || return 1
  printf '{"protocol":'
  json_string "$protocol"
  printf ',"port":%s}' "$port"
}

run_inventory() {
  local -a seats=("$@")
  local i record name host ports port first_endpoint
  printf '{"schema_version":%s,"kind":"vdi_live_target_inventory","seats":[' "$SCHEMA_VERSION"
  for ((i = 0; i < ${#seats[@]}; i++)); do
    record=${seats[$i]}
    name=${record%%$'\t'*}
    host=${record#*$'\t'}
    (( i > 0 )) && printf ','
    printf '{"name":'
    json_string "$name"
    printf ',"host":'
    json_string "$host"
    if ports=$(probe_seat "$host"); then
      printf ',"status":"reachable","endpoints":['
      first_endpoint=1
      while IFS= read -r port; do
        [[ -n "$port" ]] || continue
        (( first_endpoint )) || printf ','
        emit_endpoint_json "$port"
        first_endpoint=0
      done <<<"$ports"
      printf ']}'
    else
      printf ',"status":"unavailable","endpoints":[]}'
    fi
  done
  printf ']}\n'
}

self_test() {
  local parsed
  parsed=$(parse_seat 'proof-15=172.20.0.15')
  [[ "$parsed" == $'proof-15\t172.20.0.15' ]] || die 'self-test could not parse approved seat'
  [[ $(protocol_for_port 5903) == vnc ]]
  [[ $(protocol_for_port 5930) == spice ]]
  [[ $(protocol_for_port 3389) == rdp ]]
  [[ $(ports_from_listing <<<'5904
5930
5904
9999
3389
not-a-port') == $'3389\n5904\n5930' ]] \
    || die 'self-test accepted an invalid port or failed to deduplicate'
  if valid_host 'seat;rm'; then
    die 'self-test accepted unsafe SSH destination'
  fi
  [[ "$REMOTE_LISTENERS" != *ticket* && "$REMOTE_LISTENERS" != *password* ]] \
    || die 'self-test found credential-bearing remote probe'
  printf 'discover-vdi-live-targets: self-test passed\n'
}

main() {
  local -a seats=()
  while (( $# )); do
    case "$1" in
      --seat)
        (( $# >= 2 )) || die '--seat requires NAME=HOST'
        seats+=("$(parse_seat "$2")")
        shift 2
        ;;
      --self-test)
        (( $# == 1 )) || die '--self-test does not accept other arguments'
        self_test
        return
        ;;
      --help|-h)
        usage
        return
        ;;
      *) die "unknown argument: $1" ;;
    esac
  done
  (( ${#seats[@]} > 0 )) || die 'at least one explicit --seat is required'
  run_inventory "${seats[@]}"
}

main "$@"
