#!/usr/bin/env bash
# farm-jobs.sh — the worklist binding (requirement A) shared by all 5 build-farm
# automation capabilities. Turns the human worklist into a machine-readable job
# list so a fleet-side automation (NOT an AI) can pick build work.
#
# CONVENTION: any worklist task line may carry a farm-build request:
#     - [>] **BUS-RETENTION-1: …**  @farm:{cargo build -p mde-bus}
# The text inside @farm:{ … } is the exact command run on a build VM. A task may
# carry more than one. Status comes from the task's checkbox:
#     [ ] open · [>] in-progress · [✓] done · [!] blocked
# Only OPEN + IN-PROGRESS tasks yield ACTIVE jobs (done/blocked are skipped).
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

WORKLIST="${MCNF_WORKLIST:-$(cd "$(dirname "$0")/../.." && pwd)/docs/WORKLIST.md}"

jobid() { printf '%s\037%s' "$1" "$2" | sha1sum | cut -c1-12; }

# Parse: emit "<jobid>\t<status>\t<task_id>\t<command>" for every @farm:{…}.
parse() {
  [ -f "$WORKLIST" ] || { echo "no worklist at $WORKLIST" >&2; return 1; }
  # Walk lines; remember the most recent task id + status, attach @farm jobs to it.
  local task_id="-" status="?"
  while IFS= read -r line; do
    # Task header line?  - [x] **PREFIX-N: title …
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
      [ -n "$cmd" ] && printf '%s\t%s\t%s\t%s\n' "$(jobid "$task_id" "$cmd")" "$status" "$task_id" "$cmd"
    done
  done < "$WORKLIST"
}

case "${1:-active}" in
  list)   parse ;;
  active) parse | awk -F'\t' '$2=="open"||$2=="prog"' ;;
  jobid)  jobid "${2:?task}" "${3:?cmd}" ;;
  -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: farm-jobs.sh list|active|jobid <task> <cmd>" >&2; exit 1 ;;
esac
