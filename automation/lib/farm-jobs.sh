#!/usr/bin/env bash
# farm-jobs.sh — the worklist binding (requirement A) shared by all 5 build-farm
# automation capabilities. Turns the human worklist into a machine-readable job
# list so a fleet-side automation (NOT an AI) can pick build work.
#
# CONVENTION: any active worklist item may carry a farm-build request:
#     ### WL-BUILD-002 - Farm shared cache
#     - Status: Remaining
#     - Verification method: @farm:{cargo build -p mde-bus}
# The text inside @farm:{ … } is the exact command run on a build VM. A task may
# carry more than one. Status comes from the reconciled worklist status:
#     Remaining -> open; Blocked / Needs clarification -> blocked
# Only OPEN tasks yield ACTIVE jobs (blocked/clarification-needed are skipped).
# The parser also understands the retired checkbox format used by old fixtures:
#     [ ] open · [>] in-progress · [✓] done · [!] blocked.
#
# A @farm:{…} payload counts as a REAL build job ONLY when its command is an actual
# build command — i.e. it begins with `cargo ` (cargo build/test/clippy/generate-rpm
# …). Documentation-only payloads that happen to use the @farm:{…} syntax are NOT
# commands and contribute ZERO demand:
#     @farm:{crate,verify}  — a TEMPLATE describing the tag format (not a command)
#     @farm:{…}             — a literal ellipsis placeholder
# is_build_command() (below) is the single gate; the parser drops anything it rejects
# so the demand count reflects only genuine `cargo …` work.
#
# Output (one job per line, tab-separated, stable job id = sha1 of status-blind
# key so a re-run of the same task+command is the same job):
#     <jobid>\t<status>\t<task_id>\t<command>
#
# Usage:
#   farm-jobs.sh list            all jobs (active + inactive), tab-separated
#   farm-jobs.sh active          only OPEN/IN-PROGRESS jobs (what to run)
#   farm-jobs.sh jobid <task> <cmd>   print the stable id for a task+command
set -uo pipefail

WORKLIST="${MCNF_WORKLIST:-$(cd "$(dirname "$0")/../.." && pwd)/docs/platform/WORKLIST.md}"

jobid() { printf '%s\037%s' "$1" "$2" | sha1sum | cut -c1-12; }

# is_build_command <payload> — PURE: true iff the @farm:{…} payload is a real build
# command (begins with `cargo ` after trimming leading whitespace). Rejects template/
# placeholder payloads like `crate,verify`, `…`, `...`, or anything not starting with
# the `cargo` build verb, so documentation that reuses the @farm:{…} syntax never
# inflates the queue. Tab/space leading whitespace is trimmed before the check.
is_build_command() {
  local c="$1"
  # Trim leading whitespace (spaces/tabs) so `@farm:{ cargo … }` still counts.
  c="${c#"${c%%[![:space:]]*}"}"
  case "$c" in
    cargo' '*) return 0 ;;   # `cargo build/test/clippy/generate-rpm …` — a real job
    *)         return 1 ;;   # template / placeholder / non-build payload — 0 demand
  esac
}

# Parse: emit "<jobid>\t<status>\t<task_id>\t<command>" for every @farm:{…}.
parse() {
  [ -f "$WORKLIST" ] || { echo "no worklist at $WORKLIST" >&2; return 1; }
  # Walk lines; remember the most recent task id + status, attach @farm jobs to it.
  local task_id="-" status="?"
  while IFS= read -r line; do
    # Reconciled task header?  ### WL-AREA-001 - title
    if [[ "$line" =~ ^###[[:space:]]+([A-Z][A-Za-z0-9._-]*)[[:space:]]+-[[:space:]]+ ]]; then
      task_id="${BASH_REMATCH[1]}"
      status="?"
    fi
    # Reconciled status line. Remaining is the only unblocked/runnable state.
    if [[ "$line" =~ ^[[:space:]]*-[[:space:]]Status:[[:space:]]*(Remaining|Blocked|Needs[[:space:]]clarification)[[:space:]]*$ ]]; then
      case "${BASH_REMATCH[1]}" in
        Remaining) status="open" ;;
        Blocked|Needs*) status="blocked" ;;
        *) status="?" ;;
      esac
    fi
    # Retired task header line?  - [x] **PREFIX-N: title …
    if [[ "$line" =~ ^[[:space:]]*-[[:space:]]*\[([ x>!✓])\][[:space:]]*\*\*([A-Z][A-Za-z0-9._-]*): ]]; then
      case "${BASH_REMATCH[1]}" in
        " ") status="open" ;;
        ">") status="prog" ;;
        "✓"|"x") status="done" ;;
        "!") status="blocked" ;;
        *) status="?" ;;
      esac
      task_id="${BASH_REMATCH[2]}"
    fi
    # Any @farm:{…} on this line (header or sub-bullet) attaches to the current task.
    local rest="$line"
    while [[ "$rest" == *'@farm:{'* ]]; do
      rest="${rest#*@farm:\{}"
      local cmd="${rest%%\}*}"
      rest="${rest#*\}}"
      # Count ONLY real build commands (cargo …); drop template/placeholder payloads
      # (crate,verify / … ) so documentation never inflates demand.
      is_build_command "$cmd" || continue
      printf '%s\t%s\t%s\t%s\n' "$(jobid "$task_id" "$cmd")" "$status" "$task_id" "$cmd"
    done
  done < "$WORKLIST"
}

