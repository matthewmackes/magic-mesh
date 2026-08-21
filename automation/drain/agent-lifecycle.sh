#!/usr/bin/env bash
# agent-lifecycle.sh — durable lifecycle accounting for native implementation agents.
#
# Worktree existence is not liveness. The native adapter writes a metadata file;
# this manager reconciles PID, heartbeat, and terminal state without deleting
# worktrees or diffs.
set -euo pipefail

ROOT="${MCNF_AGENT_WORKTREE_ROOT:-${TMPDIR:-/tmp}/mcnf-drain-worktrees}"
STALE_SECS="${MCNF_AGENT_STALE_SECS:-3600}"

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
  heartbeat="$(read_field "$meta" heartbeat_at)"
  if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" 2>/dev/null; then
    printf 'running\n'
    return
  fi
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

reap_cmd() {
  local meta status
  shopt -s nullglob
  local -a metas=("$ROOT"/*/.agent-state)
  for meta in "${metas[@]}"; do
    status="$(status_for "$meta")"
    case "$status" in
      running|completed|failed|blocked|salvaged|requeued) continue ;;
      stale)
        write_field "$meta" status stale
        write_field "$meta" stale_at "$(now)"
        printf 'agent-lifecycle: stale job=%s worktree=%s\n' \
          "$(read_field "$meta" job_id)" "$(dirname "$meta")"
        ;;
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
  printf 'job_id=test12345678\nstatus=dispatching\npid=999999\nheartbeat_at=1\n' >"$meta"
  MCNF_AGENT_WORKTREE_ROOT="$td" MCNF_AGENT_STALE_SECS=1 "$0" reap >/dev/null
  grep -q '^status=stale$' "$meta" || die "stale state was not recorded"
  MCNF_AGENT_WORKTREE_ROOT="$td" MCNF_AGENT_SALVAGE_ROOT="$td/salvage" \
    "$0" salvage test12345678 >/dev/null
  compgen -G "$td/salvage/test12345678-*/worktree.diff" >/dev/null ||
    die "salvage archive missing"
  printf 'agent-lifecycle: self-test passed\n'
}

case "${1:-status}" in
  status) status_cmd ;;
  reap) reap_cmd ;;
  salvage) shift; salvage_cmd "${1:-}" ;;
  requeue) shift; requeue_cmd "${1:-}" ;;
  --self-test) self_test ;;
  *) die "usage: $0 {status|reap|--self-test}" ;;
esac
