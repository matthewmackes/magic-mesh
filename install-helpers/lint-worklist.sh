#!/usr/bin/env bash
# lint-worklist.sh — guard the reconciled platform worklist from regressing into
# the old mixed active/archive tracker shape.
#
# Exit 0 = clean. Run with --self-test to exercise planted failures.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKLIST="${MCNF_WORKLIST:-$ROOT/docs/platform/WORKLIST.md}"
FARM_JOBS="${MCNF_FARM_JOBS:-$ROOT/automation/lib/farm-jobs.sh}"
MAX_LINE_LENGTH="${MCNF_WORKLIST_MAX_LINE_LENGTH:-180}"

usage() { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; }

structure_check() {
  local wl="$1"
  awk -v max="$MAX_LINE_LENGTH" '
    function fail(msg) {
      print "lint-worklist.sh: " msg > "/dev/stderr"
      failed = 1
    }
    function finish_item() {
      if (item_id != "" && item_status == "") {
        fail(item_line ": " item_id " is missing a Status line")
      }
    }
    length($0) > max {
      fail(FNR ": line length " length($0) " exceeds " max)
    }
    /^[[:space:]]*-[[:space:]]*\[[^]]+\]/ {
      fail(FNR ": retired checkbox marker is not allowed in active worklist")
    }
    /^### WL-[A-Z0-9-]+ - / {
      finish_item()
      item_id = $2
      item_line = FNR
      item_status = ""
      items++
      next
    }
    /^[[:space:]]*-[[:space:]]Status:[[:space:]]*/ {
      if (item_id == "") {
        fail(FNR ": Status line is outside a WL item")
        next
      }
      item_status = $0
      sub(/^[[:space:]]*-[[:space:]]Status:[[:space:]]*/, "", item_status)
      if (item_status != "Remaining" &&
          item_status != "Blocked" &&
          item_status != "Needs clarification") {
        fail(FNR ": invalid active status for " item_id ": " item_status)
      }
      status_count[item_status]++
      next
    }
    END {
      finish_item()
      if (items == 0) {
        fail("no active WL items found")
      }
      printf "lint-worklist.sh: items=%d remaining=%d blocked=%d needs_clarification=%d\n",
        items,
        status_count["Remaining"] + 0,
        status_count["Blocked"] + 0,
        status_count["Needs clarification"] + 0
      exit failed ? 1 : 0
    }
  ' "$wl"
}

secret_check() {
  local wl="$1"
  awk '
    function fail(msg) {
      print "lint-worklist.sh: " msg > "/dev/stderr"
      failed = 1
    }
    /DO[A-Z0-9]{16,}/ {
      fail(FNR ": DigitalOcean-token-shaped value must not appear in active worklist")
    }
    /(AKIA|ASIA)[A-Z0-9]{16}/ {
      fail(FNR ": AWS-key-shaped value must not appear in active worklist")
    }
    /age-secret-key-[a-z0-9]+/ {
      fail(FNR ": age secret key must not appear in active worklist")
    }
    /BEGIN [A-Z ]*PRIVATE KEY/ {
      fail(FNR ": private key material must not appear in active worklist")
    }
    index($0, "mm/<") && $0 !~ /mm\/<REDACTED>/ {
      fail(FNR ": credential path placeholders must be redacted")
    }
    END { exit failed ? 1 : 0 }
  ' "$wl"
}

farm_payload_check() {
  local wl="$1"
  awk '
    function trim(s) {
      sub(/^[[:space:]]+/, "", s)
      sub(/[[:space:]]+$/, "", s)
      return s
    }
    function fail(msg) {
      print "lint-worklist.sh: " msg > "/dev/stderr"
      failed = 1
    }
    {
      rest = $0
      while ((pos = index(rest, "@farm:{")) > 0) {
        rest = substr(rest, pos + 7)
        end = index(rest, "}")
        if (end == 0) {
          fail(FNR ": unterminated @farm payload")
          break
        }
        cmd = trim(substr(rest, 1, end - 1))
        if (cmd !~ /^cargo[[:space:]]/) {
          fail(FNR ": non-cargo or placeholder @farm payload: " cmd)
        }
        rest = substr(rest, end + 1)
      }
    }
    END { exit failed ? 1 : 0 }
  ' "$wl"
}