# --self-test — pure-function assertions for the build-command gate (no worklist I/O).
self_test() {
  local fails=0
  check() { # check <label> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok: $1"
    else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1)); fi
  }
  bc() { is_build_command "$1" && echo yes || echo no; }   # build-command? yes/no
  echo "farm-jobs --self-test:"
  # Real build commands → counted.
  check "cargo build -p → yes"     "$(bc 'cargo build -p mde-bus')" yes
  check "cargo test -p → yes"      "$(bc 'cargo test -p mde-theme')" yes
  check "cargo clippy → yes"       "$(bc 'cargo clippy --workspace')" yes
  check "cargo generate-rpm → yes" "$(bc 'cargo generate-rpm -p crates/mesh/mackesd')" yes
  check "leading space + cargo → yes" "$(bc '  cargo build -p x')" yes
  # Template / placeholder / non-build payloads → NOT counted.
  check "crate,verify template → no" "$(bc 'crate,verify')" no
  check "ellipsis … placeholder → no" "$(bc '…')" no
  check "ascii ... placeholder → no"  "$(bc '...')" no
  check "empty payload → no"          "$(bc '')" no
  check "non-cargo verb → no"         "$(bc 'make all')" no
  check "cargo-prefixed non-word → no" "$(bc 'cargofoo build')" no
  local td wl
  td="$(mktemp -d "${TMPDIR:-/tmp}/farm-jobs-self.XXXXXX")" || return 1
  trap "rm -rf '$td'" EXIT
  wl="$td/WORKLIST.md"
  {
    printf '%s\n' '# Worklist'
    printf '%s\n' '### WL-BUILD-002 - Real job'
    printf '%s\n' '- Status: Remaining'
    printf '%s\n' '- Verification method: @farm:{cargo build -p mde-bus}'
    printf '%s\n' '### WL-CRIT-004 - Blocked job'
    printf '%s\n' '- Status: Blocked'
    printf '%s\n' '- Verification method: @farm:{cargo test -p mackesd}'
    printf '%s\n' '### WL-DOC-001 - Placeholder'
    printf '%s\n' '- Status: Remaining'
    printf '%s\n' '- Verification method: @farm:{crate,verify}'
  } >"$wl"
  check "new worklist active job count → 1" \
    "$(MCNF_WORKLIST="$wl" "$0" active | wc -l | tr -d ' ')" 1
  check "new worklist list job count → 2" \
    "$(MCNF_WORKLIST="$wl" "$0" list | wc -l | tr -d ' ')" 2
  check "new worklist task id preserved" \
    "$(MCNF_WORKLIST="$wl" "$0" active | awk -F'\t' 'NR==1 { print $3 }')" WL-BUILD-002
  if [ "$fails" -eq 0 ]; then echo "farm-jobs: self-test passed"; return 0; fi
  echo "farm-jobs: SELF-TEST FAILED ($fails)" >&2; return 1
}

case "${1:-active}" in
  list)   parse ;;
  active) parse | awk -F'\t' '$2=="open"||$2=="prog"' ;;
  jobid)  jobid "${2:?task}" "${3:?cmd}" ;;
  --self-test) self_test ;;
  -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: farm-jobs.sh list|active|jobid <task> <cmd>|--self-test" >&2; exit 1 ;;
esac
