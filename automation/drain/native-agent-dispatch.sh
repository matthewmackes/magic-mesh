#!/usr/bin/env bash
# native-agent-dispatch.sh — tool-preserving DRAIN-7 adapter.
#
# Called by agent-dispatch.sh with one worklist queue unit. It creates an
# isolated detached worktree and starts the SAME agent runtime that invoked the
# drain. The parent coordinator remains non-blocking; the child owns only its
# worktree and must not commit or push.
set -euo pipefail

REPO="${MCNF_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
ROOT="${MCNF_AGENT_WORKTREE_ROOT:-${TMPDIR:-/tmp}/mcnf-drain-worktrees}"
LIFECYCLE="${MCNF_AGENT_LIFECYCLE:-$(cd "$(dirname "$0")" && pwd)/agent-lifecycle.sh}"
RUNTIME=""
JOB=""
EPIC=""
COMMAND=""

self_test() {
  local td repo wt root
  td="$(mktemp -d "${TMPDIR:-/tmp}/native-agent-dispatch.XXXXXX")"
  trap 'rm -rf "$td"' RETURN
  repo="$td/repo"
  root="$td/worktrees"
  mkdir -p "$repo"
  git -C "$repo" init -q
  git -C "$repo" config user.email luna@example.invalid
  git -C "$repo" config user.name Luna
  printf 'x\n' > "$repo/file.txt"
  git -C "$repo" add file.txt
  git -C "$repo" commit -qm init
  MCNF_REPO="$repo" MCNF_AGENT_WORKTREE_ROOT="$root" \
    "$0" --runtime cursor --job-id 0123456789ab --epic WL-TEST-001 --command 'cargo test -p mde-bus' >/dev/null 2>&1 || true
  [[ -d "$root/cursor-WL-TEST-001-0123456789ab" ]] || { echo "native-agent-dispatch: self-test failed (worktree missing)" >&2; exit 1; }
  echo "native-agent-dispatch: self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit 0
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runtime) RUNTIME="${2:?runtime}"; shift 2 ;;
    --job-id) JOB="${2:?job-id}"; shift 2 ;;
    --epic) EPIC="${2:?epic}"; shift 2 ;;
    --command) COMMAND="${2:?command}"; shift 2 ;;
    *) echo "native-agent-dispatch: unknown argument $1" >&2; exit 2 ;;
  esac
done

case "$RUNTIME" in
  cursor|codex|claude) ;;
  *) echo "native-agent-dispatch: unsupported runtime '$RUNTIME'" >&2; exit 2 ;;
esac
[[ "$JOB" =~ ^[0-9a-f]{12}$ ]] || { echo "native-agent-dispatch: invalid job id" >&2; exit 2; }
[[ "$COMMAND" == cargo\ * ]] || { echo "native-agent-dispatch: non-cargo command" >&2; exit 2; }

mkdir -p "$ROOT"
WORKTREE="$ROOT/$RUNTIME-$EPIC-$JOB"
LOG="$WORKTREE/agent.log"
if [[ -e "$WORKTREE" ]]; then
  if [[ -f "$WORKTREE/.agent-state" ]] &&
      [[ "$(awk -F= '$1 == "pid" { print $2; exit }' "$WORKTREE/.agent-state")" =~ ^[0-9]+$ ]] &&
      kill -0 "$(awk -F= '$1 == "pid" { print $2; exit }' "$WORKTREE/.agent-state")" 2>/dev/null; then
    echo "native-agent-dispatch: job already running: $WORKTREE" >&2
    exit 0
  fi
  echo "native-agent-dispatch: stale worktree: $WORKTREE" >&2
  exit 75
fi
git -C "$REPO" worktree add --detach "$WORKTREE" HEAD >/dev/null
printf 'job_id=%s\nepic=%s\nruntime=%s\ncommand=%s\nstatus=dispatching\npid=\nstarted_at=%s\nheartbeat_at=%s\n' \
  "$JOB" "$EPIC" "$RUNTIME" "$COMMAND" "$(date +%s)" "$(date +%s)" >"$WORKTREE/.agent-state"

PROMPT="$(cat <<EOF
You are the $RUNTIME implementation agent for worklist unit $JOB ($EPIC).
Work only in this isolated worktree: $WORKTREE.
Read AGENTS.md, AI_GOVERNANCE.md §10.0.4, and the exact $EPIC entry in
docs/platform/WORKLIST.md. This unit's farm verification command is:
  $COMMAND
Choose a concrete, real implementation or required evidence slice from that
epic, with a disjoint file/write scope. Do not invent busywork or redundant
tests. Do not touch files owned by concurrent workers, do not commit, and do
not push; leave the worktree diff for the parent coordinator to review and
fold. Run only the required focused farm verification and local fmt. Report
the exact scope, evidence, and any blocker; park ENOSPC rather than retrying.
EOF
)"

case "$RUNTIME" in
  cursor)
    command -v cursor-agent >/dev/null || { echo "cursor-agent missing" >&2; exit 2; }
    nohup cursor-agent --print --trust --workspace "$WORKTREE" "$PROMPT" \
      >"$LOG" 2>&1 &
    ;;
  codex)
    command -v codex >/dev/null || { echo "codex missing" >&2; exit 2; }
    nohup codex exec --cd "$WORKTREE" --sandbox workspace-write \
      --approve-for-me "$PROMPT" >"$LOG" 2>&1 &
    ;;
  claude)
    command -v claude >/dev/null || { echo "claude missing" >&2; exit 2; }
    nohup bash -c 'cd "$1" && exec claude -p --permission-mode acceptEdits \
      --add-dir "$1" "$2"' bash "$WORKTREE" "$PROMPT" >"$LOG" 2>&1 &
    ;;
esac
printf '%s\n' "$!" > "$WORKTREE/.agent-pid"
agent_pid="$!"
touch "$WORKTREE/.agent-heartbeat"
(
  while kill -0 "$agent_pid" 2>/dev/null; do
    touch "$WORKTREE/.agent-heartbeat"
    sleep "${MCNF_AGENT_HEARTBEAT_SECS:-30}"
  done
) >/dev/null 2>&1 &
awk -F= -v pid="$agent_pid" -v heartbeat="$(date +%s)" '
  $1 == "pid" { print "pid=" pid; next }
  $1 == "status" { print "status=running"; next }
  $1 == "heartbeat_at" { print "heartbeat_at=" heartbeat; next }
  { print }
' "$WORKTREE/.agent-state" >"$WORKTREE/.agent-state.tmp"
mv "$WORKTREE/.agent-state.tmp" "$WORKTREE/.agent-state"
printf 'native-agent-dispatch: started runtime=%s job=%s worktree=%s pid=%s\n' \
  "$RUNTIME" "$JOB" "$WORKTREE" "$agent_pid"
