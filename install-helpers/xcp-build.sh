#!/usr/bin/env bash
# xcp-build.sh — multi-slot farm build driver. Keeps heavy compute OFF the local
# AI/dev host (operator directive 2026-06-20: "only AI work local; farm all other
# work to XCP; make better use of the compute you have access to") and lets
# *parallel* build/test pipelines run at once across independent slots, so testing
# is always in flight while the AI keeps working locally.
#
# A "slot" is one isolated build environment = {host, user, key, remote_dir}. Each
# slot keeps its own target/ + warm cargo cache, so two slots never clobber each
# other and can build concurrently (ideally on different physical XCP hosts for
# true parallelism). Slots come from the registry written by xcp-slots.sh:
#
#     .xcp-slots.conf   (repo root, gitignored)
#     # name  host          user  key                               remote_dir
#     a       172.20.0.50   mm    ~/.ssh/mackes_mesh_ed25519        magic-mesh
#     b       172.20.145.40 mm    ~/.ssh/mackes_mesh_ed25519        magic-mesh
#
# If no registry exists it falls back to a single back-compat slot from the env
# (MCNF_BUILD_HOST / MCNF_BUILD_USER / MCNF_BUILD_KEY), so the original release
# flow keeps working.
#
# Usage:
#   xcp-build.sh slots                       list slots + reachability
#   xcp-build.sh sync          [--slot S]    rsync the working tree → slot
#   xcp-build.sh cargo <args…>  [--slot S]   sync + run `cargo <args>` on the slot
#   xcp-build.sh gate  <name>   [--slot S]   run ONE gate, write a structured result
#   xcp-build.sh gates          [--slot S]   run the full gate set (the ship/release gates)
#   xcp-build.sh crate <pkg> <gate> [--slot S]   per-crate gate (check|test|clippy|build)
#   xcp-build.sh render [slug] [out.png] [--slot S]   build+headless-render mde-workbench, pull the PNG
#   xcp-build.sh rpm                         release build + cargo generate-rpm; pull the RPM
#   xcp-build.sh pull  <glob>   [--slot S]   rsync artifacts back (relative to the remote repo)
#   xcp-build.sh shell          [--slot S]   interactive ssh into the slot
#   xcp-build.sh result [latest|<file>]      print the last (or named) structured result
#
# Gate names: fmt | clippy | test | check | build | boundary | carbon | bus | libcosmic
# Each gate/gates run writes .xcp-build/results/<slot>-<epoch>-<gate>.json :
#   {slot,host,gate,ok,started,duration_s,steps:[{name,ok,duration_s,exit}],error_tail}
# so the caller (AI or CI) reads pass/fail programmatically instead of scraping logs.
#
# Designed to be fired in the background:  xcp-build.sh gates --slot a   (run_in_background)
# Different --slot values are safe to run at the same time.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
# Slot registry config: explicit env override, else the repo-root file, else a
# stable per-user fallback ($HOME/.xcp-slots.conf). The fallback matters for
# ISOLATED git worktrees (parallel drain agents): a fresh worktree does NOT
# carry the gitignored repo-root .xcp-slots.conf, so without this it would drop
# to the stale "main" default below and fail with "no route to host".
SLOTS_CONF="${MCNF_SLOTS_CONF:-}"
if [ -z "$SLOTS_CONF" ]; then
  for _c in "$REPO/.xcp-slots.conf" "$HOME/.xcp-slots.conf"; do
    [ -f "$_c" ] && { SLOTS_CONF="$_c"; break; }
  done
  SLOTS_CONF="${SLOTS_CONF:-$REPO/.xcp-slots.conf}"
fi
RESULTS_DIR="$REPO/.xcp-build/results"
SSH_BASE=(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o BatchMode=yes)
log() { echo "==> xcp-build: $*" >&2; }
die() { echo "!! xcp-build: $*" >&2; exit 1; }

