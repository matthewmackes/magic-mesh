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
#     --vdi-evidence vdi-evidence.json \
#     --fedora-target fedora-44=pass \
#     --live-gate dell-gui=unavailable \
#     --unavailable "live Dell visual signoff not available" \
#     --preview-verdict pass --production-verdict not-promoted
#   install-helpers/release-evidence.sh validate evidence.json
#   install-helpers/release-evidence.sh --self-test
#
# Requires: bash, jq, realpath, stat, sha256sum, mktemp.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
SCHEMA_VERSION=4

die() {
  echo "release-evidence: $*" >&2
  exit 2
}

usage() {
  sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'USAGE'

Commands:
  write --out FILE --source-commit SHA --artifact PATH...
        --check GITHUB_CHECK=STATUS... --farm-job ID... --farm-slot ID...
        --sbom NAME=STATUS... --sbom-manifest FILE
        --gate-manifest FILE --ci-gate-status FILE \
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

binding_payload() {
  jq -cS '{source_commit,
    artifacts,
    sbom_manifest: .provenance.sbom_manifest,
    gate_manifest: .provenance.gate_manifest,
    ci_gate_status: .provenance.ci_gate_status,
    topology_evidence: .provenance.topology_evidence,
    vdi_evidence: .provenance.vdi_evidence,
    gates: {checks, farm, sbom, fedora_matrix, live_gates,
      unavailable_evidence, verdict}}' "$1"
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

validate_file() {
  local file="$1"
  local -a topology_verify_args=()
  [ -f "$file" ] && [ ! -L "$file" ] || die "evidence file is not a regular, non-symlink file: $file"
  jq -e --arg source_commit "$(jq -r '.source_commit' "$file")" '
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
    def production_verdict_ok:
      # Production promotion is valid only when every declared release gate is
      # an observed pass and no live or hardware evidence is unavailable.
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
        (.provenance.vdi_evidence | type == "object")
      );

    (type == "object") and
    (keys == ["artifacts", "checks", "farm", "fedora_matrix", "live_gates", "provenance", "sbom", "schema_version", "source_commit", "unavailable_evidence", "verdict"]) and
    (.schema_version == 4) and
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
      (.slot_ids | nonempty_sorted_unique_strings) and
      ((.job_ids | length) == (.slot_ids | length))) and
    (.sbom | sorted_unique_named("name")) and
    (.fedora_matrix | sorted_unique_named("target")) and
    (.live_gates | sorted_unique_named("name")) and
    (.unavailable_evidence | type == "array" and
      all(.[]; type == "string" and length > 0) and
      (. == (sort | unique))) and
    (.provenance | type == "object" and
      (keys == ["binding_sha256", "ci_gate_status", "gate_manifest", "sbom_manifest", "schema_version", "topology_evidence", "vdi_evidence"]) and
      (.schema_version == 1) and
      (.binding_sha256 | type == "string" and test("^[0-9a-f]{64}$"))) and
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
    (.verdict | type == "object" and (keys == ["preview", "production"]) and
      (.preview | verdict_ok) and (.production | verdict_ok))
    and production_verdict_ok
  ' "$file" >/dev/null || die "invalid or incomplete evidence schema: $file"
  verify_descriptor "SBOM" "$(jq -c '.provenance.sbom_manifest' "$file")"
  verify_descriptor "gate" "$(jq -c '.provenance.gate_manifest' "$file")"
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
    jq -e --arg job "$(jq -r '.job_id' "$ci_gate")" \
      --arg slot "$(jq -r '.build_host' "$ci_gate")/$(jq -r '.build_slot' "$ci_gate")" \
      '(.farm.job_ids | index($job)) and (.farm.slot_ids | index($slot))' \
      "$file" >/dev/null \
      || die "ci-gate status job/slot is not listed in release farm evidence"
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
  local out="" source_commit="" preview="" production="" sbom_manifest="" gate_manifest="" ci_gate_status="" topology_evidence="" vdi_evidence="" arg parent tmp binding topology_verification topology_descriptor
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
  [[ "$source_commit" =~ ^[0-9A-Fa-f]{7,64}$ ]] || die "source commit must be a hexadecimal Git SHA (7-64 characters)"
  [ "${#artifacts[@]}" -gt 0 ] || die "at least one --artifact is required"
  [ "${#checks[@]}" -gt 0 ] || die "at least one --check is required"
  [ "${#farm_jobs[@]}" -gt 0 ] || die "at least one --farm-job is required"
  [ "${#farm_slots[@]}" -gt 0 ] || die "at least one --farm-slot is required"
  [ "${#farm_jobs[@]}" -eq "${#farm_slots[@]}" ] \
    || die "each --farm-job requires a matching --farm-slot"
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
    --slurpfile vdi_evidence "$tmp.vdi" \
    --argjson topology_evidence "$topology_descriptor" \
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
      verdict: {preview: $preview, production: $production},
      provenance: {schema_version: 1, binding_sha256: "",
        sbom_manifest: $sbom_manifest[0], gate_manifest: $gate_manifest[0],
        ci_gate_status: $ci_gate_status[0], topology_evidence: $topology_evidence,
        vdi_evidence: $vdi_evidence[0]}}' \
    | jq -S . >"$tmp"
  rm -f -- "$tmp.artifacts"
  rm -f -- "$tmp.sbom" "$tmp.gate" "$tmp.ci-gate"
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

