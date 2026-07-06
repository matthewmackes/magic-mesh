#!/usr/bin/env bash
# park-worklist-item.sh — DRAIN-5: mark a blocked worklist unit and keep moving.
#
# Usage:
#   park-worklist-item.sh TASK-ID "operator/live-infra blocker reason"
#   park-worklist-item.sh --self-test
#
# The script edits exactly two durable tracker files:
#   * docs/WORKLIST.md: [ ]/[>] TASK-ID -> [!]
#   * docs/NEEDS-OPERATOR.md: appends an idempotent blocker entry
set -euo pipefail

REPO="${MCNF_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
WORKLIST="${MCNF_WORKLIST:-$REPO/docs/WORKLIST.md}"
NEEDS="${MCNF_NEEDS_OPERATOR:-$REPO/docs/NEEDS-OPERATOR.md}"
DATE_="${MCNF_PARK_DATE:-$(date -u +%Y-%m-%d)}"

usage() { sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; }

die() { echo "park-worklist-item: $*" >&2; exit 2; }

task_line_re() {
  local task="$1"
  printf '^[[:space:]]*-[[:space:]]*\\[[ >]\\][[:space:]]*\\*\\*%s:' "$task"
}

park_task() {
  local task="${1:?task id}" reason="${2:?blocker reason}"
  [ -f "$WORKLIST" ] || die "missing worklist: $WORKLIST"
  mkdir -p "$(dirname "$NEEDS")"
  [ -f "$NEEDS" ] || printf '# Needs-Operator\n\n' > "$NEEDS"

  if ! grep -Eq "$(task_line_re "$task")" "$WORKLIST"; then
    if grep -Eq "^[[:space:]]*-[[:space:]]*\\[!\\][[:space:]]*\\*\\*$task:" "$WORKLIST"; then
      :
    else
      die "task is not open/in-progress in $WORKLIST: $task"
    fi
  fi

  local tmp
  tmp="$(mktemp)"
  awk -v task="$task" '
    $0 ~ "^[[:space:]]*-[[:space:]]*\\[[ >]\\][[:space:]]*\\*\\*" task ":" {
      sub(/\[[ >]\]/, "[!]")
    }
    { print }
  ' "$WORKLIST" > "$tmp"
  cat "$tmp" > "$WORKLIST"
  rm -f "$tmp"

  if ! grep -Eq "^- \\*\\*$task\\*\\* — " "$NEEDS"; then
    {
      printf '\n## Parked by drain coordinator (%s)\n\n' "$DATE_"
      printf -- '- **%s** — %s\n' "$task" "$reason"
    } >> "$NEEDS"
  fi
  printf 'parked %s: %s\n' "$task" "$reason"
}

self_test() {
  local td out self
  self="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")"
  td="$(mktemp -d)"
  trap 'rm -rf "$td"' RETURN
  cat > "$td/WORKLIST.md" <<'EOF'
# Worklist
- [ ] **DRAIN-X: open thing.**
- [>] **DRAIN-Y: active thing.**
- [✓] **DRAIN-Z: done thing.**
EOF
  cat > "$td/NEEDS.md" <<'EOF'
# Needs-Operator
EOF
  MCNF_WORKLIST="$td/WORKLIST.md" MCNF_NEEDS_OPERATOR="$td/NEEDS.md" MCNF_PARK_DATE=2026-07-05 "$self" DRAIN-X "needs an operator action" >/dev/null
  MCNF_WORKLIST="$td/WORKLIST.md" MCNF_NEEDS_OPERATOR="$td/NEEDS.md" MCNF_PARK_DATE=2026-07-05 "$self" DRAIN-X "needs an operator action" >/dev/null
  out="$(grep -c '^- \*\*DRAIN-X\*\*' "$td/NEEDS.md")"
  grep -q '^- \[!\] \*\*DRAIN-X:' "$td/WORKLIST.md"
  grep -q '^- \[>\] \*\*DRAIN-Y:' "$td/WORKLIST.md"
  [ "$out" = "1" ]
  echo "park-worklist-item: self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help|'') usage ;;
  *) [ $# -ge 2 ] || die "usage: park-worklist-item.sh TASK-ID REASON"; park_task "$1" "$2" ;;
esac
