#!/usr/bin/env bash
# release-evidence.sh — WL-CRIT-006 deterministic release evidence.
#
# This helper records the evidence envelope around a candidate release. It does
# not sign artifacts, publish a release, install anything, or promote a fleet;
# install-helpers/sign-release.sh remains the operator-gated signing path.
#
# The output is deliberately timestamp-free and sorted. Re-running `write` with
# the same inputs and unchanged artifacts produces byte-identical JSON.
#
# Usage:
#   install-helpers/release-evidence.sh write --out evidence.json \
#     --source-commit <git-sha> \
#     --artifact path/to/package.rpm \
#     --check cargo-test=pass \
#     --farm-job <job-id> --farm-slot <host/slot> \
#     --sbom package=pass --fedora-target fedora-44=pass \
#     --live-gate dell-gui=unavailable \
#     --unavailable "live Dell visual signoff not available" \
#     --preview-verdict pass --production-verdict not-promoted
#   install-helpers/release-evidence.sh validate evidence.json
#   install-helpers/release-evidence.sh --self-test
#
# Requires: bash, jq, realpath, stat, sha256sum, mktemp.
set -euo pipefail

SCHEMA_VERSION=1

die() {
  echo "release-evidence: $*" >&2
  exit 2
}

usage() {
  sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'USAGE'

Commands:
  write --out FILE --source-commit SHA --artifact PATH...
        --check NAME=STATUS... --farm-job ID... --farm-slot ID...
        --sbom NAME=STATUS... --fedora-target TARGET=STATUS...
        --live-gate NAME=STATUS... [--unavailable TEXT...]
        --preview-verdict STATUS --production-verdict STATUS
  validate FILE
  --self-test

Statuses:
  pass | fail | blocked | not-run | unavailable

The production verdict additionally accepts: not-promoted.
All required sections must be present and non-empty except
unavailable_evidence, which may be an explicit empty array.
USAGE
}

need_commands() {
  local command
  for command in jq realpath stat sha256sum mktemp; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
  done
}

valid_status() {
  case "${1:-}" in
    pass | fail | blocked | not-run | unavailable) return 0 ;;
    *) return 1 ;;
  esac
}

valid_verdict() {
  case "${1:-}" in
    pass | fail | blocked | not-run | unavailable | not-promoted) return 0 ;;
    *) return 1 ;;
  esac
}

pair_parts() {
  local pair="$1" name status
  [[ "$pair" == *=* ]] || die "expected NAME=STATUS, got: $pair"
  name="${pair%%=*}"
  status="${pair#*=}"
  [ -n "$name" ] || die "pair name must not be empty: $pair"
  [ -n "$status" ] || die "pair status must not be empty: $pair"
  valid_status "$status" || die "invalid status '$status' in: $pair"
  printf '%s\t%s\n' "$name" "$status"
}

named_status_json() {
  local field="$1" pair name status
  shift
  [ "$#" -gt 0 ] || die "at least one $field entry is required"
  for pair in "$@"; do
    IFS=$'\t' read -r name status < <(pair_parts "$pair")
    jq -cn --arg field "$field" --arg name "$name" --arg status "$status" \
      '{($field): $name, status: $status}'
  done | jq -s --arg field "$field" 'sort_by(.[$field])'
}

string_array_json() {
  [ "$#" -gt 0 ] || die "at least one value is required"
  printf '%s\n' "$@" | jq -R -s 'split("\n") | map(select(length > 0)) | sort | unique'
}

artifact_json() {
  local path="$1" resolved size digest
  [ -f "$path" ] || die "artifact is not a regular file: $path"
  resolved="$(realpath -e -- "$path")" || die "could not resolve artifact: $path"
  size="$(stat -c '%s' -- "$resolved")" || die "could not stat artifact: $resolved"
  digest="$(sha256sum -- "$resolved" | awk '{print $1}')"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "could not hash artifact: $resolved"
  jq -cn --arg path "$resolved" --arg sha256 "$digest" --argjson size "$size" \
    '{path: $path, size_bytes: $size, sha256: $sha256}'
}

