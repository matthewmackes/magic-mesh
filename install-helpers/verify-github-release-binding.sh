#!/usr/bin/env bash
# Verify the workflow-carried final-artifact binding for WL-CRIT-006.
#
# This is deliberately a verifier, not a publisher. It never discovers a
# revision or farm identity and never contacts GitHub. The caller supplies the
# exact expected GitHub/farm association; this helper checks the downloaded
# files against the canonical write-binding payload and the authenticated
# ci-gate log/status pair.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAX_BINDING_BYTES=$((1024 * 1024))
MAX_ARTIFACTS=1024

die() {
  echo "verify-github-release-binding: $*" >&2
  exit 1
}

sha256_file() {
  sha256sum -- "$1" | awk '{print $1}'
}

require_regular() {
  local label="$1" path="$2"
  [ -f "$path" ] && [ ! -L "$path" ] \
    || die "$label is not a regular, non-symlink file: $path"
}

verify_bundle() {
  local bundle_root="$1" artifact_root="$2" revision="$3" job_id="$4"
  local build_host="$5" build_slot="$6"
  local status log binding copies binding_size canonical binding_sha binding_line
  local status_before log_before binding_before descriptor_path name expected_size expected_sha
  local copy actual_size actual_sha descriptor_count copy_count

  [ -d "$bundle_root" ] && [ ! -L "$bundle_root" ] \
    || die "bundle root is not a non-symlink directory: $bundle_root"
  bundle_root="$(realpath -e -- "$bundle_root")"
  case "$artifact_root" in
    /tmp/mcnf-github-release-*/artifacts) ;;
    *) die "artifact root is outside the workflow-owned /tmp namespace" ;;
  esac
  case "$artifact_root" in
    *[[:space:][:cntrl:]]*) die "artifact root contains unsafe characters" ;;
  esac
  status="$bundle_root/ci-gate-status.json"
  log="$bundle_root/ci-gate.log"
  binding="$bundle_root/release-binding.json"
  copies="$bundle_root/release-artifacts"
  require_regular "ci-gate status" "$status"
  require_regular "ci-gate log" "$log"
  require_regular "release binding" "$binding"
  [ -d "$copies" ] && [ ! -L "$copies" ] \
    || die "release artifact directory is not a non-symlink directory: $copies"
  binding_size="$(stat -c '%s' -- "$binding")"
  [ "$binding_size" -gt 0 ] && [ "$binding_size" -le "$MAX_BINDING_BYTES" ] \
    || die "release binding must be 1..$MAX_BINDING_BYTES bytes"
  command -v jq >/dev/null 2>&1 || die "jq is required"

  jq -e \
    --arg revision "$revision" \
    --arg job_id "$job_id" \
    --arg build_host "$build_host" \
    --arg build_slot "$build_slot" \
    --arg artifact_root "$artifact_root" \
    --argjson max_artifacts "$MAX_ARTIFACTS" '
      def identity:
        (type == "string") and (length > 0) and (length <= 255) and
        test("^[^[:space:][:cntrl:]]+$");
      def artifact_name:
        type == "string" and length > 0 and length <= 255 and
        test("^[A-Za-z0-9][A-Za-z0-9._+-]*\\.rpm$");
      (type == "object") and
      (keys == ["artifacts", "farm", "schema_version", "source_commit"]) and
      (.schema_version == 1) and
      (.source_commit == $revision) and
      (.farm == {job_id:$job_id,build_host:$build_host,build_slot:$build_slot}) and
      (.farm.job_id | identity) and (.farm.build_host | identity) and
      (.farm.build_slot | identity) and
      (.artifacts | type == "array" and length > 0 and length <= $max_artifacts and
        all(.[];
          (type == "object") and
          (keys == ["path", "sha256", "size_bytes"]) and
          (.path | startswith($artifact_root + "/")) and
          (.path | ltrimstr($artifact_root + "/") | artifact_name) and
          ((.path | ltrimstr($artifact_root + "/")) | contains("/") | not) and
          (.size_bytes | type == "number" and floor == . and . > 0) and
          (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))) and
        (. == sort_by(.path)) and
        ((map(.path) | unique | length) == length))
    ' "$binding" >/dev/null \
    || die "binding is malformed or does not match the exact revision/job/host/slot/artifact root"

  status_before="$(sha256_file "$status")"
  log_before="$(sha256_file "$log")"
  binding_before="$(sha256_file "$binding")"
  "$HERE/ci-gate.sh" verify "$status" \
    --expected-revision "$revision" \
    --expected-job-id "$job_id" \
    --expected-build-host "$build_host" \
    --expected-build-slot "$build_slot" >/dev/null \
    || die "ci-gate status/log did not verify against the expected GitHub farm identity"

  canonical="$(jq -cS . "$binding")" || die "could not canonicalize binding"
  binding_sha="$(printf '%s\n' "$canonical" | sha256sum | awk '{print $1}')"
  binding_line="  release-evidence-binding sha256=$binding_sha"
  [ "$(grep -Ec '^  release-evidence-binding([[:space:]]|$)' "$log" || true)" -eq 1 ] \
    && [ "$(grep -Fxc -- "$binding_line" "$log" || true)" -eq 1 ] \
    || die "authenticated gate log does not contain exactly this final-artifact binding"

  if find "$copies" -mindepth 1 -maxdepth 1 ! -type f -print -quit | grep -q .; then
    die "downloaded artifact set contains a symlink, directory, or non-regular entry"
  fi
  descriptor_count="$(jq '.artifacts | length' "$binding")"
  copy_count="$(find "$copies" -mindepth 1 -maxdepth 1 -type f -printf '.' | wc -c)"
  [ "$copy_count" -eq "$descriptor_count" ] \
    || die "downloaded artifact count does not match the bound descriptor count"

  while IFS=$'\t' read -r descriptor_path expected_size expected_sha; do
    name="${descriptor_path##*/}"
    copy="$copies/$name"
    require_regular "bound release artifact" "$descriptor_path"
    require_regular "downloaded release artifact" "$copy"
    actual_size="$(stat -c '%s' -- "$descriptor_path")"
    actual_sha="$(sha256_file "$descriptor_path")"
    [ "$actual_size" = "$expected_size" ] \
      || die "bound release artifact size does not match descriptor: $name"
    [ "$actual_sha" = "$expected_sha" ] \
      || die "bound release artifact digest does not match descriptor: $name"
    actual_size="$(stat -c '%s' -- "$copy")"
    actual_sha="$(sha256_file "$copy")"
    [ "$actual_size" = "$expected_size" ] \
      || die "downloaded release artifact size does not match binding: $name"
    [ "$actual_sha" = "$expected_sha" ] \
      || die "downloaded release artifact digest does not match binding: $name"
  done < <(jq -r '.artifacts[] | [.path, (.size_bytes | tostring), .sha256] | @tsv' "$binding")

  [ "$(sha256_file "$status")" = "$status_before" ] \
    && [ "$(sha256_file "$log")" = "$log_before" ] \
    && [ "$(sha256_file "$binding")" = "$binding_before" ] \
    || die "release evidence changed while it was being verified"
  echo "verify-github-release-binding: verified $descriptor_count final artifact(s) for $revision on $build_host/$build_slot"
}