# ---- slot registry --------------------------------------------------------
# Populate SLOT_NAMES + the per-slot maps from .xcp-slots.conf, else a single
# env-derived back-compat slot named "main".
declare -A SLOT_HOST SLOT_USER SLOT_KEY SLOT_DIR
SLOT_NAMES=()
load_slots() {
  if [ -f "$SLOTS_CONF" ]; then
    while read -r name host user key dir _rest; do
      [ -z "${name:-}" ] && continue
      case "$name" in \#*) continue;; esac
      SLOT_NAMES+=("$name")
      SLOT_HOST["$name"]="$host"; SLOT_USER["$name"]="$user"
      SLOT_KEY["$name"]="${key/#\~/$HOME}"; SLOT_DIR["$name"]="$dir"
    done < "$SLOTS_CONF"
  fi
  if [ ${#SLOT_NAMES[@]} -eq 0 ]; then
    SLOT_NAMES=(main)
    SLOT_HOST[main]="${MCNF_BUILD_HOST:-172.20.0.50}"
    SLOT_USER[main]="${MCNF_BUILD_USER:-mm}"
    SLOT_KEY[main]="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
    SLOT_DIR[main]="${MCNF_BUILD_DIR:-magic-mesh}"
  fi
}

# Resolve the active slot from --slot S / $MCNF_BUILD_SLOT / the first registered.
SLOT=""
resolve_slot() {
  local want="${1:-${MCNF_BUILD_SLOT:-}}"
  if [ -n "$want" ]; then
    for n in "${SLOT_NAMES[@]}"; do [ "$n" = "$want" ] && { SLOT="$want"; return; }; done
    die "no such slot '$want' (have: ${SLOT_NAMES[*]})"
  fi
  SLOT="${SLOT_NAMES[0]}"
}

# Pull --slot S / --no-sync out of an arg list, leaving the rest in REST[].
REST=(); SLOT_ARG=""; NOSYNC=0
parse_slot_flag() {
  REST=(); NOSYNC=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --slot) SLOT_ARG="$2"; shift 2;;
      --slot=*) SLOT_ARG="${1#--slot=}"; shift;;
      --no-sync) NOSYNC=1; shift;;
      *) REST+=("$1"); shift;;
    esac
  done
}
maybe_sync() { [ "$NOSYNC" -eq 1 ] || do_sync; }

ssh_to() { "${SSH_BASE[@]}" -i "${SLOT_KEY[$SLOT]}" "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}" "$@"; }
# Run a command in the slot's remote repo with the cargo env + workspace config.
remote() { ssh_to "source \$HOME/.cargo/env 2>/dev/null; cd ${SLOT_DIR[$SLOT]} && $*"; }

do_sync() {
  command -v rsync >/dev/null || die "rsync missing on the dev host (dnf install -y rsync)"
  log "[$SLOT] rsync → ${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}:${SLOT_DIR[$SLOT]} (excl target*/)"
  rsync -az --delete \
    -e "${SSH_BASE[*]} -i ${SLOT_KEY[$SLOT]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.xcp-build' --exclude '/.git/objects/pack/tmp_*' \
    "$REPO/" "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}:${SLOT_DIR[$SLOT]}/"
}

# Map a gate name → the remote command that runs it.
gate_cmd() {
  case "$1" in
    fmt)       echo "cargo fmt --all --check";;
    clippy)    echo "cargo clippy --all-targets";;
    test)      echo "cargo test --workspace --exclude mackesd && cargo test -p mackesd -- --test-threads=1";;
    check)     echo "cargo check --workspace";;
    build)     echo "cargo build --workspace";;
    boundary)  echo "./install-helpers/lint-mesh-boundary.sh";;
    carbon)    echo "./install-helpers/lint-carbon-tokens.sh";;
    bus)       echo "./install-helpers/lint-bus-names.sh";;
    libcosmic) echo "./install-helpers/lint-libcosmic-rev.sh";;
    *) return 1;;
  esac
}

# JSON-encode a string safely (python3 if present, else a crude fallback).
json_str() {
  if command -v python3 >/dev/null; then python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'
  else sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/ /g' | tr '\n' ' ' | sed 's/^/"/; s/$/"/'; fi
}

