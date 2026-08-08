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
#     --check github-cargo-test=pass \
#     --farm-job <job-id> --farm-slot <host/slot> \
#     --sbom package=pass --sbom-manifest sbom.json \
#     --gate-manifest gate-manifest.json --ci-gate-status ci-gate-status.json \
#     --resource-publisher-attestation resource-publisher-attestation.json \
#     --vdi-evidence vdi-evidence.json \
#     --fedora-target fedora-44=pass \
#     --live-gate dell-gui=unavailable \
#     --unavailable "live Dell visual signoff not available" \
#     --preview-verdict pass --production-verdict not-promoted
#   install-helpers/release-evidence.sh validate evidence.json
#   install-helpers/release-evidence.sh write-binding --out binding.json \
#     --source-commit <full-git-sha> --ci-gate-status ci-gate-status.json \
#     --artifact path/to/package.rpm [--artifact path/to/another.rpm ...]
#   install-helpers/release-evidence.sh --self-test
#
# Requires: bash, jq, realpath, stat, sha256sum, mktemp.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
LEGACY_SCHEMA_VERSION=4
SCHEMA_VERSION=5
MAX_RELEASE_BINDING_BYTES=$((1024 * 1024))
MAX_CI_GATE_STATUS_BYTES=$((1024 * 1024))

die() {
  echo "release-evidence: $*" >&2
  exit 2
}

usage() {
  sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'USAGE'

Commands:
  write-binding --out FILE --source-commit SHA --ci-gate-status FILE
        --artifact PATH...
  write --out FILE --source-commit SHA --artifact PATH...
        --check GITHUB_CHECK=STATUS... --farm-job ID... --farm-slot ID...
        --sbom NAME=STATUS... --sbom-manifest FILE
        --gate-manifest FILE --ci-gate-status FILE \
        --resource-publisher-attestation FILE \
        --topology-evidence FILE \
        --vdi-evidence FILE \
        --fedora-target TARGET=STATUS...
        --live-gate NAME=STATUS... [--unavailable TEXT...]
        --preview-verdict STATUS --production-verdict STATUS
  validate FILE
  --self-test

Statuses:
  pass | fail | blocked | not-run | unavailable

The production verdict additionally accepts: not-promoted.
--resource-publisher-attestation is optional for preview/not-promoted evidence,
but mandatory for a production pass. It is a detached
ResourcePublisherAttestation JSON envelope carrying the existing HMAC
publisher-attestation:v1 proof; this helper never accepts or invents a secret
key. The --resource-publication-attestation spelling is accepted as an alias.
All required sections must be present and non-empty except
unavailable_evidence, which may be an explicit empty array.
`write-binding` re-verifies the supplied green CI gate status and its sibling
log, derives the exact job/host/slot from that status, and writes the canonical
schema-1 input consumed by `ci-gate.sh bind-release`. It never discovers a
revision, artifact, or farm identity.
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

# Schema 5 keeps the public farm shape as two arrays, but they are parallel:
# both arrays are sorted by job_id and the same index is one job/slot pair.
# Build the arrays together so later independent sorting cannot erase that
# binding. Legacy schema 4 keeps its historical independent set semantics.
farm_pairs_json() {
  local jobs_name="$1" slots_name="$2" index
  local -n jobs="$jobs_name" slots="$slots_name"
  local -a pairs=()
  [ "${#jobs[@]}" -gt 0 ] || die "at least one farm job/slot pair is required"
  [ "${#jobs[@]}" -eq "${#slots[@]}" ] \
    || die "each farm job requires a matching farm slot"
  for index in "${!jobs[@]}"; do
    [ -n "${jobs[index]}" ] || die "farm job identity must not be empty"
    [ -n "${slots[index]}" ] || die "farm slot identity must not be empty"
    pairs+=("$(jq -cn --arg job "${jobs[index]}" --arg slot "${slots[index]}" \
      '{job_id: $job, slot_id: $slot}')")
  done
  printf '%s\n' "${pairs[@]}" | jq -s '
    if (map(.job_id) | unique | length) != length then
      error("duplicate farm job identity")
    elif (map(.slot_id) | unique | length) != length then
      error("duplicate farm slot identity")
    else
      sort_by(.job_id) | {job_ids: map(.job_id), slot_ids: map(.slot_id)}
    end' \
    || die "farm job/slot pairs must be unique"
}

artifact_json() {
  local path="$1" resolved size digest
  [ -f "$path" ] && [ ! -L "$path" ] || die "artifact is not a regular, non-symlink file: $path"
  resolved="$(realpath -e -- "$path")" || die "could not resolve artifact: $path"
  size="$(stat -c '%s' -- "$resolved")" || die "could not stat artifact: $resolved"
  digest="$(sha256sum -- "$resolved" | awk '{print $1}')"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "could not hash artifact: $resolved"
  jq -cn --arg path "$resolved" --arg sha256 "$digest" --argjson size "$size" \
    '{path: $path, size_bytes: $size, sha256: $sha256}'
}

manifest_json() {
  local kind="$1" path="$2" resolved size digest
  [ -s "$path" ] || die "$kind manifest is missing or empty: $path"
  [ -f "$path" ] && [ ! -L "$path" ] || die "$kind manifest is not a regular, non-symlink file: $path"
  resolved="$(realpath -e -- "$path")" || die "could not resolve $kind manifest: $path"
  size="$(stat -c '%s' -- "$resolved")" || die "could not stat $kind manifest: $resolved"
  digest="$(sha256sum -- "$resolved" | awk '{print $1}')"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "could not hash $kind manifest: $resolved"
  jq -cn --arg path "$resolved" --arg sha256 "$digest" --argjson size "$size" \
    '{path: $path, size_bytes: $size, sha256: $sha256}'
}

resource_publisher_attestation_shape() {
  local path="$1"
  jq -e '
    def identifier_ok:
      (type == "string") and (length > 0) and (length <= 255) and
      test("^[A-Za-z0-9._:/@+-]+$") and
      (startswith("/") | not) and (endswith("/") | not) and
      (contains("//") | not) and
      (split("/") | all(.[]; . != "." and . != ".."));
    (type == "object") and
    (keys == ["catalog_content_digest", "expires_at_ms", "issued_at_ms", "key_id", "publisher", "schema_version", "signature"]) and
    (.schema_version | type == "number" and floor == . and . == 1) and
    (.publisher | identifier_ok) and
    (.key_id | type == "string" and . == "resource-publisher-hmac-v1") and
    (.catalog_content_digest | type == "string" and test("^catalog:v1:[0-9a-f]{64}$")) and
    (.issued_at_ms | type == "number" and floor == . and . > 0) and
    (.expires_at_ms | type == "number" and floor == . and . > 0) and
    (.expires_at_ms > .issued_at_ms) and
    ((.expires_at_ms - .issued_at_ms) >= 1000) and
    ((.expires_at_ms - .issued_at_ms) <= 604800000) and
    (.signature | type == "string" and test("^publisher-attestation:v1:[0-9a-f]{64}$"))
  ' "$path" >/dev/null
}

resource_publisher_attestation_json() {
  local path="$1" descriptor attestation
  [ -s "$path" ] || die "resource publisher attestation is missing or empty: $path"
  [ -f "$path" ] && [ ! -L "$path" ] || die "resource publisher attestation is not a regular, non-symlink file: $path"
  resource_publisher_attestation_shape "$path" \
    || die "resource publisher attestation is not a valid HMAC publisher-attestation:v1 envelope: $path"
  descriptor="$(manifest_json "resource publisher attestation" "$path")"
  attestation="$(jq -cS . "$path")" \
    || die "resource publisher attestation is not valid JSON: $path"
  jq -cn --argjson descriptor "$descriptor" --argjson attestation "$attestation" \
    '$descriptor + $attestation'
}

binding_payload() {
  jq -cS '{source_commit,
    artifacts,
    sbom_manifest: .provenance.sbom_manifest,
    gate_manifest: .provenance.gate_manifest,
    ci_gate_status: .provenance.ci_gate_status,
    topology_evidence: .provenance.topology_evidence,
    vdi_evidence: .provenance.vdi_evidence,
    gates: {checks, farm, sbom, fedora_matrix, live_gates,
      unavailable_evidence, verdict}} +
    (if .schema_version == 5 then
       {resource_publisher_attestation: .provenance.resource_publisher_attestation}
     else
       {}
     end)' "$1"
}