validate_file() {
  local file="$1"
  [ -f "$file" ] || die "evidence file is not a regular file: $file"
  jq -e '
    def status_ok:
      . as $s |
      ($s | type) == "string" and
      (["pass", "fail", "blocked", "not-run", "unavailable"] | index($s)) != null;
    def verdict_ok:
      . as $s |
      ($s | type) == "string" and
      (["pass", "fail", "blocked", "not-run", "unavailable", "not-promoted"] | index($s)) != null;
    def nonempty_sorted_unique_strings:
      (type == "array") and (length > 0) and
      all(.[]; (type == "string") and (length > 0)) and
      (. == (sort | unique));
    def sorted_unique_named($field):
      (type == "array") and (length > 0) and
      all(.[];
        (type == "object") and
        (keys == ([$field, "status"] | sort)) and
        (.[$field] | type == "string" and length > 0) and
        (.status | status_ok)) and
      (. == (sort_by(.[$field]))) and
      ((map(.[$field]) | unique | length) == length);

    (type == "object") and
    (keys == ["artifacts", "checks", "farm", "fedora_matrix", "live_gates", "sbom", "schema_version", "source_commit", "unavailable_evidence", "verdict"]) and
    (.schema_version == 1) and
    (.source_commit | type == "string" and test("^[0-9A-Fa-f]{7,64}$")) and
    (.artifacts | type == "array" and length > 0 and
      all(.[];
        (type == "object") and
        (keys == ["path", "sha256", "size_bytes"]) and
        (.path | type == "string" and length > 0) and
        (.size_bytes | type == "number" and floor == . and . >= 0) and
        (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))) and
      (. == (sort_by(.path))) and
      ((map(.path) | unique | length) == length)) and
    (.checks | sorted_unique_named("name")) and
    (.farm | type == "object" and (keys == ["job_ids", "slot_ids"]) and
      (.job_ids | nonempty_sorted_unique_strings) and
      (.slot_ids | nonempty_sorted_unique_strings)) and
    (.sbom | sorted_unique_named("name")) and
    (.fedora_matrix | sorted_unique_named("target")) and
    (.live_gates | sorted_unique_named("name")) and
    (.unavailable_evidence | type == "array" and
      all(.[]; type == "string" and length > 0) and
      (. == (sort | unique))) and
    (.verdict | type == "object" and (keys == ["preview", "production"]) and
      (.preview | verdict_ok) and (.production | verdict_ok))
  ' "$file" >/dev/null || die "invalid or incomplete evidence schema: $file"
  echo "release-evidence: valid $file"
}

write_evidence() {
  local out="" source_commit="" preview="" production="" arg parent tmp
  local -a artifacts=() checks=() farm_jobs=() farm_slots=() sbom=()
  local -a fedora=() live_gates=() unavailable=()

  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --out) [ "$#" -ge 2 ] || die "--out needs a file"; out="$2"; shift 2 ;;
      --source-commit) [ "$#" -ge 2 ] || die "--source-commit needs a SHA"; source_commit="$2"; shift 2 ;;
      --artifact) [ "$#" -ge 2 ] || die "--artifact needs a path"; artifacts+=("$2"); shift 2 ;;
      --check) [ "$#" -ge 2 ] || die "--check needs NAME=STATUS"; checks+=("$2"); shift 2 ;;
      --farm-job) [ "$#" -ge 2 ] || die "--farm-job needs an ID"; farm_jobs+=("$2"); shift 2 ;;
      --farm-slot) [ "$#" -ge 2 ] || die "--farm-slot needs an ID"; farm_slots+=("$2"); shift 2 ;;
      --sbom) [ "$#" -ge 2 ] || die "--sbom needs NAME=STATUS"; sbom+=("$2"); shift 2 ;;
      --fedora-target) [ "$#" -ge 2 ] || die "--fedora-target needs TARGET=STATUS"; fedora+=("$2"); shift 2 ;;
      --live-gate) [ "$#" -ge 2 ] || die "--live-gate needs NAME=STATUS"; live_gates+=("$2"); shift 2 ;;
      --unavailable) [ "$#" -ge 2 ] || die "--unavailable needs text"; unavailable+=("$2"); shift 2 ;;
      --preview-verdict) [ "$#" -ge 2 ] || die "--preview-verdict needs a status"; preview="$2"; shift 2 ;;
      --production-verdict) [ "$#" -ge 2 ] || die "--production-verdict needs a status"; production="$2"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown write argument: $1" ;;
    esac
  done

  [ -n "$out" ] || die "--out is required"
  [ -n "$source_commit" ] || die "--source-commit is required"
  [[ "$source_commit" =~ ^[0-9A-Fa-f]{7,64}$ ]] || die "source commit must be a hexadecimal Git SHA (7-64 characters)"
  [ "${#artifacts[@]}" -gt 0 ] || die "at least one --artifact is required"
  [ "${#checks[@]}" -gt 0 ] || die "at least one --check is required"
  [ "${#farm_jobs[@]}" -gt 0 ] || die "at least one --farm-job is required"
  [ "${#farm_slots[@]}" -gt 0 ] || die "at least one --farm-slot is required"
  [ "${#sbom[@]}" -gt 0 ] || die "at least one --sbom entry is required"
  [ "${#fedora[@]}" -gt 0 ] || die "at least one --fedora-target entry is required"
  [ "${#live_gates[@]}" -gt 0 ] || die "at least one --live-gate entry is required"
  [ -n "$preview" ] || die "--preview-verdict is required"
  [ -n "$production" ] || die "--production-verdict is required"
  valid_verdict "$preview" || die "invalid preview verdict: $preview"
  valid_verdict "$production" || die "invalid production verdict: $production"

  parent="$(dirname -- "$out")"
  [ -d "$parent" ] || die "output parent does not exist: $parent"
  tmp="$(mktemp "$parent/.release-evidence.XXXXXX")"
  trap 'rm -f -- "$tmp"' RETURN

  {
    for arg in "${artifacts[@]}"; do artifact_json "$arg"; done
  } | jq -s 'sort_by(.path)' >"$tmp.artifacts"

  jq -n \
    --argjson schema_version "$SCHEMA_VERSION" \
    --arg source_commit "$source_commit" \
    --slurpfile artifacts "$tmp.artifacts" \
    --argjson checks "$(named_status_json name "${checks[@]}")" \
    --argjson farm_jobs "$(string_array_json "${farm_jobs[@]}")" \
    --argjson farm_slots "$(string_array_json "${farm_slots[@]}")" \
    --argjson sbom "$(named_status_json name "${sbom[@]}")" \
    --argjson fedora_matrix "$(named_status_json target "${fedora[@]}")" \
    --argjson live_gates "$(named_status_json name "${live_gates[@]}")" \
    --argjson unavailable_evidence "$(if [ "${#unavailable[@]}" -gt 0 ]; then string_array_json "${unavailable[@]}"; else printf '[]'; fi)" \
    --arg preview "$preview" \
    --arg production "$production" \
    '{schema_version: $schema_version,
      source_commit: $source_commit,
      artifacts: $artifacts[0],
      checks: $checks,
      farm: {job_ids: $farm_jobs, slot_ids: $farm_slots},
      sbom: $sbom,
      fedora_matrix: $fedora_matrix,
      live_gates: $live_gates,
      unavailable_evidence: $unavailable_evidence,
      verdict: {preview: $preview, production: $production}}' \
    | jq -S . >"$tmp"
  rm -f -- "$tmp.artifacts"
  validate_file "$tmp" >/dev/null
  mv -f -- "$tmp" "$out"
  trap - RETURN
  echo "release-evidence: wrote deterministic evidence to $out"
}