# Run one or more gates on the active slot, timing each, capturing the tail of a
# failing step, and writing a structured result JSON. Returns nonzero on any fail.
RESULT_FILE=""
run_gates() {
  local label="$1"; shift
  local gates=("$@")
  mkdir -p "$RESULTS_DIR"
  local epoch; epoch="$(date +%s)"
  RESULT_FILE="$RESULTS_DIR/${SLOT}-${epoch}-${label}.json"
  local started; started="$(date -u +%FT%TZ)"
  local overall=0 steps_json="" first=1 err_tail=""
  local t0_all; t0_all="$(date +%s)"
  for g in "${gates[@]}"; do
    local cmd; cmd="$(gate_cmd "$g")" || { log "unknown gate '$g'"; overall=1; continue; }
    log "[$SLOT] gate: $g  ($cmd)"
    local t0; t0="$(date +%s)"
    local out; out="$(remote "$cmd" 2>&1)"; local rc=$?
    local dt=$(( $(date +%s) - t0 ))
    local ok=true; [ $rc -ne 0 ] && { ok=false; overall=1; err_tail="$(printf '%s\n' "$out" | tail -40)"; }
    [ $first -eq 1 ] && first=0 || steps_json+=","
    steps_json+="{\"name\":\"$g\",\"ok\":$ok,\"duration_s\":$dt,\"exit\":$rc}"
    printf '%s\n' "$out" | tail -5 >&2
    log "[$SLOT] gate $g: $([ "$ok" = true ] && echo PASS || echo FAIL) (${dt}s)"
  done
  local dt_all=$(( $(date +%s) - t0_all ))
  local ok_all=true; [ $overall -ne 0 ] && ok_all=false
  {
    printf '{"slot":"%s","host":"%s","gate":"%s","ok":%s,"started":"%s","duration_s":%s,"steps":[%s],"error_tail":' \
      "$SLOT" "${SLOT_HOST[$SLOT]}" "$label" "$ok_all" "$started" "$dt_all" "$steps_json"
    printf '%s' "$err_tail" | json_str
    printf '}\n'
  } > "$RESULT_FILE"
  log "[$SLOT] result → ${RESULT_FILE#$REPO/}  (overall: $([ $overall -eq 0 ] && echo PASS || echo FAIL))"
  return $overall
}

# ---- dispatch -------------------------------------------------------------
load_slots
CMD="${1:-}"; shift || true