workflow_job() {
  local workflow="$1" job="$2"
  awk -v job="$job" '
    $0 == "  " job ":" {inside=1}
    inside && $0 ~ /^  [A-Za-z0-9_-]+:$/ && $0 != "  " job ":" {exit}
    inside {print}
  ' "$workflow"
}

check_workflow() {
  local workflow="$1" farm required
  require_regular "workflow" "$workflow"
  farm="$(workflow_job "$workflow" farm-gate)"
  required="$(workflow_job "$workflow" github-required)"
  [ -n "$farm" ] && [ -n "$required" ] || die "required workflow jobs are missing"
  grep -Fqx 'permissions:' "$workflow" \
    && grep -Fqx '  contents: read' "$workflow" \
    || die "workflow does not declare least top-level contents:read permissions"
  grep -Fq 'release-evidence.sh write-binding' <<<"$farm" \
    && grep -Fq 'ci-gate.sh bind-release' <<<"$farm" \
    && grep -Fq 'verify-github-release-binding.sh verify' <<<"$farm" \
    || die "farm-gate does not build, bind, and verify final artifacts"
  grep -Fq 'release-binding.json' <<<"$farm" \
    && grep -Fq 'release-artifacts' <<<"$farm" \
    || die "farm-gate does not upload the binding and final artifacts"
  grep -Fq 'actions/download-artifact@v4' <<<"$required" \
    && grep -Fq 'verify-github-release-binding.sh verify' <<<"$required" \
    || die "github-required does not download and verify final artifacts"
  if grep -Fq 'continue-on-error:' <<<"$farm$required"; then
    die "authoritative release jobs may not continue on error"
  fi
  echo "verify-github-release-binding: workflow wiring is fail-closed"
}