# The ci-gate status proves a revision/job/host/slot run and authenticates its
# sibling log by digest.  Bind that required-check proof to the exact release
# artifact descriptors as a line inside the authenticated log; otherwise the
# same green job can be attached to different bytes from the same revision.
required_check_binding_payload_values() {
  local source_commit="$1" artifacts="$2" job_id="$3" build_host="$4" build_slot="$5"
  jq -cnS \
    --arg source_commit "$source_commit" \
    --argjson artifacts "$artifacts" \
    --arg job_id "$job_id" \
    --arg build_host "$build_host" \
    --arg build_slot "$build_slot" \
    '{schema_version: 1,
      source_commit: $source_commit,
      artifacts: $artifacts,
      farm: {job_id: $job_id, build_host: $build_host, build_slot: $build_slot}}'
}

required_check_binding_shape() {
  jq -e '
    def identity:
      (type == "string") and (length > 0) and (length <= 255) and
      test("^[^[:space:][:cntrl:]]+$");
    (type == "object") and
    (keys == ["artifacts", "farm", "schema_version", "source_commit"]) and
    (.schema_version == 1) and
    (.source_commit | type == "string" and test("^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")) and
    (.artifacts | type == "array" and length > 0 and length <= 1024 and
      all(.[];
        (type == "object") and
        (keys == ["path", "sha256", "size_bytes"]) and
        (.path | type == "string" and length > 0 and length <= 4096 and
          (test("[[:cntrl:]]") | not)) and
        (.size_bytes | type == "number" and floor == . and . >= 0) and
        (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))) and
      (. == (sort_by(.path))) and
      ((map(.path) | unique | length) == length)) and
    (.farm | type == "object" and
      (keys == ["build_host", "build_slot", "job_id"]) and
      (.job_id | identity) and
      (.build_host | identity) and
      (.build_slot | identity))
  ' "$1" >/dev/null
}

write_release_binding() {
  local out="" source_commit="" ci_gate_status="" parent tmp status_size
  local status_before status_after artifacts_json job_id build_host build_slot output_size arg
  local -a artifacts=()

  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --out)
        [ "$#" -ge 2 ] || die "--out needs a file"
        [ -z "$out" ] || die "--out may be supplied only once"
        out="$2"
        shift 2
        ;;
      --source-commit)
        [ "$#" -ge 2 ] || die "--source-commit needs a SHA"
        [ -z "$source_commit" ] || die "--source-commit may be supplied only once"
        source_commit="$2"
        shift 2
        ;;
      --ci-gate-status)
        [ "$#" -ge 2 ] || die "--ci-gate-status needs a file"
        [ -z "$ci_gate_status" ] || die "--ci-gate-status may be supplied only once"
        ci_gate_status="$2"
        shift 2
        ;;
      --artifact)
        [ "$#" -ge 2 ] || die "--artifact needs a path"
        artifacts+=("$2")
        shift 2
        ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown write-binding argument: $1" ;;
    esac
  done

  [ -n "$out" ] || die "--out is required"
  [ -n "$source_commit" ] || die "--source-commit is required"
  [[ "$source_commit" =~ ^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$ ]] \
    || die "source commit must be an exact 40- or 64-character Git object ID"
  [ -n "$ci_gate_status" ] || die "--ci-gate-status is required"
  [ "${#artifacts[@]}" -gt 0 ] || die "at least one --artifact is required"
  [ "${#artifacts[@]}" -le 1024 ] || die "at most 1024 --artifact entries are accepted"
  [ -f "$ci_gate_status" ] && [ ! -L "$ci_gate_status" ] \
    || die "ci-gate status is not a regular, non-symlink file: $ci_gate_status"
  status_size="$(stat -c '%s' -- "$ci_gate_status" 2>/dev/null || true)"
  [[ "$status_size" =~ ^[0-9]+$ ]] && [ "$status_size" -gt 0 ] \
    && [ "$status_size" -le "$MAX_CI_GATE_STATUS_BYTES" ] \
    || die "ci-gate status must be 1..$MAX_CI_GATE_STATUS_BYTES bytes: $ci_gate_status"
  [ -x "$SCRIPT_DIR/ci-gate.sh" ] || die "ci-gate.sh is unavailable"

  status_before="$(sha256sum -- "$ci_gate_status" | awk '{print $1}')"
  "$SCRIPT_DIR/ci-gate.sh" verify "$ci_gate_status" \
    --expected-revision "$source_commit" >/dev/null \
    || die "ci-gate status is not an unchanged verified green result: $ci_gate_status"
  job_id="$(jq -er '.job_id' "$ci_gate_status")" \
    || die "ci-gate status has no valid job identity: $ci_gate_status"
  build_host="$(jq -er '.build_host' "$ci_gate_status")" \
    || die "ci-gate status has no valid build host: $ci_gate_status"
  build_slot="$(jq -er '.build_slot' "$ci_gate_status")" \
    || die "ci-gate status has no valid build slot: $ci_gate_status"
  status_after="$(sha256sum -- "$ci_gate_status" | awk '{print $1}')"
  [ "$status_before" = "$status_after" ] \
    || die "ci-gate status changed while release binding was prepared: $ci_gate_status"

  parent="$(dirname -- "$out")"
  [ -d "$parent" ] && [ ! -L "$parent" ] || die "output parent is not a non-symlink directory: $parent"
  if [ -e "$out" ] || [ -L "$out" ]; then
    [ -f "$out" ] && [ ! -L "$out" ] \
      || die "binding output is not a regular, non-symlink file: $out"
  fi
  tmp="$(mktemp "$parent/.release-binding.XXXXXX")"
  trap 'rm -f -- "$tmp" "$tmp.artifacts"' RETURN

  {
    for arg in "${artifacts[@]}"; do artifact_json "$arg"; done
  } | jq -cS -s 'sort_by(.path)' >"$tmp.artifacts"
  artifacts_json="$(<"$tmp.artifacts")"
  required_check_binding_payload_values \
    "$source_commit" "$artifacts_json" "$job_id" "$build_host" "$build_slot" >"$tmp"
  required_check_binding_shape "$tmp" \
    || die "generated release binding is malformed, unsorted, or contains duplicate artifacts"
  output_size="$(stat -c '%s' -- "$tmp")"
  [ "$output_size" -gt 0 ] && [ "$output_size" -le "$MAX_RELEASE_BINDING_BYTES" ] \
    || die "generated release binding must be 1..$MAX_RELEASE_BINDING_BYTES bytes"
  chmod 0644 -- "$tmp"
  [ ! -L "$out" ] || die "binding output became a symlink while it was prepared: $out"
  mv -f -- "$tmp" "$out"
  rm -f -- "$tmp.artifacts"
  trap - RETURN
  echo "release-evidence: wrote canonical release binding to $out"
}

required_check_binding_payload() {
  local evidence="$1" ci_gate="$2"
  required_check_binding_payload_values \
    "$(jq -r '.source_commit' "$evidence")" \
    "$(jq -cS '.artifacts' "$evidence")" \
    "$(jq -r '.job_id' "$ci_gate")" \
    "$(jq -r '.build_host' "$ci_gate")" \
    "$(jq -r '.build_slot' "$ci_gate")"
}

verify_required_check_binding() {
  local evidence="$1" ci_gate="$2" log expected binding_line binding_count
  log="$(dirname -- "$ci_gate")/$(jq -r '.evidence.gate_log.path' "$ci_gate")"
  expected="$(required_check_binding_payload "$evidence" "$ci_gate" | sha256sum | awk '{print $1}')"
  binding_line="  release-evidence-binding sha256=$expected"
  binding_count="$(grep -Ec '^  release-evidence-binding sha256=[0-9a-f]{64}$' "$log" || true)"
  [ "$binding_count" -eq 1 ] \
    || die "ci-gate log must contain exactly one release artifact binding: $log"
  [ "$(grep -Fxc -- "$binding_line" "$log" || true)" -eq 1 ] \
    || die "ci-gate required-check binding does not match release revision, artifacts, and farm job/host/slot: $log"
}

