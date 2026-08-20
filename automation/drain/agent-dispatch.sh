#!/usr/bin/env bash
# agent-dispatch.sh — DRAIN-7 implementation-agent bridge.
#
# The farm queue already automates build/test consumers. This bridge is the
# missing boundary between Remaining worklist units and an implementation-agent
# runtime (Codex IDE, Claude Code, or another approved supervisor). It never
# invents prompts or bypasses worktree ownership: it emits a bounded plan from
# farm-jobs.sh and invokes only the explicit MCNF_AGENT_DISPATCHER adapter.
#
# Modes:
#   agent-dispatch.sh plan [N]       write/print the next N job records
#   agent-dispatch.sh dispatch [N]   invoke the configured adapter per record
#   agent-dispatch.sh --self-test    exercise planning with a fixture worklist
#
# Adapter contract:
#   "$MCNF_AGENT_DISPATCHER" --runtime cursor|codex|claude \
#     --job-id ID --epic EPIC --command "cargo …"
# The adapter owns Codex/Claude invocation, isolated worktree creation, and
# disjoint scope assignment. An unset adapter or runtime is a hard dispatch
# failure, not a successful no-op. The adapter MUST be native to the invoking
# tool; cross-tool fallback is forbidden.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
JOBS="${MCNF_FARM_JOBS:-$REPO/automation/lib/farm-jobs.sh}"
WORKLIST="${MCNF_WORKLIST:-$REPO/docs/platform/WORKLIST.md}"
STATE="${MCNF_AGENT_DISPATCH_STATE:-$REPO/automation/.state/agent-dispatch-plan.tsv}"
RUNTIME="${MCNF_AGENT_RUNTIME:-}"
if [[ -z "$RUNTIME" ]]; then
  if [[ "${CURSOR_AGENT:-}" == "1" ]]; then RUNTIME=cursor
  elif [[ -n "${CODEX_HOME:-}" ]]; then RUNTIME=codex
  elif [[ -n "${CLAUDE_CODE:-}" ]]; then RUNTIME=claude
  fi
fi
ADAPTER="${MCNF_AGENT_DISPATCHER:-$HERE/native-agent-dispatch.sh}"

die() { printf 'agent-dispatch: %s\n' "$*" >&2; exit 2; }

validate_runtime() {
  case "$RUNTIME" in
    cursor|codex|claude) ;;
    '') die "MCNF_AGENT_RUNTIME must be cursor, codex, or claude" ;;
    *) die "unsupported MCNF_AGENT_RUNTIME '$RUNTIME'" ;;
  esac
}

free_slots() {
  local output total
  output="$("$REPO/install-helpers/drain-coordinator.sh" slots 2>/dev/null)"
  total="$(awk -F= '/^TOTAL_FREE=/{print $2}' <<<"$output")"
  [[ "$total" =~ ^[0-9]+$ ]] || die "could not determine free farm slots"
  printf '%s\n' "$total"
}

plan() {
  local requested="${1:-}" limit jobs
  jobs="$("$JOBS" active)"
  limit="${requested:-$(free_slots)}"
  [[ "$limit" =~ ^[0-9]+$ ]] || die "plan limit must be a non-negative integer"
  mkdir -p "$(dirname "$STATE")"
  {
    printf '# generated=%s free_slots=%s worklist=%s\n' \
      "$(date -u +%FT%TZ)" "$(free_slots)" "$WORKLIST"
    awk -F '\t' -v limit="$limit" '
      BEGIN { count = 0 }
      NF >= 4 && ($2 == "open" || $2 == "prog") {
        printf "%s\t%s\t%s\t%s\n", $1, $3, $2, $4
        count++
        if (count >= limit) exit
      }
    ' <<<"$jobs"
  } | tee "$STATE"
}

dispatch() {
  local requested="${1:-}" limit=0 line jid epic status command
  validate_runtime
  [[ -n "$ADAPTER" && -x "$ADAPTER" ]] || die \
    "no executable MCNF_AGENT_DISPATCHER configured for runtime $RUNTIME; plan is available at $STATE"
  [[ -f "$STATE" ]] || plan "$requested" >/dev/null
  [[ -n "$requested" ]] && limit="$requested"
  while IFS=$'\t' read -r jid epic status command; do
    [[ "$jid" == \#* || -z "$jid" ]] && continue
    "$ADAPTER" --runtime "$RUNTIME" --job-id "$jid" --epic "$epic" --command "$command"
    if (( limit > 0 )); then
      limit=$((limit - 1))
      (( limit == 0 )) && break
    fi
  done < "$STATE"
}

self_test() {
  local td wl plan_file
  td="$(mktemp -d "${TMPDIR:-/tmp}/agent-dispatch.XXXXXX")"
  wl="$td/WORKLIST.md"
  plan_file="$td/plan.tsv"
  cat >"$wl" <<'EOF'
### WL-TEST-001 - real unit
- Status: Remaining
- Verification method: @farm:{cargo test -p mde-bus}
### WL-TEST-002 - blocked unit
- Status: Blocked
- Verification method: @farm:{cargo test -p mde-theme}
EOF
  MCNF_WORKLIST="$wl" MCNF_AGENT_DISPATCH_STATE="$plan_file" \
    "$JOBS" active >"$td/jobs"
  [[ "$(wc -l <"$td/jobs")" -eq 1 ]] || die "self-test queue count mismatch"
  # The plan parser is intentionally exercised through the real queue output.
  awk -F '\t' 'NF == 4 && $3 == "WL-TEST-001" { ok = 1 } END { exit ok ? 0 : 1 }' \
    "$td/jobs" || die "self-test job identity mismatch"
  rm -rf "$td"
  echo "agent-dispatch: self-test passed"
}

case "${1:-plan}" in
  plan) shift; plan "${1:-}" ;;
  dispatch) shift; dispatch "${1:-}" ;;
  --self-test) self_test ;;
  -h|--help)
    sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
    ;;
  *) die "usage: $0 {plan [N]|dispatch [N]|--self-test}" ;;
esac