expect_reject() {
  if bash "$0" verify "$@" >/dev/null 2>&1; then
    die "self-test hostile bundle was accepted"
  fi
}

self_test() {
  local work bundle origin revision job host slot binding_sha log_digest artifact digest
  work="$(mktemp -d "${TMPDIR:-/tmp}/mcnf-github-release-self-test.XXXXXX")"
  trap 'rm -rf -- "$work"' RETURN
  bundle="$work/bundle"
  origin="$work/artifacts"
  revision=0123456789abcdef0123456789abcdef01234567
  job=github-ci:42:1
  host=172.20.0.130
  slot=github-42-1
  mkdir -p -- "$bundle/release-artifacts" "$origin"
  printf 'base rpm bytes\n' >"$bundle/release-artifacts/magic-mesh-1.x86_64.rpm"
  printf 'lighthouse rpm bytes\n' >"$bundle/release-artifacts/magic-mesh-lighthouse-1.x86_64.rpm"
  cp -- "$bundle/release-artifacts/"*.rpm "$origin/"
  jq -nS \
    --arg revision "$revision" --arg job "$job" --arg host "$host" --arg slot "$slot" \
    --arg root "$origin" \
    --arg base_sha "$(sha256_file "$bundle/release-artifacts/magic-mesh-1.x86_64.rpm")" \
    --argjson base_size "$(stat -c '%s' "$bundle/release-artifacts/magic-mesh-1.x86_64.rpm")" \
    --arg lighthouse_sha "$(sha256_file "$bundle/release-artifacts/magic-mesh-lighthouse-1.x86_64.rpm")" \
    --argjson lighthouse_size "$(stat -c '%s' "$bundle/release-artifacts/magic-mesh-lighthouse-1.x86_64.rpm")" '
      {schema_version:1,source_commit:$revision,
       artifacts:[
         {path:($root + "/magic-mesh-1.x86_64.rpm"),size_bytes:$base_size,sha256:$base_sha},
         {path:($root + "/magic-mesh-lighthouse-1.x86_64.rpm"),size_bytes:$lighthouse_size,sha256:$lighthouse_sha}],
       farm:{job_id:$job,build_host:$host,build_slot:$slot}}
    ' >"$bundle/release-binding.json"
  binding_sha="$(jq -cS . "$bundle/release-binding.json" | sha256sum | awk '{print $1}')"
  {
    printf '%s\n' 'MCNF CI gate — workflow verifier self-test'
    printf '  sha=%s  revision=%s  job_id=%s  host=%s (slot=%s)\n' \
      "${revision:0:7}" "$revision" "$job" "$host" "$slot"
    printf '%s\n' '=== CI GATE SUMMARY self-test → green ==='
    printf '%s\n' '  policy   pass' '  fmt      pass' '  clippy   pass'
    printf '%s\n' '  test     pass  (2 passed, 0 failed)' '  coverage pass'
    printf '  release-evidence-binding sha256=%s\n' "$binding_sha"
  } >"$bundle/ci-gate.log"
  log_digest="$(sha256_file "$bundle/ci-gate.log")"
  jq -nS --arg revision "$revision" --arg job "$job" --arg host "$host" \
    --arg slot "$slot" --arg digest "$log_digest" '
      {overall:"green",alert:false,failed_stage:"",
       stages:{policy:"pass",fmt:"pass",clippy:"pass",test:"pass",coverage:"pass"},
       tests_passed:2,tests_failed:0,sha:$revision,short_sha:($revision[0:7]),
       job_id:$job,build_host:$host,build_slot:$slot,
       evidence:{revision:$revision,gate_log:{path:"ci-gate.log",sha256:$digest}},
       started:"self-test",finished:"self-test",source:"ci-gate"}
    ' >"$bundle/ci-gate-status.json"

  verify_bundle "$bundle" "$origin" "$revision" "$job" "$host" "$slot" >/dev/null
  artifact="$bundle/release-artifacts/magic-mesh-1.x86_64.rpm"
  printf 'tamper\n' >>"$artifact"
  expect_reject "$bundle" "$origin" "$revision" "$job" "$host" "$slot"
  printf 'base rpm bytes\n' >"$artifact"
  expect_reject "$bundle" "$origin" "$revision" wrong-job "$host" "$slot"
  printf 'extra\n' >"$bundle/release-artifacts/extra.rpm"
  expect_reject "$bundle" "$origin" "$revision" "$job" "$host" "$slot"
  rm -f -- "$bundle/release-artifacts/extra.rpm"
  mv -- "$artifact" "$artifact.real"
  ln -s -- "$artifact.real" "$artifact"
  expect_reject "$bundle" "$origin" "$revision" "$job" "$host" "$slot"
  rm -f -- "$artifact"
  mv -- "$artifact.real" "$artifact"
  cp -- "$bundle/release-binding.json" "$work/binding.good"
  jq --arg path "$work/outside.rpm" '.artifacts[0].path = $path' \
    "$work/binding.good" >"$bundle/release-binding.json"
  expect_reject "$bundle" "$origin" "$revision" "$job" "$host" "$slot"
  mv -- "$work/binding.good" "$bundle/release-binding.json"
  cp -- "$bundle/ci-gate.log" "$work/log.good"
  grep -v '^  release-evidence-binding ' "$work/log.good" >"$bundle/ci-gate.log"
  digest="$(sha256_file "$bundle/ci-gate.log")"
  jq --arg digest "$digest" '.evidence.gate_log.sha256 = $digest' \
    "$bundle/ci-gate-status.json" >"$work/status.updated"
  mv -- "$work/status.updated" "$bundle/ci-gate-status.json"
  expect_reject "$bundle" "$origin" "$revision" "$job" "$host" "$slot"
  echo "verify-github-release-binding: self-test passed — hostile artifact, identity, set, symlink, path, and unbound-log cases rejected"
}

usage() {
  cat <<'EOF'
Usage:
  verify-github-release-binding.sh verify BUNDLE_ROOT ARTIFACT_ROOT REVISION JOB_ID BUILD_HOST BUILD_SLOT
  verify-github-release-binding.sh check-workflow WORKFLOW
  verify-github-release-binding.sh --self-test
EOF
}

case "${1:-}" in
  verify)
    [ "$#" -eq 7 ] || { usage >&2; exit 2; }
    verify_bundle "${@:2}"
    ;;
  check-workflow)
    [ "$#" -eq 2 ] || { usage >&2; exit 2; }
    check_workflow "$2"
    ;;
  --self-test)
    [ "$#" -eq 1 ] || { usage >&2; exit 2; }
    self_test
    ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
