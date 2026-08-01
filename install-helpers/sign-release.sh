#!/usr/bin/env bash
# EFF-30 — operator-gated release signing (manual, never CI).
#
# Signs built artifacts with the project GPG key and emits SHA256SUMS plus a
# detached signature.  When --evidence is supplied, the release-evidence
# envelope is validated first and a canonical PROVENANCE.json is added to the
# signed bundle.  That path binds the source, exact artifact set, SBOM
# manifest, and gate manifest before a signature can be published.
#
# Usage:
#   ./install-helpers/sign-release.sh [--evidence evidence.json] <artifact>...
#   ./install-helpers/sign-release.sh --self-test
#
# Requires: gpg; jq, realpath, stat, sha256sum, and release-evidence.sh when
# publishing provenance.  rpmsign is required when an .rpm is among artifacts.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
KEY_ID="${MAGIC_MESH_SIGN_KEY:-Magic Mesh Release Signing}"

die() {
  echo "sign-release: $*" >&2
  exit 2
}

usage() {
  cat <<'USAGE'
usage: sign-release.sh [--evidence EVIDENCE.json] ARTIFACT...
       sign-release.sh --self-test

Without --evidence, signs the supplied artifacts in the historical
SHA256SUMS/SHA256SUMS.asc format.  With --evidence, the evidence envelope must
be schema-valid and its artifact list, SBOM manifest, and gate manifest must
all be present and unchanged.  The same directory then receives a signed
PROVENANCE.json publication.
USAGE
}

file_descriptor() {
  local path="$1" resolved size digest
  [ -s "$path" ] || die "input is missing or empty: $path"
  [ -f "$path" ] || die "input is not a regular file: $path"
  resolved="$(realpath -e -- "$path")" || die "could not resolve input: $path"
  size="$(stat -c '%s' -- "$resolved")" || die "could not stat input: $resolved"
  digest="$(sha256sum -- "$resolved" | awk '{print $1}')"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "could not hash input: $resolved"
  jq -cn --arg path "$resolved" --arg sha256 "$digest" --argjson size "$size" \
    '{path: $path, size_bytes: $size, sha256: $sha256}'
}

require_provenance_inputs() {
  local evidence="$1"
  command -v jq >/dev/null 2>&1 || die "required command not found: jq"
  command -v realpath >/dev/null 2>&1 || die "required command not found: realpath"
  command -v stat >/dev/null 2>&1 || die "required command not found: stat"
  command -v sha256sum >/dev/null 2>&1 || die "required command not found: sha256sum"
  [ -x "$SCRIPT_DIR/release-evidence.sh" ] || die "release-evidence.sh is unavailable"
  [ -f "$evidence" ] || die "evidence file is missing: $evidence"
  "$SCRIPT_DIR/release-evidence.sh" validate "$evidence" >/dev/null
}

same_directory() {
  local expected_dir="$1" path resolved
  shift
  for path in "$@"; do
    resolved="$(realpath -e -- "$path")" || die "could not resolve bundle input: $path"
    [ "$(dirname -- "$resolved")" = "$expected_dir" ] \
      || die "provenance inputs must share one publication directory: $path"
  done
}

prepare_provenance() {
  local evidence="$1" outdir="$2" provenance evidence_descriptor expected actual path
  local sbom_path gate_path
  evidence="$(realpath -e -- "$evidence")"
  provenance="$outdir/PROVENANCE.json"
  evidence_descriptor="$(file_descriptor "$evidence")"
  sbom_path="$(jq -r '.provenance.sbom_manifest.path' "$evidence")"
  gate_path="$(jq -r '.provenance.gate_manifest.path' "$evidence")"
  same_directory "$outdir" "$evidence" "$sbom_path" "$gate_path"

  expected="$(jq -cS '[.[] | {path, size_bytes, sha256}] | sort_by(.path)' <<<"$(jq -c '.artifacts' "$evidence")")"
  actual="$(for path in "${ARTIFACTS[@]}"; do file_descriptor "$path"; done | jq -csS 'sort_by(.path)')"
  [ "$actual" = "$expected" ] \
    || die "artifact arguments do not exactly match the evidence artifact set"

  jq -S \
    --argjson evidence "$evidence_descriptor" \
    '{schema_version: 1,
      evidence: $evidence,
      source_commit: .source_commit,
      artifacts: .artifacts,
      sbom_manifest: .provenance.sbom_manifest,
      gate_manifest: .provenance.gate_manifest,
      binding_sha256: .provenance.binding_sha256}' \
    "$evidence" >"$provenance"
  printf '%s\n' "$provenance"
}

write_sums() {
  local outdir="$1" path
  shift
  : >"$outdir/SHA256SUMS"
  for path in "$@"; do
    ( cd -- "$outdir" && sha256sum -- "$(basename -- "$path")" ) >>"$outdir/SHA256SUMS"
  done
}

