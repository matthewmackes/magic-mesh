#!/usr/bin/env bash
# BOOT-STATUS-2 — the shared, truthful boot-status feed and console renderer.
# systemd remains the authority; this helper only projects its state into a
# bounded runtime feed for the Construct splash and tty1.
set -euo pipefail

readonly STATE_DIR="${MCNF_BOOT_STATUS_DIR:-/run/mde}"
readonly STATE_FILE="$STATE_DIR/boot-status.tsv"
readonly READY_FILE="$STATE_DIR/shell-ready"
readonly TTY="${MCNF_BOOT_STATUS_TTY:-/dev/tty1}"

# The list is intentionally explicit and stable: these are node-owned services
# whose startup materially determines what this node can do. Missing/role-gated
# units are rendered as skipped rather than invented as failures.
readonly UNITS=(
  "user@1000.service|Seat user manager"
  "nebula.service|Nebula mesh overlay"
  "etcd.service|Mesh coordination"
  "syncthing.service|Mesh file sync"
  "mackesd-control.service|Control plane"
  "mackesd-observation.service|Observation plane"
  "mackesd-actions.service|Actions plane"
  "mackesd-data.service|Data plane"
  "mackesd-compute.service|Compute plane"
  "mackesd-integrations.service|Integrations plane"
  "mesh-health.service|Node health"
  "mesh-status.service|Mesh status snapshot"
)

mkdir -p "$STATE_DIR"
# /run is boot-scoped, so a ready marker cannot survive a reboot.  Preserve it
# on service restarts: deleting a live shell's marker would let this projector
# reclaim tty1 after Construct already owns the seat.

log_event() {
  # journald is the platform log authority; logger is available in the base
  # image and preserves the event for journalctl -t mcnf-boot-status.
  logger -t mcnf-boot-status -- "$*" 2>/dev/null || true
}

unit_value() {
  local unit="$1" property="$2"
  systemctl show "$unit" -p "$property" --value 2>/dev/null || true
}

snapshot() {
  local tmp="$STATE_FILE.tmp.$$"
  {
    printf 'version\t1\n'
    local entry unit label active sub result discovered exists prior
    local entries=("${UNITS[@]}")
    # Keep the contract future-proof: new node-owned services are included
    # automatically when they use the platform's service naming families.
    while read -r discovered; do
      [[ -z "$discovered" || "$discovered" == mcnf-boot-status.service ]] && continue
      exists=0
      for prior in "${entries[@]}"; do
        [[ "${prior%%|*}" == "$discovered" ]] && exists=1 && break
      done
      if [[ "$exists" -eq 0 ]]; then
        label="$(unit_value "$discovered" Description)"
        [[ -n "$label" ]] || label="$discovered"
        entries+=("$discovered|$label")
      fi
    done < <(
      systemctl list-unit-files --type=service --no-legend --no-pager 2>/dev/null \
        | awk '$1 ~ /^(mcnf-|mackesd-|mesh-|mde-.*\.service$|nebula\.service$|etcd\.service$|syncthing\.service$|magic-setup\.service$|user@1000\.service$)/ { print $1 }'
    )
    for entry in "${entries[@]}"; do
      unit="${entry%%|*}"
      label="${entry#*|}"
      active="$(unit_value "$unit" ActiveState)"
      sub="$(unit_value "$unit" SubState)"
      result="$(unit_value "$unit" Result)"
      if [[ -z "$active" ]]; then
        active=skipped
        sub=missing
        result=success
      fi
      printf '%s\t%s\t%s\t%s\t%s\n' "$unit" "$label" "$active" "$sub" "$result"
    done
  } >"$tmp"
  mv -f "$tmp" "$STATE_FILE"
}

spinner=("⠋" "⠙" "⠹" "⠸" "⠼" "⠴" "⠦" "⠧" "⠇" "⠏")
frame=0

render_console() {
  [[ -t 1 || -w "$TTY" ]] || return 0
  [[ -e "$READY_FILE" ]] && return 0
  local out=$'\033[H\033[2J\033[H'
  out+=$'\033[1;38;5;45m◆  MAGIC MESH  /  INFORMATIVE BOOT\033[0m\n'
  out+=$'\033[38;5;117m   Establishing the node\'s operational fabric\033[0m\n\n'
  local line unit label active sub result glyph color
  while IFS=$'\t' read -r unit label active sub result; do
    [[ "$unit" == version ]] && continue
    case "$active" in
      active) glyph='✓'; color=$'\033[1;38;5;82m' ;;
      failed|deactivating) glyph='✕'; color=$'\033[1;5;31m' ;;
      skipped|inactive) glyph='·'; color=$'\033[38;5;245m' ;;
      *) glyph="${spinner[$((frame % ${#spinner[@]}))]}"; color=$'\033[1;38;5;220m' ;;
    esac
    out+="${color}${glyph}${RESET:-$'\033[0m'} ${label} ${DIM:-$'\033[38;5;245m'}[${active}/${sub}]${RESET:-$'\033[0m'}\n"
  done <"$STATE_FILE"
  out+=$'\n\033[38;5;117m   Live status is recorded in the system journal.\033[0m\n'
  printf '%s' "$out" >"$TTY" 2>/dev/null || true
}

last=""
while :; do
  snapshot
  current="$(sha256sum "$STATE_FILE" 2>/dev/null | cut -d' ' -f1 || true)"
  if [[ "$current" != "$last" ]]; then
    log_event "boot status feed updated"
    last="$current"
  fi
  render_console
  if [[ -e "$READY_FILE" ]]; then
    log_event "graphical shell owns the seat; boot status projection complete"
    exit 0
  fi
  frame=$((frame + 1))
  sleep 0.5
done
