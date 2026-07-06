#!/usr/bin/env bash
# worktree-guard.sh — DRAIN-7 STEP-0 for mutating agents.
#
# Agents must run from an isolated worktree, not the shared checkout. The default
# allowed root is <repo>/.agents/worktrees; override with MCNF_AGENT_WORKTREES.
set -euo pipefail

REPO="${MCNF_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
ALLOWED_ROOT="${MCNF_AGENT_WORKTREES:-$REPO/.agents/worktrees}"

usage() { sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; }
die() { echo "worktree-guard: $*" >&2; exit 97; }

canon() {
  local path="$1"
  mkdir -p "$path"
  (cd "$path" && pwd -P)
}

check_worktree() {
  local top allowed
  top="$(git rev-parse --show-toplevel 2>/dev/null)" || die "not inside a git worktree"
  top="$(cd "$top" && pwd -P)"
  allowed="$(canon "$ALLOWED_ROOT")"
  case "$top/" in
    "$allowed"/*) ;;
    *) die "refusing shared checkout: $top (expected isolated worktree under $allowed)" ;;
  esac
  [ "$top" != "$(cd "$REPO" && pwd -P)" ] || die "refusing repository root: $top"
  printf 'worktree ok: %s\n' "$top"
}

self_test() {
  local td repo wt self
  self="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")"
  td="$(mktemp -d)"
  trap 'rm -rf "$td"' RETURN
  repo="$td/repo"; wt="$td/agents/one"
  git init -q "$repo"
  git -C "$repo" config user.email test@example.invalid
  git -C "$repo" config user.name Test
  printf x > "$repo/file"
  git -C "$repo" add file
  git -C "$repo" commit -qm init
  mkdir -p "$td/agents"
  git -C "$repo" worktree add -q "$wt"
  (cd "$wt" && MCNF_REPO="$repo" MCNF_AGENT_WORKTREES="$td/agents" "$self" >/dev/null)
  if (cd "$repo" && MCNF_REPO="$repo" MCNF_AGENT_WORKTREES="$td/agents" "$self" >/dev/null 2>&1); then
    echo "worktree-guard: shared checkout unexpectedly accepted" >&2
    return 1
  fi
  echo "worktree-guard: self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) usage ;;
  '') check_worktree ;;
  *) die "unknown arg: $1" ;;
esac
