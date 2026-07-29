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
MAX_EPIC_LINES="${MCNF_WORKLIST_MAX_EPIC_LINES:-220}"
MAX_CURRENT_STATE_LINES="${MCNF_WORKLIST_MAX_CURRENT_STATE_LINES:-12}"

usage() { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; }

structure_check() {
  local wl="$1"
  awk \
    -v max="$MAX_LINE_LENGTH" \
    -v max_epic="$MAX_EPIC_LINES" \
    -v max_current="$MAX_CURRENT_STATE_LINES" '
    BEGIN {
      required_count = 12
      required[1] = "Status"
      required[2] = "Priority"
      required[3] = "Complexity"
      required[4] = "Problem"
      required[5] = "Required outcome"
      required[6] = "Current state"
      required[7] = "Remaining work"
      required[8] = "Scope"
      required[9] = "Relevant files/components"
      required[10] = "Acceptance criteria"
      required[11] = "Verification method"
      required[12] = "Origin or merged source IDs"
      for (i = 1; i <= required_count; i++) {
        required_index[required[i]] = i
      }
    }
    function fail(msg) {
      print "lint-worklist.sh: " msg > "/dev/stderr"
      failed = 1
    }
    function trim(s) {
      sub(/^[[:space:]]+/, "", s)
      sub(/[[:space:]]+$/, "", s)
      return s
    }
    function finish_field(end_line, lines) {
      if (item_id == "" || current_field == "") {
        return
      }
      if (current_field == "Current state") {
        lines = end_line - current_field_line + 1
        if (lines > max_current) {
          fail(current_field_line ": " item_id " Current state spans " lines \
            " lines; maximum is " max_current)
        }
      }
      current_field = ""
      current_field_line = 0
    }
    function finish_item(end_line, i, key, lines) {
      if (item_id == "") {
        return
      }
      finish_field(end_line)
      lines = end_line - item_line + 1
      if (lines > max_epic) {
        fail(item_line ": " item_id " spans " lines \
          " lines; maximum is " max_epic)
      }
      for (i = 1; i <= required_count; i++) {
        key = item_id SUBSEP required[i]
        if (!field_seen[key]) {
          fail(item_line ": " item_id " is missing required field " required[i])
        }
      }
    }
    length($0) > max {
      fail(FNR ": line length " length($0) " exceeds " max)
    }
    /^[[:space:]]*-[[:space:]]*\[[^]]+\]/ {
      fail(FNR ": retired checkbox marker is not allowed in active worklist")
    }
    /^- \*\*[0-9]+ active epics:\*\*/ {
      snapshot_seen++
      value = $0
      sub(/^- \*\*/, "", value)
      sub(/ active epics:.*/, "", value)
      snapshot_items = value + 0

      if (match($0, /[0-9]+ `Remaining`/)) {
        value = substr($0, RSTART, RLENGTH)
        sub(/ .*/, "", value)
        snapshot_remaining = value + 0
      } else {
        fail(FNR ": snapshot is missing Remaining count")
      }
      if (match($0, /[0-9]+ `Blocked`/)) {
        value = substr($0, RSTART, RLENGTH)
        sub(/ .*/, "", value)
        snapshot_blocked = value + 0
      } else {
        fail(FNR ": snapshot is missing Blocked count")
      }
      if (match($0, /[0-9]+ `Needs clarification`/)) {
        value = substr($0, RSTART, RLENGTH)
        sub(/ .*/, "", value)
        snapshot_needs = value + 0
      } else {
        fail(FNR ": snapshot is missing Needs clarification count")
      }
      next
    }
    /^### WL-[A-Z0-9-]+ - / {
      finish_item(FNR - 1)
      item_id = $2
      item_line = FNR
      last_required_index = 0
      current_field = ""
      current_field_line = 0
      if (item_seen[item_id]++) {
        fail(FNR ": duplicate active epic ID " item_id)
      }
      items++
      next
    }
    /^## / {
      finish_item(FNR - 1)
      item_id = ""
      next
    }
    /^- Progress([[:space:](]|:)/ {
      fail(FNR ": active worklist must not contain Progress fields")
      next
    }
    {
      field = ""
      for (i = 1; i <= required_count; i++) {
        if (index($0, "- " required[i] ":") == 1) {
          field = required[i]
          break
        }
      }
      if (index($0, "- Dependencies:") == 1) {
        field = "Dependencies"
      }

      if (field == "") {
        if (item_id != "" && $0 ~ /^- [A-Z][A-Za-z /-]+:/) {
          fail(FNR ": unknown top-level field in " item_id ": " $0)
        }
        next
      }

      if (item_id == "") {
        fail(FNR ": " field " field is outside a WL item")
        next
      }

      finish_field(FNR - 1)
      key = item_id SUBSEP field
      field_seen[key]++
      if (field_seen[key] > 1) {
        fail(FNR ": duplicate " field " field in " item_id)
      }

      if (field == "Dependencies") {
        if (last_required_index != 9) {
          fail(FNR ": Dependencies in " item_id \
            " must follow Relevant files/components")
        }
      } else {
        index_now = required_index[field]
        if (index_now != last_required_index + 1) {
          expected = required[last_required_index + 1]
          fail(FNR ": field " field " is out of order in " item_id \
            "; expected " expected)
        } else {
          last_required_index = index_now
        }
      }

      current_field = field
      current_field_line = FNR

      if (field == "Status") {
        item_status = $0
        sub(/^- Status:[[:space:]]*/, "", item_status)
        item_status = trim(item_status)
        if (item_status != "Remaining" &&
            item_status != "Blocked" &&
            item_status != "Needs clarification") {
          fail(FNR ": invalid active status for " item_id ": " item_status)
        } else if (field_seen[key] == 1) {
          status_count[item_status]++
        }
      } else if (field == "Priority") {
        value = $0
        sub(/^- Priority:[[:space:]]*/, "", value)
        value = trim(value)
        if (value !~ /^P[0-3]$/) {
          fail(FNR ": invalid Priority for " item_id ": " value)
        }
      } else if (field == "Complexity") {
        value = $0
        sub(/^- Complexity:[[:space:]]*/, "", value)
        value = trim(value)
        if (value != "Small" && value != "Medium" &&
            value != "Large" && value != "Epic") {
          fail(FNR ": invalid Complexity for " item_id ": " value)
        }
      }
      next
    }
    END {
      finish_item(FNR)
      if (items == 0) {
        fail("no active WL items found")
      }
      if (snapshot_seen != 1) {
        fail("expected exactly one Current Snapshot active-epic count line")
      } else {
        if (snapshot_items != items) {
          fail("snapshot items=" snapshot_items " but parsed items=" items)
        }
        if (snapshot_remaining != status_count["Remaining"] + 0) {
          fail("snapshot Remaining=" snapshot_remaining \
            " but parsed Remaining=" status_count["Remaining"] + 0)
        }
        if (snapshot_blocked != status_count["Blocked"] + 0) {
          fail("snapshot Blocked=" snapshot_blocked \
            " but parsed Blocked=" status_count["Blocked"] + 0)
        }
        if (snapshot_needs != status_count["Needs clarification"] + 0) {
          fail("snapshot Needs clarification=" snapshot_needs \
            " but parsed Needs clarification=" \
            status_count["Needs clarification"] + 0)
        }
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
      printf '%s\n' '- **2 active epics:** 1 `Remaining`, 1 `Blocked`, 0 `Needs clarification`.'
      printf '%s\n' '### WL-TEST-001 - Good remaining item'
      printf '%s\n' '- Status: Remaining'
      printf '%s\n' '- Priority: P1'
      printf '%s\n' '- Complexity: Medium'
      printf '%s\n' '- Problem: The item is unfinished and actionable.'
      printf '%s\n' '- Required outcome: The observable result works.'
      printf '%s\n' '- Current state: The foundation exists.'
      printf '%s\n' '- Remaining work: Implement the final behavior.'
      printf '%s\n' '- Scope: The focused test surface.'
      printf '%s\n' '- Relevant files/components: `src/test.rs`.'
      printf '%s\n' '- Dependencies: None.'
      printf '%s\n' '- Acceptance criteria: The result is observable.'
      if [ -n "$farm_line" ]; then
        printf '%s\n' "- Verification method: $farm_line"
      else
        printf '%s\n' '- Verification method: Focused fixture tests.'
      fi
      printf '%s\n' '- Origin or merged source IDs: self-test.'
      printf '%s\n' '### WL-TEST-002 - Good blocked item'
      printf '%s\n' '- Status: Blocked'
      printf '%s\n' '- Priority: P2'
      printf '%s\n' '- Complexity: Small'
      printf '%s\n' '- Problem: The item needs a live resource.'
      printf '%s\n' '- Required outcome: The resource-backed proof passes.'
      printf '%s\n' '- Current state: Local implementation is complete.'
      printf '%s\n' '- Remaining work: Run the named live proof.'
      printf '%s\n' '- Scope: The live proof only.'
      printf '%s\n' '- Relevant files/components: `src/live.rs`.'
      printf '%s\n' '- Acceptance criteria: The live result is observed.'
      printf '%s\n' '- Verification method: Direct live smoke.'
      printf '%s\n' '- Origin or merged source IDs: self-test-live.'
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

  sed '0,/- Status: Remaining/s//- Status: Completed/' \
    "$td/good.md" >"$td/completed.md"
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

  sed '0,/- Status: Remaining/d' "$td/good.md" >"$td/missing-status.md"
  expect_fail "missing status" "$td/missing-status.md"

  sed '0,/- Current state:/d' "$td/good.md" >"$td/missing-field.md"
  expect_fail "missing required field" "$td/missing-field.md"

  awk '
    { print }
    /^- Scope:/ && !inserted {
      print "- Scope: Duplicate scope."
      inserted = 1
    }
  ' "$td/good.md" >"$td/duplicate-field.md"
  expect_fail "duplicate required field" "$td/duplicate-field.md"

  awk '
    /^- Problem:/ && !moved {
      problem = $0
      moved = 1
      next
    }
    /^- Required outcome:/ && moved && !printed {
      print
      print problem
      printed = 1
      next
    }
    { print }
  ' "$td/good.md" >"$td/out-of-order.md"
  expect_fail "out-of-order fields" "$td/out-of-order.md"

  write_good "$td/progress.md"
  printf '%s\n' '- Progress (today): historical diary entry.' >>"$td/progress.md"
  expect_fail "active progress diary" "$td/progress.md"

  write_good "$td/oversized.md"
  for ((i = 0; i < 221; i++)); do
    printf '%s\n' '  Additional oversized epic detail.' >>"$td/oversized.md"
  done
  expect_fail "oversized active epic" "$td/oversized.md"

  sed 's/\*\*2 active epics:/\*\*3 active epics:/' \
    "$td/good.md" >"$td/stale-count.md"
  expect_fail "stale snapshot count" "$td/stale-count.md"

  sed '0,/- Priority: P1/s//- Priority: urgent/' \
    "$td/good.md" >"$td/bad-priority.md"
  expect_fail "invalid priority" "$td/bad-priority.md"

  sed '0,/- Complexity: Medium/s//- Complexity: Huge/' \
    "$td/good.md" >"$td/bad-complexity.md"
  expect_fail "invalid complexity" "$td/bad-complexity.md"

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