verify_descriptor() {
  local kind="$1" descriptor="$2" path expected_size expected_sha256 actual_size actual_sha256
  path="$(jq -r '.path' <<<"$descriptor")"
  expected_size="$(jq -r '.size_bytes' <<<"$descriptor")"
  expected_sha256="$(jq -r '.sha256' <<<"$descriptor")"
  [ -s "$path" ] || die "$kind manifest is missing or empty: $path"
  [ -f "$path" ] && [ ! -L "$path" ] || die "$kind manifest is not a regular, non-symlink file: $path"
  actual_size="$(stat -c '%s' -- "$path")" || die "could not stat $kind manifest: $path"
  actual_sha256="$(sha256sum -- "$path" | awk '{print $1}')"
  [ "$actual_size" = "$expected_size" ] \
    || die "$kind manifest changed size since evidence was written: $path"
  [ "$actual_sha256" = "$expected_sha256" ] \
    || die "$kind manifest digest does not match evidence: $path"
}

verify_resource_publisher_attestation() {
  local descriptor="$1" path expected_attestation actual_attestation
  path="$(jq -r '.path' <<<"$descriptor")"
  verify_descriptor "resource publisher attestation" \
    "$(jq -c '{path, sha256, size_bytes}' <<<"$descriptor")"
  resource_publisher_attestation_shape "$path" \
    || die "resource publisher attestation is not a valid HMAC publisher-attestation:v1 envelope: $path"
  expected_attestation="$(jq -cS 'del(.path, .sha256, .size_bytes)' <<<"$descriptor")"
  actual_attestation="$(jq -cS . "$path")" \
    || die "resource publisher attestation is not valid JSON: $path"
  [ "$actual_attestation" = "$expected_attestation" ] \
    || die "resource publisher attestation descriptor does not match its file: $path"
}