farm_parser_check() {
  local wl="$1"
  [ -x "$FARM_JOBS" ] || return 0
  MCNF_WORKLIST="$wl" "$FARM_JOBS" list >/dev/null
}

lint_one() {
  local wl="$1" rc=0
  if [ ! -f "$wl" ]; then
    echo "lint-worklist.sh: missing worklist: $wl" >&2
    return 1
  fi
  structure_check "$wl" || rc=1
  secret_check "$wl" || rc=1
  farm_payload_check "$wl" || rc=1
  if ! farm_parser_check "$wl"; then
    echo "lint-worklist.sh: farm job parser could not parse $wl" >&2
    rc=1
  fi
  return "$rc"
}

self_test() {
  local td fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/lint-worklist.XXXXXX")" || return 1
  trap "rm -rf '$td'" EXIT

  write_good() {
    local path="$1" farm_line="${2:-}"
    {
      printf '%s\n' '# Platform Worklist'
      printf '%s\n' '### WL-TEST-001 - Good remaining item'
      printf '%s\n' '- Status: Remaining'
      printf '%s\n' '- Problem: The item is unfinished and actionable.'
      if [ -n "$farm_line" ]; then
        printf '%s\n' "- Verification method: $farm_line"
      fi
      printf '%s\n' '### WL-TEST-002 - Good blocked item'
      printf '%s\n' '- Status: Blocked'
      printf '%s\n' '- Problem: The item needs a live resource.'
    } >"$path"
  }

  expect_pass() {
    local label="$1" path="$2"
    if lint_one "$path" >/dev/null 2>/dev/null; then
      echo "  ok: $label"
    else
      echo "  FAIL: $label should pass" >&2
      fails=$((fails + 1))
    fi
  }

  expect_fail() {
    local label="$1" path="$2"
    if lint_one "$path" >/dev/null 2>/dev/null; then
      echo "  FAIL: $label should fail" >&2
      fails=$((fails + 1))
    else
      echo "  ok: $label"
    fi
  }

  write_good "$td/good.md"
  expect_pass "clean worklist" "$td/good.md"

  write_good "$td/good-farm.md" '@farm:{cargo test -p mde-bus}'
  expect_pass "real cargo farm payload" "$td/good-farm.md"

  write_good "$td/completed.md"
  printf '%s\n' '### WL-TEST-003 - Invalid completed item' '- Status: Completed' >>"$td/completed.md"
  expect_fail "completed status marker" "$td/completed.md"

  write_good "$td/checkbox.md"
  printf '%s\n' '- [x] **OLD-1: completed old row.**' >>"$td/checkbox.md"
  expect_fail "retired checkbox marker" "$td/checkbox.md"

  write_good "$td/long.md"
  printf -- '- Problem: %190s\n' '' | tr ' ' x >>"$td/long.md"
  expect_fail "mega-line" "$td/long.md"

  write_good "$td/secret.md"
  printf '%s\n' '- Problem: DOABCDEFGHIJKLMNOP' >>"$td/secret.md"
  expect_fail "credential-shaped token" "$td/secret.md"

  write_good "$td/farm-placeholder.md" '@farm:{crate,verify}'
  expect_fail "placeholder farm job" "$td/farm-placeholder.md"

  {
    printf '%s\n' '# Platform Worklist'
    printf '%s\n' '### WL-TEST-001 - Missing status'
    printf '%s\n' '- Problem: This should fail.'
  } >"$td/missing-status.md"
  expect_fail "missing status" "$td/missing-status.md"

  if [ "$fails" -eq 0 ]; then
    echo "lint-worklist.sh: self-test passed"
    return 0
  fi
  echo "lint-worklist.sh: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) usage ;;
  "") lint_one "$WORKLIST" ;;
  *) MCNF_WORKLIST="$1" lint_one "$1" ;;
esac
