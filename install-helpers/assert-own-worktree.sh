#!/usr/bin/env bash
# assert-own-worktree.sh — DRAIN-7 guardrail (worktree-isolation discipline,
# operator-locked 2026-06-24). A subagent that strays into the SHARED coordinator
# worktree (calm-ray-dcr8) or the PRIMARY checkout (/root/magic-mesh) can have its
# uncommitted work wiped by the coordinator's reset/merge — it happened once
# (2026-06-24) and silently lost an agent's work.
#
# This is an isolated agent's STEP-0 guard: run it before touching any file. It
# REFUSES (exit 3) when the current working tree is the shared or primary checkout,
# so an agent can never edit/reset the coordinator's worktree. It PASSES (exit 0)
# only from the agent's OWN isolated worktree.
#
#   STEP-0 in every isolated subagent:
#     ./install-helpers/assert-own-worktree.sh || exit 1
#
# Inverse for the coordinator/main loop (assert you ARE the shared checkout, e.g.
# to fail fast if launched from the wrong place):
#     ./install-helpers/assert-own-worktree.sh --coordinator
#
# Overridable for tests / a relocated repo:
#   MCNF_REPO              primary checkout            (default /root/magic-mesh)
#   MCNF_ACTIVE_WORKTREE   shared coordinator worktree (default calm-ray-dcr8)
set -uo pipefail

MODE="${1:-agent}"
REPO="${MCNF_REPO:-/root/magic-mesh}"
ACTIVE="${MCNF_ACTIVE_WORKTREE:-calm-ray-dcr8}"

# Resolve the current git worktree root. No git / not a checkout is itself a
# failure for an agent (it must be in its assigned worktree).
top="$(git rev-parse --show-toplevel 2>/dev/null)" || top=""
if [ -z "$top" ]; then
  echo "assert-own-worktree: not inside a git checkout (cwd=$PWD)" >&2
  exit 2
fi
top="$(readlink -f "$top" 2>/dev/null || echo "$top")"
repo_real="$(readlink -f "$REPO" 2>/dev/null || echo "$REPO")"
shared_real="$(readlink -f "$repo_real/.claude/worktrees/$ACTIVE" 2>/dev/null || echo "$repo_real/.claude/worktrees/$ACTIVE")"

is_shared=0
[ "$top" = "$repo_real" ] && is_shared=1     # the primary checkout
[ "$top" = "$shared_real" ] && is_shared=1   # the shared coordinator worktree

case "$MODE" in
  --coordinator)
    if [ "$is_shared" -eq 1 ]; then
      echo "assert-own-worktree: OK — coordinator in the shared checkout ($top)"
      exit 0
    fi
    echo "assert-own-worktree: REFUSE — --coordinator expected the shared checkout" >&2
    echo "   ($repo_real or .../$ACTIVE), but cwd worktree is $top" >&2
    exit 3 ;;
  agent|--agent)
    if [ "$is_shared" -eq 1 ]; then
      echo "✋ assert-own-worktree: REFUSE — this is the SHARED/primary checkout:" >&2
      echo "     $top" >&2
      echo "   An isolated agent must operate ONLY in its own worktree (DRAIN-7)." >&2
      echo "   Editing/resetting the shared checkout can wipe the coordinator's" >&2
      echo "   work (it did, 2026-06-24). cd to your assigned worktree first." >&2
      exit 3
    fi
    echo "assert-own-worktree: OK — isolated worktree ($top), not the shared $ACTIVE"
    exit 0 ;;
  *)
    echo "usage: $0 [agent|--coordinator]" >&2; exit 2 ;;
esac