validate_file() {
  local file="$1"
  local -a topology_verify_args=()
  [ -f "$file" ] && [ ! -L "$file" ] || die "evidence file is not a regular, non-symlink file: $file"
  jq -e \
    --arg source_commit "$(jq -r '.source_commit' "$file")" \
    --argjson schema_version "$SCHEMA_VERSION" \
    --argjson legacy_schema_version "$LEGACY_SCHEMA_VERSION" '
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
    def nonempty_unique_strings:
      (type == "array") and (length > 0) and
      all(.[]; (type == "string") and (length > 0)) and
      ((map(.) | unique | length) == length);
    def sorted_unique_named($field):
      (type == "array") and (length > 0) and
      all(.[];
        (type == "object") and
        (keys == ([$field, "status"] | sort)) and
        (.[$field] | type == "string" and length > 0) and
        (.status | status_ok)) and
      (. == (sort_by(.[$field]))) and
      ((map(.[$field]) | unique | length) == length);
    def exact_source_commit:
      (type == "string") and
      test("^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$");
    def farm_host_slot:
      (type == "string") and
      test("^[^/[:space:][:cntrl:]]+/[^/[:space:][:cntrl:]]+$");
    def ci_farm_slots_ok:
      (.provenance.ci_gate_status == null) or
      all(.farm.slot_ids[]; farm_host_slot);
    def resource_publisher_attestation_descriptor_ok:
      (type == "object") and
      (keys == ["catalog_content_digest", "expires_at_ms", "issued_at_ms", "key_id", "path", "publisher", "schema_version", "sha256", "signature", "size_bytes"]) and
      (.path | type == "string" and length > 0) and
      (.size_bytes | type == "number" and floor == . and . > 0) and
      (.sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
      (.schema_version | type == "number" and floor == . and . == 1) and
      (.publisher | type == "string" and length > 0 and length <= 255 and test("^[A-Za-z0-9._:/@+-]+$")) and
      (.key_id | type == "string" and . == "resource-publisher-hmac-v1") and
      (.catalog_content_digest | type == "string" and test("^catalog:v1:[0-9a-f]{64}$")) and
      (.issued_at_ms | type == "number" and floor == . and . > 0) and
      (.expires_at_ms | type == "number" and floor == . and . > 0) and
      (.expires_at_ms > .issued_at_ms) and
      ((.expires_at_ms - .issued_at_ms) >= 1000) and
      ((.expires_at_ms - .issued_at_ms) <= 604800000) and
      (.signature | type == "string" and test("^publisher-attestation:v1:[0-9a-f]{64}$"));
    def production_verdict_ok:
      # Production promotion is valid only when every declared release gate is
      # an observed pass, no live or hardware evidence is unavailable, and a
      # current authenticated resource-publication proof is attached.
      (.verdict.production != "pass") or
      (
        (.checks | all(.[]; .status == "pass")) and
        (.sbom | all(.[]; .status == "pass")) and
        (.fedora_matrix | all(.[]; .status == "pass")) and
        (.live_gates | all(.[]; .status == "pass")) and
        (.unavailable_evidence | length == 0) and
        (any(.checks[]; .name == "github-required" and .status == "pass")) and
        (.provenance.ci_gate_status | type == "object") and
        (.provenance.topology_evidence | type == "object") and
        (.provenance.topology_evidence.verification.live_required == true) and
        (.provenance.topology_evidence.verification.revision == .source_commit) and
        (.provenance.vdi_evidence | type == "object") and
        (.provenance.resource_publisher_attestation | resource_publisher_attestation_descriptor_ok) and
        (.provenance.resource_publisher_attestation.issued_at_ms <= (now * 1000 | floor)) and
        (.provenance.resource_publisher_attestation.expires_at_ms > (now * 1000 | floor))
      );

    (type == "object") and
    (keys == ["artifacts", "checks", "farm", "fedora_matrix", "live_gates", "provenance", "sbom", "schema_version", "source_commit", "unavailable_evidence", "verdict"]) and
    ((.schema_version == $schema_version) or
      (.schema_version == $legacy_schema_version and .verdict.production != "pass")) and
    (.source_commit | exact_source_commit) and
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
      (.slot_ids | nonempty_unique_strings) and
      ((.job_ids | length) == (.slot_ids | length))) and
    (if .schema_version == $legacy_schema_version then
       (.farm.slot_ids == (.farm.slot_ids | sort | unique))
     else
       true
     end) and
    ci_farm_slots_ok and
    (.sbom | sorted_unique_named("name")) and
    (.fedora_matrix | sorted_unique_named("target")) and
    (.live_gates | sorted_unique_named("name")) and
    (.unavailable_evidence | type == "array" and
      all(.[]; type == "string" and length > 0) and
      (. == (sort | unique))) and
    (if .schema_version == $schema_version then
       (.provenance | type == "object" and
        (keys == ["binding_sha256", "ci_gate_status", "gate_manifest", "resource_publisher_attestation", "sbom_manifest", "schema_version", "topology_evidence", "vdi_evidence"]) and
        (.schema_version == 1) and
        (.binding_sha256 | type == "string" and test("^[0-9a-f]{64}$")))
     elif .schema_version == $legacy_schema_version then
       (.provenance | type == "object" and
        (keys == ["binding_sha256", "ci_gate_status", "gate_manifest", "sbom_manifest", "schema_version", "topology_evidence", "vdi_evidence"]) and
        (.schema_version == 1) and
        (.binding_sha256 | type == "string" and test("^[0-9a-f]{64}$")))
     else
       false
     end) and
    ([.provenance.sbom_manifest, .provenance.gate_manifest] |
      all(.[];
        (type == "object") and (keys == ["path", "sha256", "size_bytes"]) and
        (.path | type == "string" and length > 0) and
        (.size_bytes | type == "number" and floor == . and . > 0) and
        (.sha256 | type == "string" and test("^[0-9a-f]{64}$")))) and
    ((.provenance.ci_gate_status == null) or
      ((.provenance.ci_gate_status | type == "object") and
       (.provenance.ci_gate_status | keys == ["path", "sha256", "size_bytes"]) and
       (.provenance.ci_gate_status.path | type == "string" and length > 0) and
       (.provenance.ci_gate_status.size_bytes | type == "number" and floor == . and . > 0) and
       (.provenance.ci_gate_status.sha256 | type == "string" and test("^[0-9a-f]{64}$")))) and
    ((.provenance.topology_evidence == null) or
      ((.provenance.topology_evidence | type == "object") and
       (.provenance.topology_evidence | keys == ["path", "sha256", "size_bytes", "verification"]) and
       (.provenance.topology_evidence.path | type == "string" and length > 0) and
       (.provenance.topology_evidence.size_bytes | type == "number" and floor == . and . > 0) and
       (.provenance.topology_evidence.sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
       (.provenance.topology_evidence.verification | type == "object" and .node_count == 6 and
        (.live_required | type == "boolean") and .revision == $source_commit))) and
    ((.provenance.vdi_evidence == null) or
      ((.provenance.vdi_evidence | type == "object") and
       (.provenance.vdi_evidence | keys == ["path", "sha256", "size_bytes"]) and
       (.provenance.vdi_evidence.path | type == "string" and length > 0) and
       (.provenance.vdi_evidence.size_bytes | type == "number" and floor == . and . > 0) and
       (.provenance.vdi_evidence.sha256 | type == "string" and test("^[0-9a-f]{64}$")))) and
    ((.provenance.resource_publisher_attestation == null) or
      (.provenance.resource_publisher_attestation | resource_publisher_attestation_descriptor_ok)) and
    (.verdict | type == "object" and (keys == ["preview", "production"]) and
      (.preview | verdict_ok) and (.production | verdict_ok))
    and production_verdict_ok
  ' "$file" >/dev/null || die "invalid or incomplete evidence schema: $file"
  verify_descriptor "SBOM" "$(jq -c '.provenance.sbom_manifest' "$file")"
  verify_descriptor "gate" "$(jq -c '.provenance.gate_manifest' "$file")"
  if [ "$(jq -r '.provenance.resource_publisher_attestation // empty' "$file")" ]; then
    verify_resource_publisher_attestation \
      "$(jq -c '.provenance.resource_publisher_attestation' "$file")"
  elif [ "$(jq -r '.verdict.production' "$file")" = "pass" ]; then
    die "production pass requires an authenticated resource publisher attestation descriptor"
  fi
  if [ "$(jq -r '.provenance.topology_evidence // empty' "$file")" ]; then
    local topology_path
    topology_path="$(jq -r '.provenance.topology_evidence.path' "$file")"
    verify_descriptor "topology evidence" "$(jq -c '.provenance.topology_evidence | del(.verification)' "$file")"
    [ -x "$SCRIPT_DIR/verify-six-node-topology.py" ] || die "six-node topology verifier is unavailable"
    topology_verify_args=()
    if [ "$(jq -r '.verdict.production' "$file")" = "pass" ]; then
      topology_verify_args+=(--require-live --expected-revision "$(jq -r '.source_commit' "$file")")
    fi
    "$SCRIPT_DIR/verify-six-node-topology.py" --evidence "$topology_path" "${topology_verify_args[@]}" >/dev/null \
      || die "topology evidence is not a verified live six-node result: $topology_path"
  fi
  if [ "$(jq -r '.provenance.ci_gate_status // empty' "$file")" ]; then
    local ci_gate
    ci_gate="$(jq -r '.provenance.ci_gate_status.path' "$file")"
    verify_descriptor "ci-gate status" "$(jq -c '.provenance.ci_gate_status' "$file")"
    [ -x "$SCRIPT_DIR/ci-gate.sh" ] || die "ci-gate.sh is unavailable"
    "$SCRIPT_DIR/ci-gate.sh" verify "$ci_gate" --expected-revision "$(jq -r '.source_commit' "$file")" >/dev/null \
      || die "ci-gate status is not a verified green farm result: $ci_gate"
    [ "$(jq -r '.sha' "$ci_gate")" = "$(jq -r '.source_commit' "$file")" ] \
      || die "ci-gate status revision does not match release source commit"
    if [ "$(jq -r '.schema_version' "$file")" = "$SCHEMA_VERSION" ]; then
      jq -e --arg job "$(jq -r '.job_id' "$ci_gate")" \
        --arg slot "$(jq -r '.build_host' "$ci_gate")/$(jq -r '.build_slot' "$ci_gate")" \
        '(.farm.job_ids | index($job)) as $job_index |
         (.farm.slot_ids | index($slot)) as $slot_index |
         ($job_index != null and $slot_index != null and $job_index == $slot_index)' \
        "$file" >/dev/null \
        || die "ci-gate status job/slot pair is not bound to one release farm pair"
      verify_required_check_binding "$file" "$ci_gate"
    else
      jq -e --arg job "$(jq -r '.job_id' "$ci_gate")" \
        --arg slot "$(jq -r '.build_host' "$ci_gate")/$(jq -r '.build_slot' "$ci_gate")" \
        '(.farm.job_ids | index($job)) and (.farm.slot_ids | index($slot))' \
        "$file" >/dev/null \
        || die "ci-gate status job/slot is not listed in release farm evidence"
    fi
  fi
  if [ "$(jq -r '.provenance.vdi_evidence // empty' "$file")" ]; then
    local vdi_evidence vdi_source_commit vdi_status
    vdi_evidence="$(jq -r '.provenance.vdi_evidence.path' "$file")"
    verify_descriptor "VDI evidence" "$(jq -c '.provenance.vdi_evidence' "$file")"
    [ -x "$SCRIPT_DIR/verify-vdi-live-proof.py" ] || die "VDI proof verifier is unavailable"
    "$SCRIPT_DIR/verify-vdi-live-proof.py" validate "$vdi_evidence" >/dev/null \
      || die "VDI evidence is not a valid bounded proof: $vdi_evidence"
    vdi_status="$(jq -r '.status' "$vdi_evidence")"
    vdi_source_commit="$(jq -r '.source_commit // empty' "$vdi_evidence")"
    if [ "$(jq -r '.verdict.production' "$file")" = "pass" ]; then
      [ "$vdi_status" = observed ] || die "production evidence requires an observed VDI framebuffer"
      [ "$vdi_source_commit" = "$(jq -r '.source_commit' "$file")" ] \
        || die "VDI evidence source commit does not match release source commit"
    fi
  elif [ "$(jq -r '.verdict.production' "$file")" = "pass" ]; then
    die "production pass requires --vdi-evidence with observed guest framebuffer proof"
  fi
  local actual_binding expected_binding
  actual_binding="$(binding_payload "$file" | sha256sum | awk '{print $1}')"
  expected_binding="$(jq -r '.provenance.binding_sha256' "$file")"
  [ "$actual_binding" = "$expected_binding" ] \
    || die "provenance binding digest does not match evidence: $file"
  echo "release-evidence: valid $file"
}

write_evidence() {
  local out="" source_commit="" preview="" production="" sbom_manifest="" gate_manifest="" ci_gate_status="" resource_publisher_attestation="" topology_evidence="" vdi_evidence="" arg parent tmp binding topology_verification topology_descriptor farm
  local -a topology_verify_args=()
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
      --sbom-manifest) [ "$#" -ge 2 ] || die "--sbom-manifest needs a file"; sbom_manifest="$2"; shift 2 ;;
      --gate-manifest) [ "$#" -ge 2 ] || die "--gate-manifest needs a file"; gate_manifest="$2"; shift 2 ;;
      --ci-gate-status) [ "$#" -ge 2 ] || die "--ci-gate-status needs a file"; ci_gate_status="$2"; shift 2 ;;
      --resource-publisher-attestation|--resource-publication-attestation)
        [ "$#" -ge 2 ] || die "$1 needs a file"
        resource_publisher_attestation="$2"
        shift 2
        ;;
      --topology-evidence) [ "$#" -ge 2 ] || die "--topology-evidence needs a file"; topology_evidence="$2"; shift 2 ;;
      --vdi-evidence) [ "$#" -ge 2 ] || die "--vdi-evidence needs a file"; vdi_evidence="$2"; shift 2 ;;
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
  [[ "$source_commit" =~ ^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$ ]] \
    || die "source commit must be an exact 40- or 64-character Git object ID"
  [ "${#artifacts[@]}" -gt 0 ] || die "at least one --artifact is required"
  [ "${#checks[@]}" -gt 0 ] || die "at least one --check is required"
  [ "${#farm_jobs[@]}" -gt 0 ] || die "at least one --farm-job is required"
  [ "${#farm_slots[@]}" -gt 0 ] || die "at least one --farm-slot is required"
  [ "${#farm_jobs[@]}" -eq "${#farm_slots[@]}" ] \
    || die "each --farm-job requires a matching --farm-slot"
  farm="$(farm_pairs_json farm_jobs farm_slots)"
  [ "${#sbom[@]}" -gt 0 ] || die "at least one --sbom entry is required"
  [ -n "$sbom_manifest" ] || die "--sbom-manifest is required"
  [ -n "$gate_manifest" ] || die "--gate-manifest is required"
  [ "${#fedora[@]}" -gt 0 ] || die "at least one --fedora-target entry is required"
  [ "${#live_gates[@]}" -gt 0 ] || die "at least one --live-gate entry is required"
  [ -n "$preview" ] || die "--preview-verdict is required"
  [ -n "$production" ] || die "--production-verdict is required"
  valid_verdict "$preview" || die "invalid preview verdict: $preview"
  valid_verdict "$production" || die "invalid production verdict: $production"
  if [ "$production" = "pass" ] && [ -z "$topology_evidence" ]; then
    die "production pass requires --topology-evidence with live six-node proof"
  fi
  if [ "$production" = "pass" ] && [ -z "$resource_publisher_attestation" ]; then
    die "production pass requires --resource-publisher-attestation with an authenticated resource-publication proof"
  fi

  parent="$(dirname -- "$out")"
  [ -d "$parent" ] || die "output parent does not exist: $parent"
  tmp="$(mktemp "$parent/.release-evidence.XXXXXX")"
  trap 'rm -f -- "$tmp"' RETURN

  {
    for arg in "${artifacts[@]}"; do artifact_json "$arg"; done
  } | jq -s 'sort_by(.path)' >"$tmp.artifacts"
  manifest_json "SBOM" "$sbom_manifest" >"$tmp.sbom"
  manifest_json "gate" "$gate_manifest" >"$tmp.gate"
  if [ -n "$ci_gate_status" ]; then
    manifest_json "ci-gate status" "$ci_gate_status" >"$tmp.ci-gate"
  else
    printf 'null\n' >"$tmp.ci-gate"
  fi
  if [ -n "$resource_publisher_attestation" ]; then
    resource_publisher_attestation_json "$resource_publisher_attestation" >"$tmp.resource-publisher-attestation"
  else
    printf 'null\n' >"$tmp.resource-publisher-attestation"
  fi
  if [ -n "$topology_evidence" ]; then
    manifest_json "topology evidence" "$topology_evidence" >"$tmp.topology"
    [ -x "$SCRIPT_DIR/verify-six-node-topology.py" ] || die "six-node topology verifier is unavailable"
    if [ "$production" = "pass" ]; then
      topology_verify_args+=(--require-live --expected-revision "$source_commit")
    fi
    topology_verification="$("$SCRIPT_DIR/verify-six-node-topology.py" --evidence "$(realpath -e -- "$topology_evidence")" "${topology_verify_args[@]}" --json)" \
      || die "topology evidence is not a verified live six-node result: $topology_evidence"
    topology_descriptor="$(jq -c --argjson verification "$topology_verification" '. + {verification: $verification}' "$tmp.topology")"
  else
    topology_descriptor="null"
  fi
  if [ -n "$vdi_evidence" ]; then
    manifest_json "VDI evidence" "$vdi_evidence" >"$tmp.vdi"
  else
    printf 'null\n' >"$tmp.vdi"
  fi

  jq -n \
    --argjson schema_version "$SCHEMA_VERSION" \
    --arg source_commit "$source_commit" \
    --slurpfile artifacts "$tmp.artifacts" \
    --slurpfile sbom_manifest "$tmp.sbom" \
    --slurpfile gate_manifest "$tmp.gate" \
    --slurpfile ci_gate_status "$tmp.ci-gate" \
    --slurpfile resource_publisher_attestation "$tmp.resource-publisher-attestation" \
    --slurpfile vdi_evidence "$tmp.vdi" \
    --argjson topology_evidence "$topology_descriptor" \
    --argjson checks "$(named_status_json name "${checks[@]}")" \
    --argjson farm "$farm" \
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
      farm: $farm,
      sbom: $sbom,
      fedora_matrix: $fedora_matrix,
      live_gates: $live_gates,
      unavailable_evidence: $unavailable_evidence,
      verdict: {preview: $preview, production: $production},
      provenance: {schema_version: 1, binding_sha256: "",
        sbom_manifest: $sbom_manifest[0], gate_manifest: $gate_manifest[0],
        ci_gate_status: $ci_gate_status[0],
        resource_publisher_attestation: $resource_publisher_attestation[0],
        topology_evidence: $topology_evidence,
        vdi_evidence: $vdi_evidence[0]}}' \
    | jq -S . >"$tmp"
  rm -f -- "$tmp.artifacts"
  rm -f -- "$tmp.sbom" "$tmp.gate" "$tmp.ci-gate"
  rm -f -- "$tmp.resource-publisher-attestation"
  rm -f -- "$tmp.vdi"
  rm -f -- "$tmp.topology"
  binding="$(binding_payload "$tmp" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' "$tmp" >"$tmp.bound"
  mv -f -- "$tmp.bound" "$tmp"
  validate_file "$tmp" >/dev/null
  mv -f -- "$tmp" "$out"
  trap - RETURN
  echo "release-evidence: wrote deterministic evidence to $out"
}

