#!/usr/bin/env bash
# disk-watchdog.sh — DRAIN-ENGINE guardrail (hard-enforcement, operator-locked
# 2026-06-24). Reclaim dev-host disk so a stray local build / accumulated
# Codex worktrees never wedges project cleanup or farm dispatch.
#
# Run it:
#   * pre-flight before spawning farm agents, and
#   * as a systemd timer (install-helpers/install-drain-guardrails.sh), and
#   * on demand.
#
# Default threshold 8G free on /; pass an arg to override. Exits 0 always
# (best-effort — never blocks the caller). Safe: removes only ephemeral
# agent worktree target/ dirs, local target/ dirs (we build on the farm for
# heavy work), and aged task-output logs.
set -uo pipefail
THRESH_GB="${1:-8}"
REPO="${MCNF_REPO:-/root/magic-mesh}"
WT="${MCNF_AGENT_WORKTREES:-$REPO/.agents/worktrees}"
KEEP="${MCNF_ACTIVE_WORKTREE:-}"

free_gb() { df -P / | awk 'NR==2{print int($4/1024/1024)}'; }
before="$(free_gb)"
if [ "$before" -ge "$THRESH_GB" ]; then
  echo "disk-watchdog: ${before}G free >= ${THRESH_GB}G — ok"; exit 0
fi
echo "disk-watchdog: ${before}G free < ${THRESH_GB}G — RECLAIMING"

# 1) The disk hog is a local target/ from a stray local build. With the cargo
#    guard installed these never appear; reclaim any as a backstop. This is
#    SAFE for a live agent: with farm-only builds there is no local target/ to
#    lose, and the source checkout is left intact. Whole-worktree removal is
#    the COORDINATOR's job AFTER it merges that agent's PR — the watchdog must
#    never nuke a possibly-live agent's worktree.
for t in "$REPO"/target "$WT"/*/target "$REPO"/.agents/*/target; do
  [ -d "$t" ] || continue
  [ -n "$KEEP" ] && case "$t" in */"$KEEP"/*) continue;; esac
  rm -rf "$t"
done
# Drop admin entries for worktrees the coordinator already removed.
git -C "$REPO" worktree prune 2>/dev/null || true

# 3) aged task-output logs
find /tmp -maxdepth 2 \( -path '/tmp/codex-*' -o -path '/tmp/magic-mesh-*' \) \
  -name '*.output' -type f -mmin +30 -delete 2>/dev/null || true

# 4) stale farm slot dirs are reclaimed by xcp-build itself; not touched here.
after="$(free_gb)"
echo "disk-watchdog: reclaimed -> ${after}G free (+$((after-before))G)"
exit 0