self_test() {
  local work evidence_a evidence_b broken broken_farm production_pass production_pass_unbound topology farm_topology preview_farm topology_revision_mismatch topology_revision_mismatch_raw missing_ci_gate missing_github_check failed_gate unavailable_pass missing_source changed_sbom changed_gate expected_a_sha expected_binding symlink_artifact rc
  local ci_status ci_log ci_digest vdi_evidence vdi_digest
  work="$(mktemp -d)"
  trap 'rm -rf -- "$work"' RETURN
  printf 'alpha release artifact\n' >"$work/a.rpm"
  printf 'browser release artifact with a second line\n' >"$work/browser.rpm"
  printf '{"packages":["magic-mesh"]}\n' >"$work/sbom.json"
  printf '{"required":["github-policy","fedora-44"]}\n' >"$work/gates.json"
  ci_status="$work/ci-gate-status.json"
  ci_log="$work/ci-gate.log"
  vdi_evidence="$work/vdi-evidence.json"
  cat >"$ci_log" <<'EOF'
MCNF CI gate — self-test
  sha=0123456  revision=0123456789abcdef0123456789abcdef01234567  job_id=farm-job-1  host=172.20.0.90 (slot=1)
=== CI GATE SUMMARY self-test → green ===
  policy   pass
  fmt      pass
  clippy   pass
  test     pass  (1 passed, 0 failed)
  coverage pass
EOF
  ci_digest="$(sha256sum -- "$ci_log" | awk '{print $1}')"
  jq -n --arg sha 0123456789abcdef0123456789abcdef01234567 \
    --arg log "ci-gate.log" --arg digest "$ci_digest" \
    '{overall:"green",alert:false,failed_stage:"",stages:{policy:"pass",fmt:"pass",clippy:"pass",test:"pass",coverage:"pass"},tests_passed:1,tests_failed:0,sha:$sha,short_sha:($sha[0:7]),job_id:"farm-job-1",build_host:"172.20.0.90",build_slot:"1",evidence:{revision:$sha,gate_log:{path:$log,sha256:$digest}},started:"self-test",finished:"self-test",source:"ci-gate"}' \
    >"$ci_status"
  cat >"$vdi_evidence" <<'EOF'
{"frame":{"fnv1a64":"0x0123456789abcdef","height":768,"width":1024},"input_observation":"echoed","probe":{"log_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","returncode":0,"source":"mde-shell-egui ignored live worker test"},"protocol":"vnc","recorded_at":"2026-08-01T00:00:00Z","schema_version":1,"source_commit":"0123456789abcdef0123456789abcdef01234567","status":"observed","target":{"host":"127.0.0.1","port":15903}}
EOF
  expected_a_sha="$(sha256sum -- "$work/a.rpm" | awk '{print $1}')"
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
    (.farm.slot_ids == ["172.20.0.130/3", "172.20.0.90/1"]) and
    (.sbom == [{name: "rpm", status: "pass"}]) and
    (.fedora_matrix == [{target: "fedora-44", status: "pass"}]) and
    (.live_gates == [{name: "dell-install", status: "unavailable"}]) and
    (.unavailable_evidence == ["live Dell visual signoff unavailable"]) and
    (.verdict == {preview: "pass", production: "not-promoted"})
  ' "$evidence_a" >/dev/null || die "self-test: required release evidence fields were not recorded"
  expected_binding="$(binding_payload "$evidence_a" | sha256sum | awk '{print $1}')"
  jq -e --arg binding "$expected_binding" \
    '.schema_version == 4 and .provenance.binding_sha256 == $binding and
     (.provenance.sbom_manifest.sha256 | test("^[0-9a-f]{64}$")) and
     (.provenance.gate_manifest.sha256 | test("^[0-9a-f]{64}$"))' \
    "$evidence_a" >/dev/null || die "self-test: provenance binding was not recorded"

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
  production_pass_unbound="$work/production-pass-unbound.json"
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
  "$0" write --out "$production_pass" --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check github-required=pass --check github-policy=pass --check github-cargo-test=pass \
    --farm-job farm-job-1 --farm-slot 172.20.0.90/1 \
    --sbom rpm=pass --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --ci-gate-status "$ci_status" --fedora-target fedora-44=pass \
    --live-gate dell-install=pass --topology-evidence "$topology" --vdi-evidence "$vdi_evidence" \
    --preview-verdict pass --production-verdict pass >/dev/null
  "$0" validate "$production_pass" >/dev/null \
    || die "self-test: complete all-pass production evidence was rejected"

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