self_test_ci_status() {
  local dir="$1" artifacts="$2" source_commit="$3" job_id="$4" build_host="$5" build_slot="$6"
  local include_binding="${7:-yes}" status log binding="" digest
  case "$include_binding" in
    yes|no) ;;
    *) die "self-test: invalid binding fixture mode: $include_binding" ;;
  esac
  mkdir -p -- "$dir"
  status="$dir/ci-gate-status.json"
  log="$dir/ci-gate.log"
  if [ "$include_binding" = yes ]; then
    binding="$(required_check_binding_payload_values \
      "$source_commit" "$artifacts" "$job_id" "$build_host" "$build_slot" \
      | sha256sum | awk '{print $1}')"
  fi
  {
    printf '%s\n' 'MCNF CI gate — self-test'
    printf '  sha=%s  revision=%s  job_id=%s  host=%s (slot=%s)\n' \
      "${source_commit:0:7}" "$source_commit" "$job_id" "$build_host" "$build_slot"
    printf '%s\n' '=== CI GATE SUMMARY self-test → green ==='
    printf '%s\n' \
      '  policy   pass' \
      '  fmt      pass' \
      '  clippy   pass' \
      '  test     pass  (1 passed, 0 failed)' \
      '  coverage pass'
    if [ "$include_binding" = yes ]; then
      printf '  release-evidence-binding sha256=%s\n' "$binding"
    fi
  } >"$log"
  digest="$(sha256sum -- "$log" | awk '{print $1}')"
  jq -n \
    --arg sha "$source_commit" \
    --arg job_id "$job_id" \
    --arg build_host "$build_host" \
    --arg build_slot "$build_slot" \
    --arg digest "$digest" \
    '{overall:"green",alert:false,failed_stage:"",
      stages:{policy:"pass",fmt:"pass",clippy:"pass",test:"pass",coverage:"pass"},
      tests_passed:1,tests_failed:0,sha:$sha,short_sha:($sha[0:7]),
      job_id:$job_id,build_host:$build_host,build_slot:$build_slot,
      evidence:{revision:$sha,gate_log:{path:"ci-gate.log",sha256:$digest}},
      started:"self-test",finished:"self-test",source:"ci-gate"}' \
    >"$status"
  printf '%s\n' "$status"
}

