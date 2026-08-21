#!/usr/bin/env bash
# agent-lifecycle.sh — durable lifecycle accounting for native implementation agents.
#
# Worktree existence is not liveness. The native adapter writes a metadata file;
# this manager reconciles PID, heartbeat, and terminal state without deleting
# worktrees or diffs.
set -euo pipefail

ROOT="${MCNF_AGENT_WORKTREE_ROOT:-${TMPDIR:-/tmp}/mcnf-drain-worktrees}"
STALE_SECS="${MCNF_AGENT_STALE_SECS:-3600}"
REPO="${MCNF_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
SALVAGE_ROOT="${MCNF_AGENT_SALVAGE_ROOT:-${TMPDIR:-/tmp}/mcnf-drain-salvage}"

die() { printf 'agent-lifecycle: %s\n' "$*" >&2; exit 2; }
now() { date +%s; }

read_field() {
  local file="$1" key="$2"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$file"
}

write_field() {
  local file="$1" key="$2" value="$3" tmp
  tmp="${file}.tmp.$$"
  awk -F= -v key="$key" -v value="$value" '
    $1 == key { print key "=" value; found=1; next }
    { print }
    END { if (!found) print key "=" value }
  ' "$file" >"$tmp"
  mv "$tmp" "$file"
}

status_for() {
  local meta="$1" worktree pid state heartbeat age
  worktree="$(dirname "$meta")"
  state="$(read_field "$meta" status)"
  case "$state" in
    completed|failed|blocked|salvaged|requeued) printf '%s\n' "$state"; return ;;
  esac
  pid="$(read_field "$meta" pid)"
  if [[ "$pid" =~ ^[0-9]+$ ]]; then
    if kill -0 "$pid" 2>/dev/null; then
      printf 'running\n'
    else
      # A recorded PID that is gone exited/crashed without a terminal record.
      # Its worktree is residue, not live work — stale immediately, no wait.
      printf 'stale\n'
    fi
    return
  fi
  # No PID recorded yet: still inside the launch window. Fall back to the
  # heartbeat clock so a launch that dies before recording a PID still ages out.
  heartbeat="$(read_field "$meta" heartbeat_at)"
  if [[ -f "$worktree/.agent-heartbeat" ]]; then
    heartbeat="$(stat -c %Y "$worktree/.agent-heartbeat" 2>/dev/null || printf '%s\n' "$heartbeat")"
  fi
  age=$(( $(now) - ${heartbeat:-0} ))
  if (( age > STALE_SECS )); then
    printf 'stale\n'
  else
    printf 'dispatching\n'
  fi
}

status_cmd() {
  local meta status
  shopt -s nullglob
  local -a metas=("$ROOT"/*/.agent-state)
  printf 'job_id\tstatus\tpid\tworktree\theartbeat_at\n'
  for meta in "${metas[@]}"; do
    status="$(status_for "$meta")"
    printf '%s\t%s\t%s\t%s\t%s\n' \
      "$(read_field "$meta" job_id)" "$status" "$(read_field "$meta" pid)" \
      "$(dirname "$meta")" "$(read_field "$meta" heartbeat_at)"
  done
}

archive_worktree() {
  # Preserve a stale/abandoned worktree's diff, log, and metadata, then remove
  # the worktree so the unit can be dispatched fresh. Prints the archive path.
  local meta="$1" worktree job archive
  worktree="$(dirname "$meta")"
  job="$(read_field "$meta" job_id)"
  archive="$SALVAGE_ROOT/${job:-unknown}-$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$archive"
  git -C "$worktree" diff --binary >"$archive/worktree.diff" 2>/dev/null || true
  [ -s "$archive/worktree.diff" ] || rm -f "$archive/worktree.diff"
  [[ ! -f "$worktree/agent.log" ]] || cp "$worktree/agent.log" "$archive/agent.log" 2>/dev/null || true
  cp "$meta" "$archive/agent-state" 2>/dev/null || true
  git -C "$REPO" worktree remove --force "$worktree" 2>/dev/null || rm -rf "$worktree"
  git -C "$REPO" worktree prune 2>/dev/null || true
  printf '%s\n' "$archive"
}

reap_cmd() {
  # Salvage and clear every stale worktree so a dead agent never blocks the
  # next dispatch of its unit (native-agent-dispatch exits 75 on residue).
  local meta status job archive
  shopt -s nullglob
  local -a metas=("$ROOT"/*/.agent-state)
  for meta in "${metas[@]}"; do
    status="$(status_for "$meta")"
    case "$status" in
      stale)
        job="$(read_field "$meta" job_id)"
        archive="$(archive_worktree "$meta")"
        printf 'agent-lifecycle: reaped stale job=%s -> %s\n' "${job:-unknown}" "$archive"
        ;;
      *) continue ;;
    esac
  done
}