self_test() {
  local work evidence_a evidence_b broken missing_source rc
  work="$(mktemp -d)"
  trap 'rm -rf -- "$work"' RETURN
  printf 'alpha release artifact\n' >"$work/a.rpm"
  printf 'browser release artifact with a second line\n' >"$work/browser.rpm"
  evidence_a="$work/evidence-a.json"
  evidence_b="$work/evidence-b.json"
  "$0" write --out "$evidence_a" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/browser.rpm" --artifact "$work/a.rpm" \
    --check zeta=pass --check alpha=not-run \
    --farm-job farm-job-2 --farm-job farm-job-1 \
    --farm-slot 172.20.0.130/3 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live Dell visual signoff unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null
  "$0" validate "$evidence_a" >/dev/null
  "$0" write --out "$evidence_b" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --artifact "$work/browser.rpm" \
    --check alpha=not-run --check zeta=pass \
    --farm-job farm-job-1 --farm-job farm-job-2 \
    --farm-slot 172.20.0.90/1 --farm-slot 172.20.0.130/3 \
    --sbom rpm=pass --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live Dell visual signoff unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null
  cmp -s "$evidence_a" "$evidence_b" || die "self-test: equivalent inputs were not deterministic"
  jq -e '(.artifacts | length == 2) and (.artifacts[0].path | endswith("/a.rpm"))' "$evidence_a" >/dev/null \
    || die "self-test: artifacts were not sorted"

  broken="$work/broken.json"
  jq 'del(.farm.slot_ids)' "$evidence_a" >"$broken"
  set +e
  "$0" validate "$broken" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing farm slot IDs were accepted"

  missing_source="$work/missing-source.json"
  jq 'del(.source_commit)' "$evidence_a" >"$missing_source"
  set +e
  "$0" validate "$missing_source" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing source commit was accepted"
  echo "release-evidence: self-test passed (determinism + fail-closed validation)"
}

need_commands
case "${1:-}" in
  write) write_evidence "$@" ;;
  validate) [ "$#" -eq 2 ] || die "usage: $0 validate FILE"; validate_file "$2" ;;
  --self-test|self-test) self_test ;;
  -h|--help|"") usage ;;
  *) die "unknown command: $1 (see --help)" ;;
esac