self_test() {
  local work evidence_a evidence_b broken broken_farm short_source invalid_farm_slot ci_pair_mismatch production_pass topology farm_topology preview_farm topology_revision_mismatch topology_revision_mismatch_raw missing_ci_gate missing_github_check failed_gate unavailable_pass missing_source changed_sbom changed_gate legacy_preview legacy_production missing_resource_attestation invalid_resource_attestation descriptor_mismatch expected_a_sha expected_binding symlink_artifact reused_ci_artifact replacement_descriptor rc
  local ci_status ci_status_single vdi_evidence resource_attestation attestation_issued_ms attestation_expires_ms two_artifacts single_artifact
  local roundtrip_status roundtrip_binding roundtrip_binding_reordered roundtrip_evidence
  local hostile_status hostile_status_changed hostile_output duplicate_status duplicate_binding
  local hostile_descriptor_status hostile_descriptor_binding hostile_descriptor_evidence
  local missing_binding symlink_binding_artifact malformed_status
  work="$(mktemp -d)"
  trap 'rm -rf -- "$work"' RETURN
  printf 'alpha release artifact\n' >"$work/a.rpm"
  printf 'browser release artifact with a second line\n' >"$work/browser.rpm"
  printf '{"packages":["magic-mesh"]}\n' >"$work/sbom.json"
  printf '{"required":["github-policy","fedora-44"]}\n' >"$work/gates.json"
  vdi_evidence="$work/vdi-evidence.json"
  cat >"$vdi_evidence" <<'EOF'
{"frame":{"fnv1a64":"0x0123456789abcdef","height":768,"width":1024},"image_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","input_observation":"echoed","probe":{"log_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","returncode":0,"source":"mde-shell-egui ignored live worker test"},"protocol":"vnc","recorded_at":"2026-08-01T00:00:00Z","schema_version":1,"source_commit":"0123456789abcdef0123456789abcdef01234567","status":"observed","target":{"host":"127.0.0.1","port":15903}}
EOF
  resource_attestation="$work/resource-publisher-attestation.json"
  attestation_issued_ms="$(jq -nr '((now * 1000) | floor)')"
  attestation_expires_ms=$((attestation_issued_ms + 300000))
  # Structural fixture only: no live secret or key material is invented here.
  # Cryptographic HMAC verification remains the trusted key-store consumer's job.
  jq -n --argjson issued_at_ms "$attestation_issued_ms" --argjson expires_at_ms "$attestation_expires_ms" '
    {schema_version: 1,
     publisher: "self-test-publisher",
     key_id: "resource-publisher-hmac-v1",
     catalog_content_digest: ("catalog:v1:" + ("a" * 64)),
     issued_at_ms: $issued_at_ms,
     expires_at_ms: $expires_at_ms,
     signature: ("publisher-attestation:v1:" + ("0" * 64))}' \
    >"$resource_attestation"
  expected_a_sha="$(sha256sum -- "$work/a.rpm" | awk '{print $1}')"
  two_artifacts="$({ artifact_json "$work/browser.rpm"; artifact_json "$work/a.rpm"; } | jq -s 'sort_by(.path)')"
  single_artifact="$(artifact_json "$work/a.rpm" | jq -s 'sort_by(.path)')"
  ci_status="$(self_test_ci_status "$work/ci-two-artifacts" "$two_artifacts" \
    0123456789abcdef0123456789abcdef01234567 farm-job-1 172.20.0.90 1)"
  ci_status_single="$(self_test_ci_status "$work/ci-one-artifact" "$single_artifact" \
    0123456789abcdef0123456789abcdef01234567 farm-job-1 172.20.0.90 1)"

  # Production orchestration round-trip: generate the exact final descriptor
  # input, let ci-gate authenticate it, then prove release evidence consumes the
  # refreshed status and the same artifact bytes.
  roundtrip_status="$(self_test_ci_status "$work/ci-binding-roundtrip" "$two_artifacts" \
    0123456789abcdef0123456789abcdef01234567 farm-job-binding 172.20.0.90 binding-slot no)"
  roundtrip_binding="$work/release-binding.json"
  roundtrip_binding_reordered="$work/release-binding-reordered.json"
  "$0" write-binding --out "$roundtrip_binding" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$roundtrip_status" \
    --artifact "$work/browser.rpm" --artifact "$work/a.rpm" >/dev/null
  "$0" write-binding --out "$roundtrip_binding_reordered" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$roundtrip_status" \
    --artifact "$work/a.rpm" --artifact "$work/browser.rpm" >/dev/null
  cmp -s "$roundtrip_binding" "$roundtrip_binding_reordered" \
    || die "self-test: release binding generation was not deterministic"
  jq -e --argjson artifacts "$two_artifacts" '
    .schema_version == 1 and
    .source_commit == "0123456789abcdef0123456789abcdef01234567" and
    .artifacts == $artifacts and
    .farm == {job_id:"farm-job-binding",build_host:"172.20.0.90",build_slot:"binding-slot"}
  ' "$roundtrip_binding" >/dev/null \
    || die "self-test: generated release binding did not preserve exact artifacts and farm identity"
  "$SCRIPT_DIR/ci-gate.sh" bind-release "$roundtrip_binding" "$roundtrip_status" >/dev/null \
    || die "self-test: ci-gate rejected generated release binding"
  roundtrip_evidence="$work/release-binding-roundtrip-evidence.json"
  "$0" write --out "$roundtrip_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/browser.rpm" --artifact "$work/a.rpm" \
    --check github-required=pass --farm-job farm-job-binding \
    --farm-slot 172.20.0.90/binding-slot --sbom rpm=pass \
    --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$roundtrip_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live gate unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null
  "$0" validate "$roundtrip_evidence" >/dev/null \
    || die "self-test: evidence written after bind-release did not validate"

  # Duplicate, missing, and symlinked final artifacts are rejected before an
  # output can replace a caller's existing file.
  duplicate_status="$(self_test_ci_status "$work/ci-binding-hostile-inputs" "$single_artifact" \
    0123456789abcdef0123456789abcdef01234567 farm-job-hostile 172.20.0.90 hostile-slot no)"
  duplicate_binding="$work/duplicate-binding.json"
  set +e
  "$0" write-binding --out "$duplicate_binding" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$duplicate_status" \
    --artifact "$work/a.rpm" --artifact "$work/a.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ ! -e "$duplicate_binding" ] \
    || die "self-test: duplicate release binding artifact was accepted or published"
  missing_binding="$work/missing-binding.json"
  set +e
  "$0" write-binding --out "$missing_binding" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$duplicate_status" --artifact "$work/missing.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ ! -e "$missing_binding" ] \
    || die "self-test: missing release binding artifact was accepted or published"
  symlink_binding_artifact="$work/binding-linked.rpm"
  ln -s -- "$work/a.rpm" "$symlink_binding_artifact"
  set +e
  "$0" write-binding --out "$work/symlink-binding.json" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$duplicate_status" --artifact "$symlink_binding_artifact" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ ! -e "$work/symlink-binding.json" ] \
    || die "self-test: symlinked release binding artifact was accepted or published"

  # A malformed or authenticated-status identity change must leave an existing
  # output byte-for-byte untouched.
  hostile_status="$(self_test_ci_status "$work/ci-binding-hostile-status" "$single_artifact" \
    0123456789abcdef0123456789abcdef01234567 farm-job-status 172.20.0.90 status-slot no)"
  hostile_status_changed="$work/ci-binding-hostile-status/changed-status.json"
  jq '.job_id = "substituted-job"' "$hostile_status" >"$hostile_status_changed"
  hostile_output="$work/hostile-status-output.json"
  printf '%s\n' 'preserve-existing-output' >"$hostile_output"
  set +e
  "$0" write-binding --out "$hostile_output" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$hostile_status_changed" --artifact "$work/a.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ "$(<"$hostile_output")" = preserve-existing-output ] \
    || die "self-test: hostile status identity changed binding output"
  malformed_status="$work/ci-binding-hostile-status/malformed-status.json"
  printf '%s\n' '{}' >"$malformed_status"
  set +e
  "$0" write-binding --out "$work/malformed-status-binding.json" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$malformed_status" --artifact "$work/a.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ ! -e "$work/malformed-status-binding.json" ] \
    || die "self-test: malformed CI status was accepted or published"

  # If a valid generated descriptor is changed after generation, ci-gate binds
  # only those changed bytes; release evidence computed from the real artifacts
  # must consequently fail and remain unpublished.
  hostile_descriptor_status="$(self_test_ci_status "$work/ci-binding-hostile-descriptor" "$single_artifact" \
    0123456789abcdef0123456789abcdef01234567 farm-job-descriptor 172.20.0.90 descriptor-slot no)"
  hostile_descriptor_binding="$work/hostile-descriptor-binding.json"
  "$0" write-binding --out "$hostile_descriptor_binding" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --ci-gate-status "$hostile_descriptor_status" --artifact "$work/a.rpm" >/dev/null
  jq -cS '.artifacts[0].sha256 = ("f" * 64)' "$hostile_descriptor_binding" \
    >"$hostile_descriptor_binding.changed"
  mv -- "$hostile_descriptor_binding.changed" "$hostile_descriptor_binding"
  "$SCRIPT_DIR/ci-gate.sh" bind-release "$hostile_descriptor_binding" \
    "$hostile_descriptor_status" >/dev/null \
    || die "self-test: structurally valid hostile descriptor fixture did not reach evidence validation"
  hostile_descriptor_evidence="$work/hostile-descriptor-evidence.json"
  set +e
  "$0" write --out "$hostile_descriptor_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass \
    --farm-job farm-job-descriptor --farm-slot 172.20.0.90/descriptor-slot \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$hostile_descriptor_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live gate unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] && [ ! -e "$hostile_descriptor_evidence" ] \
    || die "self-test: hostile artifact descriptor produced valid release evidence"

  evidence_a="$work/evidence-a.json"
  evidence_b="$work/evidence-b.json"
  "$0" write --out "$evidence_a" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/browser.rpm" --artifact "$work/a.rpm" \
    --check github-required=pass --check github-policy=pass --check github-cargo-test=not-run \
    --farm-job farm-job-2 --farm-job farm-job-1 \
    --farm-slot 172.20.0.130/3 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" \
    --gate-manifest "$work/gates.json" --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live Dell visual signoff unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null
  "$0" validate "$evidence_a" >/dev/null
  "$0" write --out "$evidence_b" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --artifact "$work/browser.rpm" \
    --check github-cargo-test=not-run --check github-policy=pass --check github-required=pass \
    --farm-job farm-job-1 --farm-job farm-job-2 \
    --farm-slot 172.20.0.90/1 --farm-slot 172.20.0.130/3 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" \
    --gate-manifest "$work/gates.json" --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live Dell visual signoff unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null
  cmp -s "$evidence_a" "$evidence_b" || die "self-test: equivalent inputs were not deterministic"
  symlink_artifact="$work/linked.rpm"
  ln -s -- "$work/a.rpm" "$symlink_artifact"
  set +e
  "$0" write --out "$work/symlink-evidence.json" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$symlink_artifact" --check github-required=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live Dell visual signoff unavailable' \
    --preview-verdict pass --production-verdict not-promoted >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: symlinked release artifact was accepted"
  jq -e '(.artifacts | length == 2) and (.artifacts[0].path | endswith("/a.rpm"))' "$evidence_a" >/dev/null \
    || die "self-test: artifacts were not sorted"
  jq -e --arg digest "$expected_a_sha" '
    (.source_commit == "0123456789abcdef0123456789abcdef01234567") and
    (.artifacts | any(.[]; (.path | endswith("/a.rpm")) and .sha256 == $digest)) and
    (.checks == [{name: "github-cargo-test", status: "not-run"}, {name: "github-policy", status: "pass"}, {name: "github-required", status: "pass"}]) and
    (.farm.job_ids == ["farm-job-1", "farm-job-2"]) and
    (.farm.slot_ids == ["172.20.0.90/1", "172.20.0.130/3"]) and
    (.sbom == [{name: "rpm", status: "pass"}]) and
    (.fedora_matrix == [{target: "fedora-44", status: "pass"}]) and
    (.live_gates == [{name: "dell-install", status: "unavailable"}]) and
    (.unavailable_evidence == ["live Dell visual signoff unavailable"]) and
    (.verdict == {preview: "pass", production: "not-promoted"})
  ' "$evidence_a" >/dev/null || die "self-test: required release evidence fields were not recorded"
  expected_binding="$(binding_payload "$evidence_a" | sha256sum | awk '{print $1}')"
  jq -e --arg binding "$expected_binding" \
    '.schema_version == 5 and .provenance.binding_sha256 == $binding and
     .provenance.resource_publisher_attestation == null and
     (.provenance.sbom_manifest.sha256 | test("^[0-9a-f]{64}$")) and
     (.provenance.gate_manifest.sha256 | test("^[0-9a-f]{64}$"))' \
    "$evidence_a" >/dev/null || die "self-test: provenance binding was not recorded"

  reused_ci_artifact="$work/reused-ci-different-artifact.json"
  printf 'replacement release artifact\n' >"$work/replacement.rpm"
  replacement_descriptor="$(artifact_json "$work/replacement.rpm")"
  jq -S --argjson replacement "$replacement_descriptor" \
    '.artifacts[0] = $replacement | .artifacts |= sort_by(.path)' \
    "$evidence_a" >"$reused_ci_artifact"
  binding="$(binding_payload "$reused_ci_artifact" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$reused_ci_artifact" >"$reused_ci_artifact.bound"
  mv -- "$reused_ci_artifact.bound" "$reused_ci_artifact"
  set +e
  "$0" validate "$reused_ci_artifact" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    || die "self-test: GitHub required-check evidence was reused for a different release artifact"

  short_source="$work/short-source.json"
  jq '.source_commit = "0123456" | .provenance.ci_gate_status = null' \
    "$evidence_a" >"$short_source"
  binding="$(binding_payload "$short_source" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$short_source" >"$short_source.bound"
  mv -- "$short_source.bound" "$short_source"
  set +e
  "$0" validate "$short_source" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: abbreviated source revision was accepted"

  invalid_farm_slot="$work/invalid-farm-slot.json"
  jq '.farm.slot_ids[0] = "172.20.0.130"' "$evidence_a" >"$invalid_farm_slot"
  binding="$(binding_payload "$invalid_farm_slot" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$invalid_farm_slot" >"$invalid_farm_slot.bound"
  mv -- "$invalid_farm_slot.bound" "$invalid_farm_slot"
  set +e
  "$0" validate "$invalid_farm_slot" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: farm slot without an exact host/slot identity was accepted"

  ci_pair_mismatch="$work/ci-pair-mismatch.json"
  # Both identities remain individually present and the binding digest is
  # refreshed; only the positional job/slot association is hostile.
  jq '.farm.slot_ids = [.farm.slot_ids[1], .farm.slot_ids[0]]' \
    "$evidence_a" >"$ci_pair_mismatch"
  binding="$(binding_payload "$ci_pair_mismatch" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$ci_pair_mismatch" >"$ci_pair_mismatch.bound"
  mv -- "$ci_pair_mismatch.bound" "$ci_pair_mismatch"
  set +e
  "$0" validate "$ci_pair_mismatch" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: CI job/slot cross-pair was accepted"

  legacy_preview="$work/legacy-preview.json"
  jq 'del(.provenance.resource_publisher_attestation) |
      .schema_version = 4 |
      .farm.slot_ids |= (sort | unique)' \
    "$evidence_a" >"$legacy_preview"
  binding="$(binding_payload "$legacy_preview" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$legacy_preview" >"$legacy_preview.bound"
  mv -- "$legacy_preview.bound" "$legacy_preview"
  "$0" validate "$legacy_preview" >/dev/null \
    || die "self-test: legacy not-promoted evidence lost compatibility"

  broken="$work/broken.json"
  jq 'del(.farm.slot_ids)' "$evidence_a" >"$broken"
  set +e
  "$0" validate "$broken" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing farm slot IDs were accepted"

  broken_farm="$work/broken-farm.json"
  jq '.farm.slot_ids += ["172.20.0.50/2"]' "$evidence_a" >"$broken_farm"
  set +e
  "$0" validate "$broken_farm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: unmatched farm job/slot evidence was accepted"

  set +e
  "$0" write --out "$work/mismatched-write.json" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 --farm-slot 172.20.0.130/3 \
    --sbom rpm=pass --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live gate unavailable' \
    --preview-verdict pass --production-verdict not-promoted \
    >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: write accepted unmatched farm job/slot evidence"

  production_pass="$work/production-pass.json"
  topology="$work/topology.json"
  python3 - "$SCRIPT_DIR/verify-six-node-topology.py" "$work" "$topology" <<'PY'