meta_for_job() {
  local wanted="$1" meta
  shopt -s nullglob
  for meta in "$ROOT"/*/.agent-state; do
    [[ "$(read_field "$meta" job_id)" == "$wanted" ]] && {
      printf '%s\n' "$meta"
      return 0
    }
  done
  return 1
}

salvage_cmd() {
  local job="${1:-}" meta worktree archive
  [[ -n "$job" ]] || die "salvage requires a job id"
  meta="$(meta_for_job "$job")" || die "unknown job id $job"
  worktree="$(dirname "$meta")"
  archive="${MCNF_AGENT_SALVAGE_ROOT:-${TMPDIR:-/tmp}/mcnf-drain-salvage}/$job-$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$archive"
  git -C "$worktree" diff --binary >"$archive/worktree.diff"
  [[ ! -f "$worktree/agent.log" ]] || cp "$worktree/agent.log" "$archive/agent.log"
  cp "$meta" "$archive/agent-state"
  write_field "$meta" status salvaged
  write_field "$meta" salvaged_at "$(now)"
  printf 'agent-lifecycle: salvaged job=%s archive=%s\n' "$job" "$archive"
}

requeue_cmd() {
  local job="${1:-}" meta
  [[ -n "$job" ]] || die "requeue requires a job id"
  meta="$(meta_for_job "$job")" || die "unknown job id $job"
  write_field "$meta" status requeued
  write_field "$meta" heartbeat_at "$(now)"
  printf 'agent-lifecycle: requeued job=%s\n' "$job"
}

self_test() {
  local td meta
  td="$(mktemp -d "${TMPDIR:-/tmp}/agent-lifecycle.XXXXXX")"
  trap 'rm -rf "$td"' RETURN
  mkdir -p "$td/worktree"
  git -C "$td/worktree" init -q
  git -C "$td/worktree" config user.email luna@example.invalid
  git -C "$td/worktree" config user.name Luna
  printf 'fixture\n' >"$td/worktree/file"
  git -C "$td/worktree" add file
  git -C "$td/worktree" commit -qm fixture
  meta="$td/worktree/.agent-state"
  printf 'job_id=test12345678\nstatus=running\npid=999999\nheartbeat_at=%s\n' "$(now)" >"$meta"
  # A recorded-but-dead PID is stale IMMEDIATELY, even with a long stale window.
  local st
  st="$(MCNF_AGENT_WORKTREE_ROOT="$td" MCNF_AGENT_STALE_SECS=999999 "$0" status | awk -F'\t' 'NR==2{print $2}')"
  [[ "$st" == stale ]] || die "dead pid must be stale immediately, got '$st'"
  # A worktree with no pid yet inside the window is dispatching, not stale.
  printf 'x\n' >>"$td/worktree/file"; git -C "$td/worktree" add file; git -C "$td/worktree" commit -qm edit
  printf 'job_id=test12345678\nstatus=dispatching\npid=\nheartbeat_at=%s\n' "$(now)" >"$meta"
  st="$(MCNF_AGENT_WORKTREE_ROOT="$td" MCNF_AGENT_STALE_SECS=999999 "$0" status | awk -F'\t' 'NR==2{print $2}')"
  [[ "$st" == dispatching ]] || die "fresh no-pid launch must be dispatching, got '$st'"
  # Reap salvages the diff AND removes the worktree so redispatch can proceed.
  printf 'uncommitted agent work\n' >>"$td/worktree/file"
  printf 'job_id=test12345678\nstatus=running\npid=999999\nheartbeat_at=1\n' >"$meta"
  MCNF_REPO="$td/worktree" MCNF_AGENT_WORKTREE_ROOT="$td" MCNF_AGENT_SALVAGE_ROOT="$td/salvage" \
    "$0" reap >/dev/null
  [[ -e "$meta" ]] && die "reap must remove the stale worktree"
  compgen -G "$td/salvage/test12345678-*/worktree.diff" >/dev/null ||
    die "reap salvage archive missing"
  printf 'agent-lifecycle: self-test passed\n'
}

case "${1:-status}" in
  status) status_cmd ;;
  reap) reap_cmd ;;
  salvage) shift; salvage_cmd "${1:-}" ;;
  requeue) shift; requeue_cmd "${1:-}" ;;
  --self-test) self_test ;;
  *) die "usage: $0 {status|reap|salvage <job>|requeue <job>|--self-test}" ;;
esac
