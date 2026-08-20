#!/usr/bin/env bash
# release-stage-journal.sh — restart-safe, candidate-bound stage receipts.
#
# Source this helper from an orchestration script.  A receipt is written with
# an atomic rename only after its stage succeeds; the next stage requires the
# matching predecessor receipt.  This is execution state, not release evidence.

release_journal_dir="${MCNF_RELEASE_JOURNAL_DIR:-${PROMOTION_STATE_DIR:-}}"
release_journal_source_revision="${MCNF_RELEASE_SOURCE_REVISION:-unknown}"

release_journal_path() {
  printf '%s/%s.receipt.json\n' "$release_journal_dir" "$1"
}

release_journal_value() {
  local stage="$1" key="$2" line
  line="$(release_journal_path "$stage")"
  [ -f "$line" ] || return 1
  sed -n "s/.*\"$key\":\"\([^\"]*\)\".*/\1/p" "$line"
}

release_journal_complete() {
  local stage="$1" sha="$2" source_revision
  [ -n "$sha" ] || return 1
  source_revision="$(release_journal_value "$stage" source_revision 2>/dev/null || true)"
  [ "$(release_journal_value "$stage" status 2>/dev/null || true)" = pass ] &&
    [ "$(release_journal_value "$stage" candidate_sha256 2>/dev/null || true)" = "$sha" ] &&
    [ "$source_revision" = "$release_journal_source_revision" ]
}

release_journal_require_previous() {
  local stage="$1" sha="$2" previous="${3:-}" previous_sha
  [ -n "$previous" ] || return 0
  previous_sha="$(release_journal_value "$previous" candidate_sha256 2>/dev/null || true)"
  [ "$(release_journal_value "$previous" status 2>/dev/null || true)" = pass ] &&
    [ "$previous_sha" = "$sha" ] ||
    {
      printf 'ERROR: stage %s requires a passing %s receipt for candidate %s\n' \
        "$stage" "$previous" "$sha" >&2
      return 1
    }
}

release_journal_record_pass() {
  local stage="$1" sha="$2" previous="${3:-}" detail="${4:-}" path tmp
  [ -n "$stage" ] && [ -n "$sha" ] || {
    printf 'ERROR: stage receipt needs a stage and candidate hash\n' >&2
    return 1
  }
  mkdir -p "$release_journal_dir"
  release_journal_require_previous "$stage" "$sha" "$previous" || return 1
  path="$(release_journal_path "$stage")"
  if [ -f "$path" ]; then
    release_journal_complete "$stage" "$sha" || {
      printf 'ERROR: refusing to replace a different receipt for stage %s\n' "$stage" >&2
      return 1
    }
    return 0
  fi
  tmp="${path}.$$"
  printf '{"schema":"ReleaseStageReceiptV1","stage":"%s","source_revision":"%s","candidate_sha256":"%s","previous_stage":"%s","status":"pass","ts":"%s","detail":"%s"}\n' \
    "$stage" "$release_journal_source_revision" "$sha" "$previous" \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$detail" >"$tmp"
  mv -n "$tmp" "$path" || {
    rm -f "$tmp"
    release_journal_complete "$stage" "$sha"
  }
}

release_journal_self_test() {
  local tmp sha
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  release_journal_dir="$tmp"
  sha="$(printf candidate | sha256sum | awk '{print $1}')"
  release_journal_record_pass build "$sha" "" farm-build
  release_journal_complete build "$sha"
  release_journal_record_pass l1 "$sha" build clean-install
  release_journal_record_pass l1 "$sha" build clean-install
  if release_journal_record_pass eagle deadbeef l1 wrong-candidate 2>/dev/null; then
    echo "self-test: dependency mismatch was accepted" >&2
    return 1
  fi
  [ "$(release_journal_value l1 schema)" = ReleaseStageReceiptV1 ]
  echo "release-stage-journal: ALL PASS"
}

if [ "${1:-}" = self-test ]; then
  release_journal_self_test
fi