import importlib.util
import json
import sys
import time
from pathlib import Path

module_path, root_name, output_name = sys.argv[1:]
spec = importlib.util.spec_from_file_location("six_node_verifier", module_path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
root = Path(root_name)
bundle = module._fixture(source="live", now_ms=time.time_ns() // 1_000_000)
bundle["revision"] = "0123456789abcdef0123456789abcdef01234567"
module._materialize_fixture(bundle, root)
Path(output_name).write_text(json.dumps(bundle), encoding="utf-8")
PY
  ci_status="$ci_status_single"
  missing_resource_attestation="$work/production-missing-resource-attestation.json"
  set +e
  "$0" write --out "$missing_resource_attestation" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=pass --topology-evidence "$topology" --vdi-evidence "$vdi_evidence" \
    --preview-verdict pass --production-verdict pass >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production pass accepted missing resource publisher attestation"

  invalid_resource_attestation="$work/invalid-resource-publisher-attestation.json"
  jq '.signature = "not-an-authenticated-publisher-proof"' "$resource_attestation" \
    >"$invalid_resource_attestation"
  set +e
  "$0" write --out "$work/invalid-resource-attestation-write.json" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=pass --topology-evidence "$topology" --vdi-evidence "$vdi_evidence" \
    --resource-publisher-attestation "$invalid_resource_attestation" \
    --preview-verdict pass --production-verdict pass >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: malformed resource publisher attestation was accepted"

  "$0" write --out "$production_pass" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=pass --topology-evidence "$topology" --vdi-evidence "$vdi_evidence" \
    --resource-publisher-attestation "$resource_attestation" \
    --preview-verdict pass --production-verdict pass >/dev/null
  "$0" validate "$production_pass" >/dev/null \
    || die "self-test: complete all-pass production evidence was rejected"
  jq -e --arg publisher self-test-publisher \
    '.schema_version == 5 and
     .provenance.resource_publisher_attestation.publisher == $publisher and
     .provenance.resource_publisher_attestation.key_id == "resource-publisher-hmac-v1" and
     (.provenance.resource_publisher_attestation.signature | startswith("publisher-attestation:v1:"))' \
    "$production_pass" >/dev/null \
    || die "self-test: resource publisher attestation descriptor was not recorded"

  missing_resource_attestation="$work/production-missing-resource-attestation-evidence.json"
  jq '.provenance.resource_publisher_attestation = null' "$production_pass" \
    >"$missing_resource_attestation"
  binding="$(binding_payload "$missing_resource_attestation" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$missing_resource_attestation" >"$missing_resource_attestation.bound"
  mv -- "$missing_resource_attestation.bound" "$missing_resource_attestation"
  set +e
  "$0" validate "$missing_resource_attestation" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production evidence accepted missing resource publisher attestation"

  descriptor_mismatch="$work/production-resource-descriptor-mismatch.json"
  jq '.provenance.resource_publisher_attestation.publisher = "other-publisher"' \
    "$production_pass" >"$descriptor_mismatch"
  binding="$(binding_payload "$descriptor_mismatch" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$descriptor_mismatch" >"$descriptor_mismatch.bound"
  mv -- "$descriptor_mismatch.bound" "$descriptor_mismatch"
  set +e
  "$0" validate "$descriptor_mismatch" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: detached resource publisher attestation descriptor mismatch was accepted"

  legacy_production="$work/legacy-production.json"
  jq 'del(.provenance.resource_publisher_attestation) | .schema_version = 4' \
    "$production_pass" >"$legacy_production"
  binding="$(binding_payload "$legacy_production" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$legacy_production" >"$legacy_production.bound"
  mv -- "$legacy_production.bound" "$legacy_production"
  set +e
  "$0" validate "$legacy_production" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: legacy production pass bypassed resource publisher attestation"

  topology_revision_mismatch_raw="$work/topology-revision-mismatch-raw.json"
  jq '.revision = "different-source"' "$topology" >"$topology_revision_mismatch_raw"
  set +e
  "$0" write --out "$work/topology-revision-mismatch-write.json" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=pass --topology-evidence "$topology_revision_mismatch_raw" \
    --preview-verdict pass --production-verdict pass >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production topology from a different source revision was accepted"

  topology_revision_mismatch="$work/topology-revision-mismatch.json"
  jq '.provenance.topology_evidence.verification.revision = "different-source"' \
    "$production_pass" >"$topology_revision_mismatch"
  binding="$(binding_payload "$topology_revision_mismatch" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$topology_revision_mismatch" >"$topology_revision_mismatch.bound"
  mv -- "$topology_revision_mismatch.bound" "$topology_revision_mismatch"
  set +e
  "$0" validate "$topology_revision_mismatch" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: topology from a different source revision was accepted"

  farm_topology="$work/farm-topology.json"
  python3 - "$SCRIPT_DIR/verify-six-node-topology.py" "$work" "$farm_topology" <<'PY'
import importlib.util
import json
import sys
import time
from pathlib import Path

module_path, root_name, output_name = sys.argv[1:]
spec = importlib.util.spec_from_file_location("six_node_verifier", module_path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
root = Path(root_name)
bundle = module._fixture(source="farm", now_ms=time.time_ns() // 1_000_000)
bundle["revision"] = "0123456789abcdef0123456789abcdef01234567"
module._materialize_fixture(bundle, root)
Path(output_name).write_text(json.dumps(bundle), encoding="utf-8")
PY
  preview_farm="$work/preview-farm.json"
  "$0" write --out "$preview_farm" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=not-run \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --unavailable 'live unavailable' \
    --topology-evidence "$farm_topology" --preview-verdict pass --production-verdict not-promoted >/dev/null
  "$0" validate "$preview_farm" >/dev/null \
    || die "self-test: farm topology was rejected for preview evidence"

  missing_ci_gate="$work/production-missing-ci-gate.json"
  jq '.provenance.ci_gate_status = null' "$production_pass" >"$missing_ci_gate"
  binding="$(binding_payload "$missing_ci_gate" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$missing_ci_gate" >"$missing_ci_gate.bound"
  mv -- "$missing_ci_gate.bound" "$missing_ci_gate"
  set +e
  "$0" validate "$missing_ci_gate" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production pass accepted missing verified farm status"

  missing_github_check="$work/production-missing-github-check.json"
  jq '.checks |= map(select(.name != "github-required"))' "$production_pass" >"$missing_github_check"
  binding="$(binding_payload "$missing_github_check" | sha256sum | awk '{print $1}')"
  jq -S --arg binding "$binding" '.provenance.binding_sha256 = $binding' \
    "$missing_github_check" >"$missing_github_check.bound"
  mv -- "$missing_github_check.bound" "$missing_github_check"
  set +e
  "$0" validate "$missing_github_check" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production pass accepted missing GitHub-required check"

  failed_gate="$work/failed-gate.json"
  jq '.checks[0].status = "fail" | .verdict.production = "pass"' "$evidence_a" >"$failed_gate"
  set +e
  "$0" validate "$failed_gate" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production pass accepted a failed required gate"

  unavailable_pass="$work/unavailable-pass.json"
  jq '.unavailable_evidence = ["live hardware unavailable"] | .verdict.production = "pass"' \
    "$evidence_a" >"$unavailable_pass"
  set +e
  "$0" validate "$unavailable_pass" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production pass accepted undisclosed unavailable evidence"

  missing_source="$work/missing-source.json"
  jq 'del(.source_commit)' "$evidence_a" >"$missing_source"
  set +e
  "$0" validate "$missing_source" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing source commit was accepted"
  changed_sbom="$work/changed-sbom.json"
  cp -- "$evidence_a" "$changed_sbom"
  printf '{"packages":["tampered"]}\n' >"$work/sbom.json"
  set +e
  "$0" validate "$changed_sbom" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: changed SBOM manifest was accepted"
  changed_gate="$work/changed-gate.json"
  cp -- "$evidence_a" "$changed_gate"
  printf '{"required":["tampered"]}\n' >"$work/gates.json"
  set +e
  "$0" validate "$changed_gate" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: changed gate manifest was accepted"
  set +e
  "$0" write --out "$work/missing-manifest.json" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 --sbom rpm=pass \
    --gate-manifest "$work/gates.json" --fedora-target fedora-44=pass \
    --live-gate dell-install=unavailable --preview-verdict pass \
    --production-verdict not-promoted >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing SBOM manifest was accepted"
  echo "release-evidence: self-test passed (deterministic binding round-trip + fail-closed validation)"
}

need_commands
case "${1:-}" in
  write-binding) write_release_binding "$@" ;;
  write) write_evidence "$@" ;;
  validate) [ "$#" -eq 2 ] || die "usage: $0 validate FILE"; validate_file "$2" ;;
  --self-test|self-test) self_test ;;
  -h|--help|"") usage ;;
  *) die "unknown command: $1 (see --help)" ;;
esac
