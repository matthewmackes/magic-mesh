#!/usr/bin/env bash
# release-stage-journal.sh — restart-safe, candidate-bound stage receipts.
#
# Source this helper from an orchestration script.  A receipt is written with
# an atomic rename only after its stage succeeds; the next stage requires the
# matching predecessor receipt.  This is execution state, not release evidence.

release_journal_dir="${MCNF_RELEASE_JOURNAL_DIR:-${PROMOTION_STATE_DIR:-}}"
release_journal_source_revision="${MCNF_RELEASE_SOURCE_REVISION:-unknown}"
release_journal_owner="${MCNF_RELEASE_OWNER:-${USER:-unknown}@${HOSTNAME:-unknown}}"

release_journal_path() {
  printf '%s/%s.receipt.json\n' "$release_journal_dir" "$1"
}

release_journal_owner_path() {
  printf '%s/%s.owner.json\n' "$release_journal_dir" "$1"
}

release_journal_value() {
  local stage="$1" key="$2" line
  line="$(release_journal_path "$stage")"
  [ -f "$line" ] || return 1
  sed -n "s/.*\"$key\":\"\([^\"]*\)\".*/\1/p" "$line"
}

release_journal_owner_value() {
  local stage="$1" key="$2" line
  line="$(release_journal_owner_path "$stage")"
  [ -f "$line" ] || return 1
  sed -n "s/.*\"$key\":\"\([^\"]*\)\".*/\1/p" "$line"
}

release_journal_claim_stage() {
  local stage="$1" path tmp existing_owner existing_source
  [ -n "$stage" ] || {
    printf 'ERROR: stage ownership needs a stage\n' >&2
    return 1
  }
  [ -n "$release_journal_owner" ] || {
    printf 'ERROR: stage ownership needs an owner identity\n' >&2
    return 1
  }
  mkdir -p "$release_journal_dir"
  path="$(release_journal_owner_path "$stage")"
  if [ -f "$path" ]; then
    existing_owner="$(release_journal_owner_value "$stage" owner 2>/dev/null || true)"
    existing_source="$(release_journal_owner_value "$stage" source_revision 2>/dev/null || true)"
    [ "$existing_owner" = "$release_journal_owner" ] &&
      [ "$existing_source" = "$release_journal_source_revision" ] || {
      printf 'ERROR: stage %s is owned by %s for source %s\n' \
        "$stage" "${existing_owner:-unknown}" "${existing_source:-unknown}" >&2
      return 1
    }
    return 0
  fi
  tmp="${path}.$$"
  printf '{"schema":"ReleaseStageOwnerV1","stage":"%s","source_revision":"%s","owner":"%s","claimed_at":"%s"}\n' \
    "$stage" "$release_journal_source_revision" "$release_journal_owner" \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$tmp"
  if mv -n "$tmp" "$path" 2>/dev/null; then
    if [ -f "$path" ] &&
      [ "$(release_journal_owner_value "$stage" owner 2>/dev/null || true)" = "$release_journal_owner" ] &&
      [ "$(release_journal_owner_value "$stage" source_revision 2>/dev/null || true)" = "$release_journal_source_revision" ]; then
      return 0
    fi
  fi
  rm -f "$tmp"
  existing_owner="$(release_journal_owner_value "$stage" owner 2>/dev/null || true)"
  existing_source="$(release_journal_owner_value "$stage" source_revision 2>/dev/null || true)"
  [ "$existing_owner" = "$release_journal_owner" ] &&
    [ "$existing_source" = "$release_journal_source_revision" ] || {
    printf 'ERROR: stage %s ownership claim lost to %s for source %s\n' \
      "$stage" "${existing_owner:-unknown}" "${existing_source:-unknown}" >&2
    return 1
  }
}

release_journal_assert_current_source() {
  local root="${1:-}" current
  [ -n "$root" ] || {
    printf 'ERROR: current-source check needs a repository root\n' >&2
    return 1
  }
  current="$(git -C "$root" rev-parse HEAD 2>/dev/null || true)"
  [ -n "$current" ] && [ "$current" = "$release_journal_source_revision" ] || {
    printf 'ERROR: current source %s does not match release source %s\n' \
      "${current:-unavailable}" "$release_journal_source_revision" >&2
    return 1
  }
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
  release_journal_claim_stage "$stage" || return 1
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
  local tmp sha old_owner
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  release_journal_dir="$tmp"
  release_journal_source_revision="self-test-source"
  release_journal_owner="self-test-owner"
  sha="$(printf candidate | sha256sum | awk '{print $1}')"
  release_journal_claim_stage build
  old_owner="$release_journal_owner"
  release_journal_owner="other-owner"
  if release_journal_claim_stage build 2>/dev/null; then
    echo "self-test: competing stage owner was accepted" >&2
    return 1
  fi
  release_journal_owner="$old_owner"
  release_journal_record_pass build "$sha" "" farm-build
  release_journal_complete build "$sha"
  release_journal_record_pass l1 "$sha" build clean-install
  release_journal_record_pass l1 "$sha" build clean-install
  if release_journal_record_pass eagle deadbeef l1 wrong-candidate 2>/dev/null; then
    echo "self-test: dependency mismatch was accepted" >&2
    return 1
  fi
  [ "$(release_journal_value l1 schema)" = ReleaseStageReceiptV1 ]
  [ "$(release_journal_owner_value build owner)" = self-test-owner ]
  echo "release-stage-journal: ALL PASS"
}

if [ "${1:-}" = self-test ]; then
  release_journal_self_test
fi