case "$CMD" in
  slots)
    printf '%-8s %-16s %-6s %-14s %s\n' SLOT HOST USER REMOTE_DIR REACHABLE
    for n in "${SLOT_NAMES[@]}"; do
      SLOT="$n"
      r="no"; ssh_to 'echo ok' >/dev/null 2>&1 && r="yes"
      printf '%-8s %-16s %-6s %-14s %s\n' "$n" "${SLOT_HOST[$n]}" "${SLOT_USER[$n]}" "${SLOT_DIR[$n]}" "$r"
    done
    ;;

  sync)   parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"; do_sync ;;

  cargo)  parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"; do_sync; remote "cargo ${REST[*]}" ;;

  gate)   parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          [ ${#REST[@]} -ge 1 ] || die "gate needs a name (fmt|clippy|test|check|build|boundary|carbon|bus|libcosmic)"
          maybe_sync; run_gates "${REST[0]}" "${REST[0]}" ;;

  gates)  parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          maybe_sync; run_gates gates fmt clippy test boundary carbon ;;

  crate)  parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          [ ${#REST[@]} -ge 2 ] || die "crate needs <pkg> <gate>"
          pkg="${REST[0]}"; g="${REST[1]}"
          case "$g" in check|test|clippy|build) c="cargo $g -p $pkg";; *) die "crate gate must be check|test|clippy|build";; esac
          maybe_sync
          mkdir -p "$RESULTS_DIR"; epoch="$(date +%s)"
          RESULT_FILE="$RESULTS_DIR/${SLOT}-${epoch}-${pkg}-${g}.json"
          t0="$(date +%s)"; out="$(remote "$c" 2>&1)"; rc=$?; dt=$(( $(date +%s) - t0 ))
          ok=true; [ $rc -ne 0 ] && ok=false
          { printf '{"slot":"%s","host":"%s","gate":"%s","crate":"%s","ok":%s,"duration_s":%s,"error_tail":' \
              "$SLOT" "${SLOT_HOST[$SLOT]}" "$g" "$pkg" "$ok" "$dt"
            printf '%s' "$([ $rc -ne 0 ] && printf '%s\n' "$out" | tail -40)" | json_str; printf '}\n'; } > "$RESULT_FILE"
          printf '%s\n' "$out" | tail -8 >&2
          log "[$SLOT] $pkg $g: $([ $rc -eq 0 ] && echo PASS || echo FAIL) (${dt}s) → ${RESULT_FILE#$REPO/}"
          exit $rc ;;

  render) # build + headless-render a GUI on the slot, pull the PNG back local.
          # Uses install-helpers/preview-capture.sh (sway headless + grim, pixman
          # software render). Default surface = mde-workbench; --focus <slug>.
          parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          slug="${REST[0]:-}"; outarg="${REST[1]:-}"
          maybe_sync
          log "[$SLOT] building mde-workbench for render"
          remote "cargo build -p mde-workbench" || die "workbench build failed"
          ts="$(date +%s)"; rel="${slug:-home}"; rel="${rel//./_}"
          log "[$SLOT] capturing slug='${slug:-<home>}'"
          remote "./install-helpers/preview-capture.sh '$slug' /tmp/mde-render-$ts.png" \
            || die "render/capture failed on $SLOT (need sway+grim+mesa — run: xcp-slots.sh bootstrap $SLOT)"
          mkdir -p "$REPO/.xcp-build/renders"
          local_out="${outarg:-$REPO/.xcp-build/renders/${SLOT}-${ts}-${rel}.png}"
          rsync -az -e "${SSH_BASE[*]} -i ${SLOT_KEY[$SLOT]}" \
            "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}:/tmp/mde-render-$ts.png" "$local_out" \
            && log "[$SLOT] render → ${local_out#$REPO/}" || die "could not pull the render PNG"
          ;;

  rpm)    # back-compat release path (single slot)
          resolve_slot "${MCNF_BUILD_SLOT:-}"; do_sync
          log "[$SLOT] release build + generate-rpm (heavy — on XCP)"
          remote "cargo build --workspace --release && cargo generate-rpm -p crates/mesh/mackesd" || die "rpm build failed"
          ART="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"; mkdir -p "$ART"
          rsync -az -e "${SSH_BASE[*]} -i ${SLOT_KEY[$SLOT]}" \
            "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}:${SLOT_DIR[$SLOT]}/target/generate-rpm/*.rpm" "$ART/"
          ls -la "$ART"/*.rpm ;;

  pull)   parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          [ ${#REST[@]} -ge 1 ] || die "pull needs a remote glob"
          ART="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"; mkdir -p "$ART"
          rsync -az -e "${SSH_BASE[*]} -i ${SLOT_KEY[$SLOT]}" \
            "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}:${SLOT_DIR[$SLOT]}/${REST[0]}" "$ART/" ;;

  shell)  parse_slot_flag "$@"; resolve_slot "$SLOT_ARG"
          exec "${SSH_BASE[@]}" -o BatchMode=no -i "${SLOT_KEY[$SLOT]}" "${SLOT_USER[$SLOT]}@${SLOT_HOST[$SLOT]}" ;;

  result) f="${1:-latest}"
          if [ "$f" = latest ]; then f="$(ls -1t "$RESULTS_DIR"/*.json 2>/dev/null | head -1)"; fi
          [ -n "$f" ] && [ -f "$f" ] || die "no result file"
          if command -v python3 >/dev/null; then python3 -m json.tool "$f"; else cat "$f"; fi ;;

  ""|-h|--help|help) sed -n '21,45p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) die "unknown command '$CMD' (try: slots sync cargo gate gates crate rpm pull shell result)";;
esac