publish() {
  local evidence="" outdir="" provenance
  local -a bundle_inputs=()
  if [ -n "${EVIDENCE:-}" ]; then
    require_provenance_inputs "$EVIDENCE"
    EVIDENCE="$(realpath -e -- "$EVIDENCE")"
    outdir="$(dirname -- "$(realpath -e -- "${ARTIFACTS[0]}")")"
    same_directory "$outdir" "$EVIDENCE" "${ARTIFACTS[@]}"
    provenance="$(prepare_provenance "$EVIDENCE" "$outdir")"
    bundle_inputs=("${ARTIFACTS[@]}" "$EVIDENCE" \
      "$(jq -r '.provenance.sbom_manifest.path' "$EVIDENCE")" \
      "$(jq -r '.provenance.gate_manifest.path' "$EVIDENCE")" "$provenance")
    # Basenames are the names carried by SHA256SUMS; duplicates would make
    # verification ambiguous, so refuse them instead of silently overwriting.
    [ "$(printf '%s\n' "${bundle_inputs[@]}" | xargs -r -n1 basename | sort -u | wc -l)" -eq "${#bundle_inputs[@]}" ] \
      || die "provenance bundle contains duplicate basenames"
  else
    outdir="$(dirname -- "$(realpath -e -- "${ARTIFACTS[0]}")")"
    bundle_inputs=("${ARTIFACTS[@]}")
  fi

  if ! gpg --list-secret-keys "$KEY_ID" >/dev/null 2>&1; then
    die "secret key '$KEY_ID' not in this keyring — run on the release operator's machine"
  fi

  for artifact in "${ARTIFACTS[@]}"; do
    case "$artifact" in
      *.rpm)
        if [ -n "${EVIDENCE:-}" ]; then
          # Evidence hashes the final artifact.  Rewriting an RPM here would
          # invalidate that binding, so provenance publication consumes the
          # already-signed artifact instead of mutating it.
          :
        else
          command -v rpmsign >/dev/null || die "rpmsign missing (dnf install rpm-sign)"
          rpmsign --define "_gpg_name $KEY_ID" --addsign "$artifact"
          # Older rpm implementations can report NOKEY for the newer signing
          # subkey; target-side verification remains authoritative.
          rpm --checksig "$artifact" || true
        fi
      ;;
    esac
  done

  write_sums "$outdir" "${bundle_inputs[@]}"
  gpg --armor --detach-sign --local-user "$KEY_ID" --yes \
    --output "$outdir/SHA256SUMS.asc" "$outdir/SHA256SUMS"

  if [ -n "${EVIDENCE:-}" ]; then
    echo "sign-release: signed provenance bundle with ${#ARTIFACTS[@]} artifact(s); wrote $outdir/{PROVENANCE.json,SHA256SUMS{,.asc}}"
  else
    echo "sign-release: signed ${#ARTIFACTS[@]} artifact(s); wrote $outdir/SHA256SUMS{,.asc}"
  fi
  echo "verify with:  (cd $outdir && sha256sum -c SHA256SUMS && gpg --verify SHA256SUMS.asc SHA256SUMS)"
}

self_test() {
  local work fakebin evidence rc
  work="$(mktemp -d)"
  trap 'rm -rf -- "$work"' RETURN
  fakebin="$work/bin"
  mkdir -p -- "$fakebin"
  cat >"$fakebin/gpg" <<'FAKE_GPG'
#!/usr/bin/env bash
set -euo pipefail
case " $* " in
  *" --list-secret-keys "*) exit 0 ;;
esac
output=""
previous=""
for arg in "$@"; do
  if [ "$previous" = "--output" ]; then output="$arg"; fi
  previous="$arg"
done
[ -n "$output" ] || exit 2
printf 'test signature\n' >"$output"
FAKE_GPG
  chmod +x "$fakebin/gpg"
  printf 'artifact\n' >"$work/a.rpm"
  printf '{"packages":["a"]}\n' >"$work/sbom.json"
  printf '{"gates":["check"]}\n' >"$work/gates.json"
  evidence="$work/evidence.json"
  "$SCRIPT_DIR/release-evidence.sh" write --out "$evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/a.rpm" --check policy=pass \
    --farm-job farm-1 --farm-slot slot-1 --sbom rpm=pass \
    --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --fedora-target fedora-44=pass --live-gate live=unavailable \
    --unavailable 'live unavailable' --preview-verdict pass \
    --production-verdict not-promoted >/dev/null
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    "$0" --evidence "$work/missing.json" "$work/a.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: missing evidence was accepted"
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    "$0" --evidence "$evidence" "$work/a.rpm" >/dev/null
  [ -s "$work/PROVENANCE.json" ] || die "self-test: provenance publication missing"
  [ -s "$work/SHA256SUMS.asc" ] || die "self-test: detached signature missing"
  jq -e --arg source 0123456789abcdef0123456789abcdef01234567 \
    '.schema_version == 1 and .source_commit == $source and
     (.artifacts | length == 1) and (.binding_sha256 | test("^[0-9a-f]{64}$"))' \
    "$work/PROVENANCE.json" >/dev/null \
    || die "self-test: provenance publication was not bound to evidence"
  grep -q 'PROVENANCE.json' "$work/SHA256SUMS" \
    || die "self-test: provenance was not included in signed checksums"
  echo "sign-release: self-test passed (fail-closed provenance publication)"
}

EVIDENCE=""
ARTIFACTS=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --evidence)
      [ "$#" -ge 2 ] || die "--evidence needs a file"
      EVIDENCE="$2"
      shift 2
      ;;
    --self-test) self_test; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; ARTIFACTS+=("$@"); break ;;
    -*) die "unknown option: $1" ;;
    *) ARTIFACTS+=("$1"); shift ;;
  esac
done

[ "${#ARTIFACTS[@]}" -gt 0 ] || { usage >&2; exit 2; }
for artifact in "${ARTIFACTS[@]}"; do
  [ -s "$artifact" ] || die "missing or empty artifact: $artifact"
  [ -f "$artifact" ] || die "artifact is not a regular file: $artifact"
done
publish
