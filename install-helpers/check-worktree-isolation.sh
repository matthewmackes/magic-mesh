#!/usr/bin/env bash
# check-worktree-isolation.sh — DRAIN-7: STEP-0 guard that an autonomous agent is
# operating in its OWN isolated git worktree, NOT a shared checkout.
#
# Why: a subagent once strayed into the shared `calm-ray-dcr8` worktree and its
# work was wiped (2026-06-24). Agents must touch ONLY their assigned isolated
# worktree. This guard makes that mechanically checkable (and refusable) at STEP 0
# instead of relying on the agent reading the rule.
#
# The layout (this repo): the MAIN checkout is /root/magic-mesh; linked agent
# worktrees live under <repo>/.claude/worktrees/<name>. Two of those are SHARED /
# off-limits — `bright-elm-ajw0` (the coordinator's main working copy) and
# `calm-ray-dcr8` (the live autoscaler's). An isolated agent worktree is any
# OTHER `.claude/worktrees/<name>`.
#
# Verdict (the `classify_worktree_path` pure rule):
#   - path basename ∈ SHARED names                  → shared   (REFUSE, rc 1)
#   - path under `/.claude/worktrees/<other>`        → isolated (OK,     rc 0)
#   - anything else (the main /root/magic-mesh root) → main     (REFUSE, rc 1)
#
# Use as a STEP-0 guard (two equivalent forms):
#   ./install-helpers/check-worktree-isolation.sh        # check $PWD, rc!=0 = stop
#   source install-helpers/check-worktree-isolation.sh && require_isolated_worktree
#
# Override SHARED names via MCNF_SHARED_WORKTREES (space-separated) if the set of
# off-limits worktrees changes.
#
# Modes (when executed, not sourced):
#   check-worktree-isolation.sh [dir]   classify dir (default $PWD); rc 0 iff isolated
#   check-worktree-isolation.sh --self-test
#   check-worktree-isolation.sh -h | --help

# The off-limits worktree basenames. Operator-overridable.
MCNF_SHARED_WORKTREES="${MCNF_SHARED_WORKTREES:-bright-elm-ajw0 calm-ray-dcr8}"

# classify_worktree_path <toplevel-path> [shared-names] — PURE: no git, no I/O.
# Echoes "isolated:<base>" / "shared:<base>" / "main:<path>". rc 0 ONLY for
# isolated (so callers can `classify… && proceed`). Exercised by --self-test.
classify_worktree_path() {
  local top="$1" shared="${2:-$MCNF_SHARED_WORKTREES}" base name
  # Normalise: drop any trailing slash so basename is stable.
  case "$top" in */) top="${top%/}";; esac
  base="${top##*/}"
  # Shared-name match wins (these live under .claude/worktrees but are off-limits).
  for name in $shared; do
    if [ "$base" = "$name" ]; then echo "shared:$base"; return 1; fi
  done
  # An isolated agent worktree is any OTHER path under a .claude/worktrees segment.
  case "$top" in
    */.claude/worktrees/*) echo "isolated:$base"; return 0 ;;
    *)                     echo "main:$top";       return 1 ;;
  esac
}

# worktree_toplevel <dir> — resolve the git worktree root for <dir>; falls back to
# the absolute <dir> when <dir> is not in a git repo (so the guard still classifies
# a plain path rather than crashing).
worktree_toplevel() {
  local dir="${1:-$PWD}" top
  if top="$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null)" && [ -n "$top" ]; then
    printf '%s\n' "$top"
  else
    ( cd "$dir" 2>/dev/null && pwd ) || printf '%s\n' "$dir"
  fi
}

# require_isolated_worktree [dir] — the sourceable STEP-0 guard. Returns 0 and
# prints the OK verdict when <dir> (default $PWD) is an isolated worktree; prints a
# clear refusal to stderr and returns 1 otherwise. A sourcing caller does:
#   source …/check-worktree-isolation.sh && require_isolated_worktree || exit 1
require_isolated_worktree() {
  local dir="${1:-$PWD}" top verdict rc
  top="$(worktree_toplevel "$dir")"
  verdict="$(classify_worktree_path "$top")"; rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "worktree-isolation OK: ${verdict#isolated:} ($top)"
    return 0
  fi
  {
    echo "REFUSING: not an isolated agent worktree — would edit a SHARED checkout."
    case "$verdict" in
      shared:*) echo "  this is the shared '${verdict#shared:}' worktree (off-limits, DRAIN-7)." ;;
      main:*)   echo "  this is the MAIN checkout '$top' (off-limits — agents work in their own worktree)." ;;
    esac
    echo "  cwd=$dir  toplevel=$top"
    echo "  Move to your assigned .claude/worktrees/<your-id> worktree before editing."
  } >&2
  return 1
}

# ---------------------------------------------------------------------------
# --self-test — pure-function assertions (no git, no I/O). Run first, exits.
# ---------------------------------------------------------------------------
_cwi_self_test() {
  local fails=0
  check() { # check <label> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok: $1"
    else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1)); fi
  }
  echo "check-worktree-isolation --self-test:"

  # classify_worktree_path verdict string + rc, over synthetic toplevels.
  check "isolated agent worktree → isolated" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/wf_abc123)" "isolated:wf_abc123"
  check "isolated rc is 0" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/wf_abc123 >/dev/null; echo $?)" 0

  check "shared bright-elm → shared" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/bright-elm-ajw0)" "shared:bright-elm-ajw0"
  check "shared bright-elm rc is 1" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/bright-elm-ajw0 >/dev/null; echo $?)" 1

  check "shared calm-ray → shared" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/calm-ray-dcr8)" "shared:calm-ray-dcr8"
  check "shared calm-ray rc is 1" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/calm-ray-dcr8 >/dev/null; echo $?)" 1

  check "main checkout → main" \
    "$(classify_worktree_path /root/magic-mesh)" "main:/root/magic-mesh"
  check "main checkout rc is 1" \
    "$(classify_worktree_path /root/magic-mesh >/dev/null; echo $?)" 1

  # Trailing slash is normalised (basename still resolves).
  check "trailing slash normalised" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/calm-ray-dcr8/)" "shared:calm-ray-dcr8"

  # An operator override of the shared set is honoured.
  check "override marks a custom worktree shared" \
    "$(classify_worktree_path /root/magic-mesh/.claude/worktrees/temp-x 'temp-x')" "shared:temp-x"

  # The LIVE worktree this script runs in must classify as isolated (end-to-end:
  # git resolve + classify). Skipped (not failed) outside a git repo.
  if git rev-parse --show-toplevel >/dev/null 2>&1; then
    local live; live="$(classify_worktree_path "$(worktree_toplevel "$PWD")")"
    case "$live" in
      isolated:*) echo "  ok: live worktree is isolated ($live)" ;;
      *) echo "  note: live worktree classified '$live' (run from an agent worktree to see 'isolated')" ;;
    esac
  else
    echo "  skip: live-worktree check (not in a git repo)"
  fi

  if [ "$fails" -eq 0 ]; then echo "check-worktree-isolation: self-test passed"; return 0; fi
  echo "check-worktree-isolation: SELF-TEST FAILED ($fails)" >&2; return 1
}

# Dispatch ONLY when executed directly; when sourced, just define the functions
# above so a caller can `require_isolated_worktree` as a STEP-0 guard.
if [ "${BASH_SOURCE[0]:-$0}" = "$0" ]; then
  case "${1:-}" in
    --self-test) _cwi_self_test; exit $? ;;
    -h|--help)   sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)           require_isolated_worktree "${1:-$PWD}"; exit $? ;;
  esac
fi
