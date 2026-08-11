#!/usr/bin/env bash
# EFF-30 — operator-gated release signing (manual, never CI).
#
# The operator-only --prepare-rpms stage embeds and verifies RPM signatures but
# never emits a release bundle. The separate --evidence publication stage never
# mutates artifacts: it validates the release-evidence envelope, represents it
# with a canonical PROVENANCE.json, and emits SHA256SUMS plus its detached
# signature. That final path binds the source, exact artifact set, SBOM
# manifest, gate manifest, and verified publisher-attestation descriptor before
# a signature can be published. The publisher HMAC itself covers the
# attestation/catalog digest, not the Git revision; checkout equality and the
# final GPG-signed bundle provide that separate revision binding.
#
# Usage:
#   ./install-helpers/sign-release.sh --prepare-rpms <rpm>...
#   ./install-helpers/sign-release.sh --evidence evidence.json
#     [--resource-publisher-credential /absolute/path] <artifact>...
#   ./install-helpers/sign-release.sh --self-test
#
# Requires: gpg; rpmsign and rpm for --prepare-rpms; jq, python3, realpath,
# stat, sha256sum, git, release-evidence.sh,
# and verify-resource-publisher-attestation.py when publishing production-pass
# provenance. RPM artifacts must already carry their governed signatures so
# their evidence-bound bytes are never rewritten here.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
KEY_ID="${MAGIC_MESH_SIGN_KEY:-Magic Mesh Release Signing}"

die() {
  echo "sign-release: $*" >&2
  exit 2
}

usage() {
  cat <<'USAGE'
usage: sign-release.sh --prepare-rpms RPM...
       sign-release.sh --evidence EVIDENCE.json
                       [--resource-publisher-credential ABSOLUTE_PATH] ARTIFACT...
       sign-release.sh --self-test

--prepare-rpms is the only artifact-mutation stage. It accepts only regular,
non-symlink RPMs from one directory, embeds and verifies their RPM signatures,
and emits no release checksums, detached signature, or provenance.

Final publication requires an evidence envelope whose artifact list, SBOM
manifest, and gate manifest must all be present and unchanged.  The same
directory then receives a signed PROVENANCE.json publication. Production-pass
evidence additionally requires a fresh resource-publisher HMAC proof verified
with the dedicated credential at ABSOLUTE_PATH, or resource-publisher-hmac
under CREDENTIALS_DIRECTORY. Supplying artifacts without either explicit mode
is always refused.
USAGE
}

PREPARE_TRANSACTION_ACTIVE=0
PREPARE_TRANSACTION_ARTIFACTS=()
PREPARE_TRANSACTION_BACKUPS=()

rollback_rpm_preparation() {
  local index
  [ "$PREPARE_TRANSACTION_ACTIVE" -eq 1 ] || return 0
  PREPARE_TRANSACTION_ACTIVE=0
  for index in "${!PREPARE_TRANSACTION_BACKUPS[@]}"; do
    if [ -e "${PREPARE_TRANSACTION_BACKUPS[$index]}" ]; then
      mv -fT -- "${PREPARE_TRANSACTION_BACKUPS[$index]}" \
        "${PREPARE_TRANSACTION_ARTIFACTS[$index]}" \
        || echo "sign-release: could not roll back prepared RPM: ${PREPARE_TRANSACTION_ARTIFACTS[$index]}" >&2
    fi
  done
}

prepare_rpms() {
  local expected_dir="" artifact resolved directory backup signing_fingerprint
  local -A seen=()
  local -a rpms=()

  [ "${#ARTIFACTS[@]}" -gt 0 ] || die "--prepare-rpms requires at least one RPM"
  command -v gpg >/dev/null 2>&1 || die "required command not found: gpg"
  command -v rpmsign >/dev/null 2>&1 || die "required command not found: rpmsign"
  command -v rpm >/dev/null 2>&1 || die "required command not found: rpm"

  # Validate the complete set before mutating the first RPM. A hostile later
  # argument must not leave an earlier artifact partially prepared.
  for artifact in "${ARTIFACTS[@]}"; do
    case "$(basename -- "$artifact")" in
      *.rpm) ;;
      *) die "--prepare-rpms accepts RPM artifacts only: $artifact" ;;
    esac
    [ -s "$artifact" ] && [ -f "$artifact" ] && [ ! -L "$artifact" ] \
      || die "RPM is not a non-empty regular, non-symlink file: $artifact"
    resolved="$(realpath -e -- "$artifact")" || die "could not resolve RPM: $artifact"
    [ -z "${seen[$resolved]:-}" ] || die "duplicate RPM argument: $artifact"
    seen[$resolved]=1
    directory="$(dirname -- "$resolved")"
    if [ -z "$expected_dir" ]; then
      expected_dir="$directory"
    else
      [ "$directory" = "$expected_dir" ] \
        || die "all RPMs prepared together must share one directory"
    fi
    rpm -qp --qf '%{NAME}\n' "$resolved" >/dev/null \
      || die "RPM could not be parsed before preparation: $artifact"
    rpms+=("$resolved")
  done

  signing_fingerprint="$(resolve_signing_fingerprint)"
  PREPARE_TRANSACTION_ARTIFACTS=()
  PREPARE_TRANSACTION_BACKUPS=()
  PREPARE_TRANSACTION_ACTIVE=1
  trap rollback_rpm_preparation EXIT
  for artifact in "${rpms[@]}"; do
    backup="$(mktemp --tmpdir="$expected_dir" .sign-release.rpm-backup.XXXXXX)"
    if ! cp --reflink=auto --preserve=all -- "$artifact" "$backup"; then
      rm -f -- "$backup"
      die "could not stage RPM rollback copy: $artifact"
    fi
    PREPARE_TRANSACTION_ARTIFACTS+=("$artifact")
    PREPARE_TRANSACTION_BACKUPS+=("$backup")
  done
  for artifact in "${rpms[@]}"; do
    rpmsign --define "_gpg_name $signing_fingerprint" --addsign "$artifact"
    rpm --checksig "$artifact" >/dev/null \
      || die "embedded RPM signature did not verify: $artifact"
  done
  PREPARE_TRANSACTION_ACTIVE=0
  trap - EXIT
  rm -f -- "${PREPARE_TRANSACTION_BACKUPS[@]}"
  echo "sign-release: prepared and verified ${#rpms[@]} RPM(s); no release bundle was emitted"
}

declare -A VERIFIED_INPUT_IDENTITIES=()
declare -A VERIFIED_INPUT_DESCRIPTORS=()

file_descriptor() {
  local path="$1" resolved size digest input_fd input_fd_path opened_identity path_identity
  [ -s "$path" ] || die "input is missing or empty: $path"
  [ -f "$path" ] || die "input is not a regular file: $path"
  resolved="$(realpath -e -- "$path")" || die "could not resolve input: $path"
  exec {input_fd}<"$resolved" || die "could not open input: $resolved"
  input_fd_path="/proc/$BASHPID/fd/$input_fd"
  opened_identity="$(stat -Lc '%d:%i' -- "$input_fd_path")" \
    || die "could not identify opened input: $resolved"
  path_identity="$(stat -Lc '%d:%i' -- "$resolved")" \
    || die "could not identify input pathname: $resolved"
  [ "$opened_identity" = "$path_identity" ] \
    || die "input pathname changed while it was opened: $resolved"
  size="$(stat -Lc '%s' -- "$input_fd_path")" \
    || die "could not stat opened input: $resolved"
  digest="$(sha256sum -- "$input_fd_path" | awk '{print $1}')"
  path_identity="$(stat -Lc '%d:%i' -- "$resolved")" \
    || die "input pathname disappeared while it was hashed: $resolved"
  exec {input_fd}<&-
  [ "$opened_identity" = "$path_identity" ] \
    || die "input pathname changed while it was hashed: $resolved"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "could not hash input: $resolved"
  jq -cn --arg path "$resolved" --arg sha256 "$digest" --argjson size "$size" \
    '{path: $path, size_bytes: $size, sha256: $sha256}'
}

record_verified_input() {
  local path="$1" descriptor="$2" resolved identity
  resolved="$(realpath -e -- "$path")" || die "could not resolve verified input: $path"
  identity="$(stat -Lc '%d:%i' -- "$resolved")" \
    || die "could not identify verified input: $resolved"
  [ -z "${VERIFIED_INPUT_IDENTITIES[$resolved]:-}" ] \
    || die "duplicate verified publication input: $resolved"
  VERIFIED_INPUT_IDENTITIES[$resolved]="$identity"
  VERIFIED_INPUT_DESCRIPTORS[$resolved]="$(jq -cS '{path, size_bytes, sha256}' <<<"$descriptor")"
}

assert_verified_input_unchanged() {
  local path="$1" resolved identity
  resolved="$(realpath -e -- "$path")" || die "verified input pathname disappeared: $path"
  [ -n "${VERIFIED_INPUT_IDENTITIES[$resolved]:-}" ] \
    || die "publication input was not identity-bound: $resolved"
  identity="$(stat -Lc '%d:%i' -- "$resolved")" \
    || die "could not identify publication input: $resolved"
  [ "$identity" = "${VERIFIED_INPUT_IDENTITIES[$resolved]}" ] \
    || die "verified input pathname was replaced before publication: $resolved"
}

require_provenance_inputs() {
  local evidence="$1"
  command -v jq >/dev/null 2>&1 || die "required command not found: jq"
  command -v realpath >/dev/null 2>&1 || die "required command not found: realpath"
  command -v stat >/dev/null 2>&1 || die "required command not found: stat"
  command -v sha256sum >/dev/null 2>&1 || die "required command not found: sha256sum"
  command -v python3 >/dev/null 2>&1 || die "required command not found: python3"
  [ -x "$SCRIPT_DIR/release-evidence.sh" ] || die "release-evidence.sh is unavailable"
  [ -f "$evidence" ] || die "evidence file is missing: $evidence"
  "$SCRIPT_DIR/release-evidence.sh" validate "$evidence" >/dev/null
}

enforce_production_publisher_attestation() {
  local evidence="$1" production revision evidence_revision before after
  local verifier="$SCRIPT_DIR/verify-resource-publisher-attestation.py"
  local -a verifier_args
  production="$(jq -er '.verdict.production | strings' "$evidence")" \
    || die "evidence has no production verdict"
  [ "$production" = "pass" ] || return 0

  command -v git >/dev/null 2>&1 || die "required command not found: git"
  command -v python3 >/dev/null 2>&1 || die "required command not found: python3"
  [ -x "$verifier" ] || die "resource-publisher attestation verifier is unavailable"
  revision="$(git -C "$SCRIPT_DIR/.." rev-parse --verify 'HEAD^{commit}' 2>/dev/null)" \
    || die "could not determine the signing checkout revision"
  [[ "$revision" =~ ^[0-9a-f]{40}$|^[0-9a-f]{64}$ ]] \
    || die "signing checkout revision is not an exact Git object ID"

  before="$(sha256sum -- "$evidence" | awk '{print $1}')"
  evidence_revision="$(jq -er '.source_commit | strings' "$evidence")" \
    || die "production evidence has no exact source revision"
  [[ "$evidence_revision" =~ ^[0-9A-Fa-f]{40}$|^[0-9A-Fa-f]{64}$ ]] \
    || die "production evidence source revision is not an exact Git object ID"
  [ "$evidence_revision" = "$revision" ] \
    || die "production evidence source revision does not match the signing checkout"
  verifier_args=(--evidence "$evidence" --expected-revision "$revision")
  if [ -n "$RESOURCE_PUBLISHER_CREDENTIAL" ]; then
    verifier_args+=(--credential "$RESOURCE_PUBLISHER_CREDENTIAL")
  fi
  "$verifier" "${verifier_args[@]}" >/dev/null \
    || die "production resource-publisher attestation did not verify"
  after="$(sha256sum -- "$evidence" | awk '{print $1}')"
  [ "$before" = "$after" ] || die "production evidence changed during attestation verification"
  VERIFIED_PRODUCTION_EVIDENCE_SHA256="$after"
}

resolve_signing_fingerprint() {
  local listing fingerprint count
  listing="$(gpg --batch --with-colons --fingerprint --list-secret-keys "$KEY_ID" 2>/dev/null)" \
    || die "secret key '$KEY_ID' not in this keyring — run on the release operator's machine"
  fingerprint="$(awk -F: '
    $1 == "sec" { primary = 1; next }
    primary && $1 == "fpr" { print toupper($10); primary = 0 }
  ' <<<"$listing")"
  count="$(sed '/^$/d' <<<"$fingerprint" | wc -l)"
  [ "$count" -eq 1 ] \
    || die "signing identity '$KEY_ID' must resolve to exactly one primary secret-key fingerprint"
  fingerprint="$(sed '/^$/d' <<<"$fingerprint")"
  [[ "$fingerprint" =~ ^[0-9A-F]{40}$|^[0-9A-F]{64}$ ]] \
    || die "signing identity '$KEY_ID' resolved to an invalid fingerprint"
  printf '%s\n' "$fingerprint"
}

verify_detached_signature_identity() {
  local signature="$1" payload="$2" expected_fingerprint="$3"
  local status valid_fingerprints signer_fingerprint primary_fingerprint
  status="$(gpg --batch --status-fd 1 --verify "$signature" "$payload" 2>/dev/null)" \
    || die "detached checksum signature did not verify"
  valid_fingerprints="$(awk '
    $1 == "[GNUPG:]" && $2 == "VALIDSIG" {
      signer = toupper($3)
      primary = toupper($NF)
      print signer ":" primary
    }
  ' <<<"$status")"
  [ "$(sed '/^$/d' <<<"$valid_fingerprints" | wc -l)" -eq 1 ] \
    || die "detached checksum signature did not yield exactly one valid signer"
  signer_fingerprint="${valid_fingerprints%%:*}"
  primary_fingerprint="${valid_fingerprints#*:}"
  [ "$signer_fingerprint" = "$expected_fingerprint" ] \
    || [ "$primary_fingerprint" = "$expected_fingerprint" ] \
    || die "detached checksum signature was produced by an unexpected signer"
}

assert_verified_evidence_unchanged() {
  local actual
  [ -n "$VERIFIED_PRODUCTION_EVIDENCE_SHA256" ] || return 0
  actual="$(sha256sum -- "$EVIDENCE" | awk '{print $1}')"
  [ "$actual" = "$VERIFIED_PRODUCTION_EVIDENCE_SHA256" ] \
    || die "production evidence changed after attestation verification"
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
  local evidence="$1" outdir="$2" provenance="$3" evidence_descriptor
  local sbom_path gate_path
  evidence="$(realpath -e -- "$evidence")"
  evidence_descriptor="$(file_descriptor "$evidence")"
  sbom_path="$(jq -r '.provenance.sbom_manifest.path' "$evidence")"
  gate_path="$(jq -r '.provenance.gate_manifest.path' "$evidence")"
  same_directory "$outdir" "$evidence" "$sbom_path" "$gate_path"

  jq -S \
    --argjson evidence "$evidence_descriptor" \
    '{schema_version: 1,
      evidence: $evidence,
      source_commit: .source_commit,
      artifacts: .artifacts,
      sbom_manifest: .provenance.sbom_manifest,
      gate_manifest: .provenance.gate_manifest,
      binding_sha256: .provenance.binding_sha256} +
     (if .provenance.resource_publisher_attestation == null then {}
      else {resource_publisher_attestation: .provenance.resource_publisher_attestation}
      end)' \
    "$evidence" >"$provenance"
}

verify_publication_artifacts() {
  local evidence="$1" expected actual artifact descriptor
  expected="$(jq -cS '[.[] | {path, size_bytes, sha256}] | sort_by(.path)' \
    <<<"$(jq -c '.artifacts' "$evidence")")"
  actual="[]"
  for artifact in "${ARTIFACTS[@]}"; do
    descriptor="$(file_descriptor "$artifact")"
    actual="$(jq -cS --argjson descriptor "$descriptor" \
      '. + [$descriptor] | sort_by(.path)' <<<"$actual")"
    record_verified_input "$artifact" "$descriptor"
  done
  [ "$actual" = "$expected" ] \
    || die "artifact arguments do not exactly match the evidence artifact set"

  for artifact in "${ARTIFACTS[@]}"; do
    case "$(basename -- "$artifact")" in
      *.rpm)
        command -v rpm >/dev/null 2>&1 || die "required command not found: rpm"
        rpm --checksig "$artifact" >/dev/null \
          || die "evidence-bound RPM has no verifiable embedded signature: $artifact"
        ;;
    esac
  done
}

write_sums() {
  local output="$1" outdir="$2" index path source resolved descriptor expected digest
  : >"$output"
  for index in "${!SUM_SOURCES[@]}"; do
    path="${SUM_LABELS[$index]}"
    source="${SUM_SOURCES[$index]}"
    resolved="$(realpath -e -- "$source")" || die "bundle input disappeared: $source"
    descriptor="$(file_descriptor "$resolved")"
    if [ -n "${VERIFIED_INPUT_DESCRIPTORS[$resolved]:-}" ]; then
      assert_verified_input_unchanged "$resolved"
      expected="${VERIFIED_INPUT_DESCRIPTORS[$resolved]}"
      [ "$(jq -cS '{path, size_bytes, sha256}' <<<"$descriptor")" = "$expected" ] \
        || die "verified input bytes changed before publication: $resolved"
    elif [ "$source" != "$PROVENANCE_READ_PATH" ]; then
      die "bundle input was not verified before publication: $resolved"
    fi
    digest="$(jq -er '.sha256' <<<"$descriptor")"
    printf '%s  %s\n' "$digest" "$(basename -- "$path")" >>"$output"
  done
}

fsync_open_file() {
  python3 - "$1" <<'PY'
import os
import sys

fd = os.open(sys.argv[1], os.O_RDONLY)
try:
    os.fsync(fd)
finally:
    os.close(fd)
PY
}

atomic_publish_noreplace() {
  local source="$1" destination="$2" expected_identity="$3"
  python3 - "$source" "$destination" "$expected_identity" <<'PY'
import ctypes
import errno
import os
import sys

source, destination, expected_identity = sys.argv[1:]
source_fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
try:
    source_stat = os.fstat(source_fd)
    actual_identity = f"{source_stat.st_dev}:{source_stat.st_ino}"
    if actual_identity != expected_identity:
        raise SystemExit(f"staged publication inode was replaced: {source}")
    os.fsync(source_fd)
finally:
    os.close(source_fd)

libc = ctypes.CDLL(None, use_errno=True)
renameat2 = libc.renameat2
renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p,
                      ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
renameat2.restype = ctypes.c_int
at_fdcwd = -100
rename_noreplace = 1
if renameat2(at_fdcwd, os.fsencode(source), at_fdcwd,
             os.fsencode(destination), rename_noreplace) != 0:
    error = ctypes.get_errno()
    if error == errno.EEXIST:
        raise SystemExit(f"publication destination already exists: {destination}")
    raise OSError(error, os.strerror(error), destination)
destination_stat = os.stat(destination, follow_symlinks=False)
published_identity = f"{destination_stat.st_dev}:{destination_stat.st_ino}"
if published_identity != expected_identity:
    raise SystemExit(f"published a replaced staging inode: {destination}")
directory_fd = os.open(os.path.dirname(destination), os.O_RDONLY | os.O_DIRECTORY)
try:
    os.fsync(directory_fd)
finally:
    os.close(directory_fd)
PY
}

remove_owned_inode_if_present() {
  local path="$1" expected_identity="$2"
  python3 - "$path" "$expected_identity" <<'PY'
import os
import sys

path, expected_identity = sys.argv[1:]
directory = os.path.dirname(path)
name = os.path.basename(path)
directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
try:
    try:
        current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    except FileNotFoundError:
        raise SystemExit(0)
    actual_identity = f"{current.st_dev}:{current.st_ino}"
    if actual_identity != expected_identity:
        raise SystemExit(
            f"refusing to remove non-transaction inode during rollback: {path}"
        )
    os.unlink(name, dir_fd=directory_fd)
    os.fsync(directory_fd)
finally:
    os.close(directory_fd)
PY
}

PUBLICATION_TRANSACTION_ACTIVE=0
PUBLICATION_PROVENANCE_TEMP=""
PUBLICATION_PROVENANCE_PATH=""
PUBLICATION_PROVENANCE_IDENTITY=""
PUBLICATION_SUMS_TEMP=""
PUBLICATION_SUMS_PATH=""
PUBLICATION_SUMS_IDENTITY=""
PUBLICATION_SIGNATURE_TEMP=""
PUBLICATION_SIGNATURE_PATH=""
PUBLICATION_SIGNATURE_IDENTITY=""

rollback_release_publication() {
  [ "$PUBLICATION_TRANSACTION_ACTIVE" -eq 1 ] || return 0
  PUBLICATION_TRANSACTION_ACTIVE=0
  remove_owned_inode_if_present \
    "$PUBLICATION_SIGNATURE_PATH" "$PUBLICATION_SIGNATURE_IDENTITY" || true
  remove_owned_inode_if_present \
    "$PUBLICATION_SIGNATURE_TEMP" "$PUBLICATION_SIGNATURE_IDENTITY" || true
  remove_owned_inode_if_present \
    "$PUBLICATION_SUMS_PATH" "$PUBLICATION_SUMS_IDENTITY" || true
  remove_owned_inode_if_present \
    "$PUBLICATION_SUMS_TEMP" "$PUBLICATION_SUMS_IDENTITY" || true
  remove_owned_inode_if_present \
    "$PUBLICATION_PROVENANCE_PATH" "$PUBLICATION_PROVENANCE_IDENTITY" || true
  remove_owned_inode_if_present \
    "$PUBLICATION_PROVENANCE_TEMP" "$PUBLICATION_PROVENANCE_IDENTITY" || true
}

assert_staged_inode() {
  local path="$1" expected_identity="$2" actual_identity
  actual_identity="$(stat -Lc '%d:%i' -- "$path")" \
    || die "staged publication inode disappeared: $path"
  [ "$actual_identity" = "$expected_identity" ] \
    || die "staged publication pathname was replaced: $path"
}

SUM_LABELS=()
SUM_SOURCES=()
PROVENANCE_READ_PATH=""

publish() {
  local outdir evidence_descriptor sbom_path gate_path descriptor path
  local provenance_temp sums_temp signature_temp provenance_fd sums_fd signature_fd
  local provenance_fd_path sums_fd_path signature_fd_path
  local provenance_identity sums_identity signature_identity sums_digest_before sums_digest_after signing_fingerprint
  local -a bundle_inputs=()
  [ -n "${EVIDENCE:-}" ] \
    || die "--evidence is mandatory; unsigned evidence gaps cannot enter the release signing path"
  EVIDENCE="$(realpath -e -- "$EVIDENCE")"
  evidence_descriptor="$(file_descriptor "$EVIDENCE")"
  require_provenance_inputs "$EVIDENCE"
  [ "$(file_descriptor "$EVIDENCE")" = "$evidence_descriptor" ] \
    || die "evidence changed while it was validated"
  record_verified_input "$EVIDENCE" "$evidence_descriptor"
  enforce_production_publisher_attestation "$EVIDENCE"
  outdir="$(dirname -- "$(realpath -e -- "${ARTIFACTS[0]}")")"
  same_directory "$outdir" "$EVIDENCE" "${ARTIFACTS[@]}"
  verify_publication_artifacts "$EVIDENCE"
  sbom_path="$(jq -r '.provenance.sbom_manifest.path' "$EVIDENCE")"
  gate_path="$(jq -r '.provenance.gate_manifest.path' "$EVIDENCE")"
  for path in "$sbom_path" "$gate_path"; do
    descriptor="$(file_descriptor "$path")"
    if [ "$path" = "$sbom_path" ]; then
      evidence_descriptor="$(jq -cS '.provenance.sbom_manifest | {path, size_bytes, sha256}' "$EVIDENCE")"
    else
      evidence_descriptor="$(jq -cS '.provenance.gate_manifest | {path, size_bytes, sha256}' "$EVIDENCE")"
    fi
    [ "$(jq -cS '{path, size_bytes, sha256}' <<<"$descriptor")" = "$evidence_descriptor" ] \
      || die "provenance manifest changed after evidence validation: $path"
    record_verified_input "$path" "$descriptor"
  done
  bundle_inputs=("${ARTIFACTS[@]}" "$EVIDENCE" "$sbom_path" "$gate_path" \
    "$outdir/PROVENANCE.json")
  # Basenames are the names carried by SHA256SUMS; duplicates would make
  # verification ambiguous, so refuse them instead of silently overwriting.
  [ "$(printf '%s\n' "${bundle_inputs[@]}" | xargs -r -n1 basename | sort -u | wc -l)" -eq "${#bundle_inputs[@]}" ] \
    || die "provenance bundle contains duplicate basenames"
  for path in "${ARTIFACTS[@]}" "$EVIDENCE" "$sbom_path" "$gate_path"; do
    assert_verified_input_unchanged "$path"
  done
  [ ! -e "$outdir/PROVENANCE.json" ] && [ ! -L "$outdir/PROVENANCE.json" ] \
    && [ ! -e "$outdir/SHA256SUMS" ] && [ ! -L "$outdir/SHA256SUMS" ] \
    && [ ! -e "$outdir/SHA256SUMS.asc" ] && [ ! -L "$outdir/SHA256SUMS.asc" ] \
    || die "publication outputs already exist; use a clean governed candidate directory"

  assert_verified_evidence_unchanged
  signing_fingerprint="$(resolve_signing_fingerprint)"
  provenance_temp="$(mktemp --tmpdir="$outdir" .sign-release.provenance.XXXXXX)"
  sums_temp="$(mktemp --tmpdir="$outdir" .sign-release.sums.XXXXXX)"
  signature_temp="$(mktemp --tmpdir="$outdir" .sign-release.signature.XXXXXX)"
  exec {provenance_fd}<>"$provenance_temp"
  exec {sums_fd}<>"$sums_temp"
  exec {signature_fd}<>"$signature_temp"
  provenance_fd_path="/proc/$BASHPID/fd/$provenance_fd"
  sums_fd_path="/proc/$BASHPID/fd/$sums_fd"
  signature_fd_path="/proc/$BASHPID/fd/$signature_fd"
  provenance_identity="$(stat -Lc '%d:%i' -- "$provenance_fd_path")"
  sums_identity="$(stat -Lc '%d:%i' -- "$sums_fd_path")"
  signature_identity="$(stat -Lc '%d:%i' -- "$signature_fd_path")"
  PUBLICATION_PROVENANCE_TEMP="$provenance_temp"
  PUBLICATION_PROVENANCE_PATH="$outdir/PROVENANCE.json"
  PUBLICATION_PROVENANCE_IDENTITY="$provenance_identity"
  PUBLICATION_SUMS_TEMP="$sums_temp"
  PUBLICATION_SUMS_PATH="$outdir/SHA256SUMS"
  PUBLICATION_SUMS_IDENTITY="$sums_identity"
  PUBLICATION_SIGNATURE_TEMP="$signature_temp"
  PUBLICATION_SIGNATURE_PATH="$outdir/SHA256SUMS.asc"
  PUBLICATION_SIGNATURE_IDENTITY="$signature_identity"
  PUBLICATION_TRANSACTION_ACTIVE=1
  trap rollback_release_publication EXIT
  prepare_provenance "$EVIDENCE" "$outdir" "$provenance_fd_path"
  fsync_open_file "$provenance_fd_path"
  assert_staged_inode "$provenance_temp" "$provenance_identity"

  assert_verified_evidence_unchanged
  SUM_LABELS=("${bundle_inputs[@]}")
  SUM_SOURCES=("${ARTIFACTS[@]}" "$EVIDENCE" "$sbom_path" "$gate_path" \
    "$provenance_fd_path")
  PROVENANCE_READ_PATH="$provenance_fd_path"
  write_sums "$sums_fd_path" "$outdir"
  fsync_open_file "$sums_fd_path"
  sums_digest_before="$(sha256sum -- "$sums_fd_path" | awk '{print $1}')"
  assert_staged_inode "$sums_temp" "$sums_identity"
  gpg --armor --detach-sign --local-user "$signing_fingerprint" --yes \
    --output "$signature_fd_path" "$sums_fd_path"
  fsync_open_file "$signature_fd_path"
  sums_digest_after="$(sha256sum -- "$sums_fd_path" | awk '{print $1}')"
  [ "$sums_digest_after" = "$sums_digest_before" ] \
    || die "checksum inode changed while GPG signed it"
  assert_staged_inode "$provenance_temp" "$provenance_identity"
  assert_staged_inode "$sums_temp" "$sums_identity"
  assert_staged_inode "$signature_temp" "$signature_identity"
  verify_detached_signature_identity \
    "$signature_fd_path" "$sums_fd_path" "$signing_fingerprint"
  assert_staged_inode "$provenance_temp" "$provenance_identity"
  assert_staged_inode "$sums_temp" "$sums_identity"
  assert_staged_inode "$signature_temp" "$signature_identity"
  atomic_publish_noreplace "$provenance_temp" "$outdir/PROVENANCE.json" \
    "$provenance_identity" \
    || die "could not publish provenance without replacing an existing path"
  atomic_publish_noreplace "$sums_temp" "$outdir/SHA256SUMS" \
    "$sums_identity" \
    || die "could not publish checksums without replacing an existing path"
  assert_staged_inode "$outdir/PROVENANCE.json" "$provenance_identity"
  assert_staged_inode "$outdir/SHA256SUMS" "$sums_identity"
  [ "$(sha256sum -- "$outdir/SHA256SUMS" | awk '{print $1}')" = "$sums_digest_before" ] \
    || die "published checksum bytes differ from the inode GPG signed"
  atomic_publish_noreplace "$signature_temp" "$outdir/SHA256SUMS.asc" \
    "$signature_identity" \
    || die "could not publish checksum signature without replacing an existing path"
  assert_staged_inode "$outdir/PROVENANCE.json" "$provenance_identity"
  assert_staged_inode "$outdir/SHA256SUMS" "$sums_identity"
  assert_staged_inode "$outdir/SHA256SUMS.asc" "$signature_identity"
  [ "$(sha256sum -- "$outdir/SHA256SUMS" | awk '{print $1}')" = "$sums_digest_before" ] \
    || die "published checksum bytes changed after signature publication"
  PUBLICATION_TRANSACTION_ACTIVE=0
  trap - EXIT
  exec {provenance_fd}<&-
  exec {sums_fd}<&-
  exec {signature_fd}<&-

  echo "sign-release: signed provenance bundle with ${#ARTIFACTS[@]} artifact(s); wrote $outdir/{PROVENANCE.json,SHA256SUMS{,.asc}}"
  echo "verify with:  (cd $outdir && sha256sum -c SHA256SUMS && gpg --verify SHA256SUMS.asc SHA256SUMS)"
}

self_test() {
  local work fakebin evidence unsigned_evidence unsigned_gpg_log manual_dir manual_gpg_log prepare_dir prepare_log rpm_log before_note preflight_dir preflight_before production_probe production_dir fake_gpg_log rc
  work="$(mktemp -d)"
  trap 'rm -rf -- "$work"' RETURN
  fakebin="$work/bin"
  mkdir -p -- "$fakebin"
  cat >"$fakebin/gpg" <<'FAKE_GPG'
#!/usr/bin/env bash
set -euo pipefail
if [ -n "${FAKE_GPG_LOG:-}" ]; then
  printf '%s\n' invoked >>"$FAKE_GPG_LOG"
fi
case " $* " in
  *" --list-secret-keys "*)
    printf '%s\n' 'sec:u:255:1:1111111111111111:0:0:::::::'
    printf '%s\n' 'fpr:::::::::1111111111111111111111111111111111111111:'
    exit 0
    ;;
esac
if [ -n "${FAKE_GPG_INPUT_LOG:-}" ]; then
  input="${@: -1}"
  printf '%s %s\n' "$(sha256sum -- "$input" | awk '{print $1}')" "$input" \
    >"$FAKE_GPG_INPUT_LOG"
fi
case " $* " in
  *" --verify "*)
    signer="${FAKE_GPG_VERIFY_FINGERPRINT:-1111111111111111111111111111111111111111}"
    printf '[GNUPG:] VALIDSIG %s 2026-08-11 0 4 0 1 10 00 %s\n' \
      "$signer" "$signer"
    exit 0
    ;;
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
  cat >"$fakebin/rpmsign" <<'FAKE_RPMSIGN'
#!/usr/bin/env bash
set -euo pipefail
artifact="${@: -1}"
printf '%s\n' "$artifact" >>"$FAKE_RPMSIGN_LOG"
printf '%s\n' 'embedded-rpm-signature' >>"$artifact"
if [ "${FAKE_RPMSIGN_FAIL_ON:-}" = "$artifact" ]; then
  exit 9
fi
FAKE_RPMSIGN
cat >"$fakebin/rpm" <<'FAKE_RPM'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "-qp" ] && [ "${2:-}" = "--qf" ] && [ "$#" -eq 4 ]; then
  if grep -Fq 'structurally-invalid-rpm' "$4"; then
    exit 1
  fi
  printf '%s\n' fake-package
  exit 0
fi
if [ "${1:-}" = "--checksig" ] && [ "$#" -eq 2 ]; then
  grep -Fq 'embedded-rpm-signature' "$2"
  printf '%s\n' "$2" >>"$FAKE_RPM_LOG"
  if [ -n "${FAKE_RPM_REPLACEMENT:-}" ]; then
    mv -- "$FAKE_RPM_REPLACEMENT" "$2"
  fi
  exit 0
fi
exit 2
FAKE_RPM
  chmod +x "$fakebin/gpg" "$fakebin/rpmsign" "$fakebin/rpm"
  "$SCRIPT_DIR/verify-resource-publisher-attestation.py" --self-test >/dev/null
  prepare_dir="$work/prepare"
  mkdir -p -- "$prepare_dir"
  printf 'first rpm\n' >"$prepare_dir/first.rpm"
  printf 'second rpm\n' >"$prepare_dir/second.rpm"
  printf 'must remain unchanged\n' >"$prepare_dir/operator-note.txt"
  before_note="$(sha256sum -- "$prepare_dir/operator-note.txt")"
  prepare_log="$prepare_dir/rpmsign-invocations"
  rpm_log="$prepare_dir/rpm-checks"
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPMSIGN_LOG="$prepare_log" FAKE_RPM_LOG="$rpm_log" \
    "$0" --prepare-rpms "$prepare_dir/first.rpm" "$prepare_dir/second.rpm" >/dev/null
  [ "$(grep -Fc 'embedded-rpm-signature' "$prepare_dir/first.rpm")" -eq 1 ] \
    && [ "$(grep -Fc 'embedded-rpm-signature' "$prepare_dir/second.rpm")" -eq 1 ] \
    && [ "$(wc -l <"$prepare_log")" -eq 2 ] \
    && [ "$(wc -l <"$rpm_log")" -eq 2 ] \
    && [ "$(sha256sum -- "$prepare_dir/operator-note.txt")" = "$before_note" ] \
    && [ ! -e "$prepare_dir/PROVENANCE.json" ] \
    && [ ! -e "$prepare_dir/SHA256SUMS" ] \
    && [ ! -e "$prepare_dir/SHA256SUMS.asc" ] \
    || die "self-test: RPM preparation mutated non-RPM input or emitted release output"
  preflight_dir="$work/prepare-preflight"
  mkdir -p -- "$preflight_dir"
  printf 'valid rpm bytes\n' >"$preflight_dir/first.rpm"
  printf 'structurally-invalid-rpm\n' >"$preflight_dir/second.rpm"
  preflight_before="$(sha256sum -- "$preflight_dir/first.rpm")"
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPMSIGN_LOG="$preflight_dir/rpmsign-invocations" \
    FAKE_RPM_LOG="$preflight_dir/rpm-checks" \
    "$0" --prepare-rpms "$preflight_dir/first.rpm" "$preflight_dir/second.rpm" \
    >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ "$(sha256sum -- "$preflight_dir/first.rpm")" = "$preflight_before" ] \
    && [ ! -e "$preflight_dir/rpmsign-invocations" ] \
    || die "self-test: prepare mutated an RPM before complete-set validation"
  local rollback_dir="$work/prepare-rollback"
  local rollback_first_before rollback_second_before
  mkdir -p -- "$rollback_dir"
  printf 'first rollback rpm\n' >"$rollback_dir/first.rpm"
  printf 'second rollback rpm\n' >"$rollback_dir/second.rpm"
  rollback_first_before="$(sha256sum -- "$rollback_dir/first.rpm")"
  rollback_second_before="$(sha256sum -- "$rollback_dir/second.rpm")"
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPMSIGN_LOG="$rollback_dir/rpmsign-invocations" \
    FAKE_RPM_LOG="$rollback_dir/rpm-checks" \
    FAKE_RPMSIGN_FAIL_ON="$rollback_dir/second.rpm" \
    "$0" --prepare-rpms "$rollback_dir/first.rpm" "$rollback_dir/second.rpm" \
    >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ "$(sha256sum -- "$rollback_dir/first.rpm")" = "$rollback_first_before" ] \
    && [ "$(sha256sum -- "$rollback_dir/second.rpm")" = "$rollback_second_before" ] \
    && [ -z "$(find "$rollback_dir" -maxdepth 1 -name '.sign-release.rpm-backup.*' -print -quit)" ] \
    || die "self-test: failed multi-RPM preparation retained a partial signature"
  manual_dir="$work/manual-bypass"
  mkdir -p -- "$manual_dir"
  printf 'manual artifact\n' >"$manual_dir/artifact.bin"
  manual_gpg_log="$manual_dir/gpg-invocations"
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test FAKE_GPG_LOG="$manual_gpg_log" \
    "$0" "$manual_dir/artifact.bin" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ ! -e "$manual_dir/PROVENANCE.json" ] \
    && [ ! -e "$manual_dir/SHA256SUMS" ] \
    && [ ! -e "$manual_dir/SHA256SUMS.asc" ] \
    && [ ! -e "$manual_gpg_log" ] \
    || die "self-test: evidence-free manual signing emitted output or invoked GPG"
  printf 'unsigned artifact\n' >"$work/unsigned.rpm"
  printf '{"packages":["a"]}\n' >"$work/sbom.json"
  printf '{"gates":["check"]}\n' >"$work/gates.json"
  unsigned_evidence="$work/unsigned-evidence.json"
  "$SCRIPT_DIR/release-evidence.sh" write --out "$unsigned_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$work/unsigned.rpm" --check policy=pass \
    --farm-job farm-1 --farm-slot slot-1 --sbom rpm=pass \
    --sbom-manifest "$work/sbom.json" --gate-manifest "$work/gates.json" \
    --fedora-target fedora-44=pass --live-gate live=unavailable \
    --unavailable 'live unavailable' --preview-verdict pass \
    --production-verdict not-promoted >/dev/null
  unsigned_gpg_log="$work/unsigned-gpg-invocations"
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_GPG_LOG="$unsigned_gpg_log" FAKE_RPM_LOG="$work/unsigned-rpm-checks" \
    "$0" --evidence "$unsigned_evidence" "$work/unsigned.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ ! -e "$work/PROVENANCE.json" ] \
    && [ ! -e "$work/SHA256SUMS" ] \
    && [ ! -e "$work/SHA256SUMS.asc" ] \
    && [ ! -e "$unsigned_gpg_log" ] \
    || die "self-test: evidence-bound unsigned RPM emitted publication output or invoked GPG"

  local replacement_dir="$work/replaced-after-verification"
  local replacement_evidence="$replacement_dir/evidence.json"
  local replacement_gpg_log="$replacement_dir/gpg-invocations"
  mkdir -p -- "$replacement_dir"
  cp -- "$prepare_dir/first.rpm" "$replacement_dir/candidate.rpm"
  # Identical bytes on a different inode prove that digest-only revalidation
  # cannot hide a same-name replacement after rpm --checksig returns.
  cp -- "$replacement_dir/candidate.rpm" "$replacement_dir/substitute.rpm"
  printf '{"packages":["candidate"]}\n' >"$replacement_dir/sbom.json"
  printf '{"gates":["candidate"]}\n' >"$replacement_dir/gates.json"
  "$SCRIPT_DIR/release-evidence.sh" write --out "$replacement_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$replacement_dir/candidate.rpm" --check policy=pass \
    --farm-job farm-1 --farm-slot slot-1 --sbom rpm=pass \
    --sbom-manifest "$replacement_dir/sbom.json" \
    --gate-manifest "$replacement_dir/gates.json" \
    --fedora-target fedora-44=pass --live-gate live=unavailable \
    --unavailable 'live unavailable' --preview-verdict pass \
    --production-verdict not-promoted >/dev/null
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_GPG_LOG="$replacement_gpg_log" \
    FAKE_RPM_LOG="$replacement_dir/rpm-checks" \
    FAKE_RPM_REPLACEMENT="$replacement_dir/substitute.rpm" \
    "$0" --evidence "$replacement_evidence" \
    "$replacement_dir/candidate.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ ! -e "$replacement_dir/PROVENANCE.json" ] \
    && [ ! -e "$replacement_dir/SHA256SUMS" ] \
    && [ ! -e "$replacement_dir/SHA256SUMS.asc" ] \
    && [ ! -e "$replacement_gpg_log" ] \
    || die "self-test: post-verification RPM pathname replacement reached publication"

  local output_race_dir="$work/output-race"
  local output_race_bin="$work/output-race-bin"
  local output_race_evidence="$output_race_dir/evidence.json"
  local output_race_victim="$work/output-race-victim"
  local output_race_victim_before
  mkdir -p -- "$output_race_dir" "$output_race_bin"
  cp -- "$prepare_dir/first.rpm" "$output_race_dir/candidate.rpm"
  printf '{"packages":["candidate"]}\n' >"$output_race_dir/sbom.json"
  printf '{"gates":["candidate"]}\n' >"$output_race_dir/gates.json"
  printf 'must never be truncated\n' >"$output_race_victim"
  output_race_victim_before="$(sha256sum -- "$output_race_victim")"
  "$SCRIPT_DIR/release-evidence.sh" write --out "$output_race_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$output_race_dir/candidate.rpm" --check policy=pass \
    --farm-job farm-1 --farm-slot slot-1 --sbom rpm=pass \
    --sbom-manifest "$output_race_dir/sbom.json" \
    --gate-manifest "$output_race_dir/gates.json" \
    --fedora-target fedora-44=pass --live-gate live=unavailable \
    --unavailable 'live unavailable' --preview-verdict pass \
    --production-verdict not-promoted >/dev/null
  cat >"$output_race_bin/python3" <<'FAKE_PYTHON'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "-" ] && [ "${3:-}" = "$FAKE_SHA256SUMS_DESTINATION" ]; then
  ln -s -- "$FAKE_SHA256SUMS_VICTIM" "$FAKE_SHA256SUMS_DESTINATION"
fi
exec /usr/bin/python3 "$@"
FAKE_PYTHON
  chmod +x "$output_race_bin/python3"
  set +e
  PATH="$output_race_bin:$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPM_LOG="$output_race_dir/rpm-checks" \
    FAKE_SHA256SUMS_DESTINATION="$output_race_dir/SHA256SUMS" \
    FAKE_SHA256SUMS_VICTIM="$output_race_victim" \
    "$0" --evidence "$output_race_evidence" \
    "$output_race_dir/candidate.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ -L "$output_race_dir/SHA256SUMS" ] \
    && [ "$(readlink -- "$output_race_dir/SHA256SUMS")" = "$output_race_victim" ] \
    && [ "$(sha256sum -- "$output_race_victim")" = "$output_race_victim_before" ] \
    && [ ! -e "$output_race_dir/PROVENANCE.json" ] \
    && [ ! -e "$output_race_dir/SHA256SUMS.asc" ] \
    && [ -z "$(find "$output_race_dir" -maxdepth 1 -name '.sign-release.*' -print -quit)" ] \
    || die "self-test: hostile SHA256SUMS output race left signer-owned partial publication"

  local signer_substitution_dir="$work/signer-substitution"
  local signer_substitution_evidence="$signer_substitution_dir/evidence.json"
  mkdir -p -- "$signer_substitution_dir"
  cp -- "$prepare_dir/first.rpm" "$signer_substitution_dir/candidate.rpm"
  printf '{"packages":["candidate"]}\n' >"$signer_substitution_dir/sbom.json"
  printf '{"gates":["candidate"]}\n' >"$signer_substitution_dir/gates.json"
  "$SCRIPT_DIR/release-evidence.sh" write --out "$signer_substitution_evidence" \
    --source-commit 0123456789abcdef0123456789abcdef01234567 \
    --artifact "$signer_substitution_dir/candidate.rpm" --check policy=pass \
    --farm-job farm-1 --farm-slot slot-1 --sbom rpm=pass \
    --sbom-manifest "$signer_substitution_dir/sbom.json" \
    --gate-manifest "$signer_substitution_dir/gates.json" \
    --fedora-target fedora-44=pass --live-gate live=unavailable \
    --unavailable 'live unavailable' --preview-verdict pass \
    --production-verdict not-promoted >/dev/null
  set +e
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPM_LOG="$signer_substitution_dir/rpm-checks" \
    FAKE_GPG_VERIFY_FINGERPRINT=2222222222222222222222222222222222222222 \
    "$0" --evidence "$signer_substitution_evidence" \
    "$signer_substitution_dir/candidate.rpm" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ ! -e "$signer_substitution_dir/PROVENANCE.json" ] \
    && [ ! -e "$signer_substitution_dir/SHA256SUMS" ] \
    && [ ! -e "$signer_substitution_dir/SHA256SUMS.asc" ] \
    && [ -z "$(find "$signer_substitution_dir" -maxdepth 1 -name '.sign-release.*' -print -quit)" ] \
    || die "self-test: substituted detached-signature identity reached publication"

  cp -- "$prepare_dir/first.rpm" "$work/a.rpm"
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
  local checksum_gpg_input_log="$work/checksum-gpg-input"
  PATH="$fakebin:$PATH" MAGIC_MESH_SIGN_KEY=test \
    FAKE_RPM_LOG="$work/final-rpm-checks" \
    FAKE_GPG_INPUT_LOG="$checksum_gpg_input_log" \
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
  [ "$(awk '{print $1}' "$checksum_gpg_input_log")" = \
      "$(sha256sum -- "$work/SHA256SUMS" | awk '{print $1}')" ] \
    && awk '$2 ~ "^/proc/[0-9]+/fd/[0-9]+$" { found = 1 } END { exit !found }' \
      "$checksum_gpg_input_log" \
    || die "self-test: GPG did not sign the exact staged checksum inode"
  production_dir="$work/revision-mismatch"
  mkdir -p -- "$production_dir"
  printf 'hostile artifact\n' >"$production_dir/artifact.bin"
  production_probe="$production_dir/evidence.json"
  printf '%s\n' \
    '{"source_commit":"0000000000000000000000000000000000000000","verdict":{"production":"pass"}}' \
    >"$production_probe"
  fake_gpg_log="$production_dir/gpg-invocations"
  set +e
  (
    unset CREDENTIALS_DIRECTORY
    # Isolate the post-schema-validation signer seam: publish must reject the
    # revision before preparing provenance, checksums, or invoking GPG.
    require_provenance_inputs() { :; }
    EVIDENCE="$production_probe"
    ARTIFACTS=("$production_dir/artifact.bin")
    RESOURCE_PUBLISHER_CREDENTIAL=""
    VERIFIED_PRODUCTION_EVIDENCE_SHA256=""
    PATH="$fakebin:$PATH" FAKE_GPG_LOG="$fake_gpg_log" publish
  ) >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || die "self-test: production signing accepted a mismatched evidence revision"
  [ ! -e "$production_dir/PROVENANCE.json" ] \
    && [ ! -e "$production_dir/SHA256SUMS" ] \
    && [ ! -e "$production_dir/SHA256SUMS.asc" ] \
    && [ ! -e "$fake_gpg_log" ] \
    || die "self-test: revision mismatch emitted signing output or invoked GPG"
  echo "sign-release: self-test passed (artifact, signer identity, and atomic rollback boundaries fail closed)"
}

EVIDENCE=""
PREPARE_RPMS=0
RESOURCE_PUBLISHER_CREDENTIAL=""
VERIFIED_PRODUCTION_EVIDENCE_SHA256=""
ARTIFACTS=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prepare-rpms)
      [ "$PREPARE_RPMS" -eq 0 ] || die "--prepare-rpms may be supplied only once"
      PREPARE_RPMS=1
      shift
      ;;
    --evidence)
      [ "$#" -ge 2 ] || die "--evidence needs a file"
      [ -z "$EVIDENCE" ] || die "--evidence may be supplied only once"
      EVIDENCE="$2"
      shift 2
      ;;
    --resource-publisher-credential)
      [ "$#" -ge 2 ] || die "--resource-publisher-credential needs an absolute path"
      [ -z "$RESOURCE_PUBLISHER_CREDENTIAL" ] \
        || die "--resource-publisher-credential may be supplied only once"
      [[ "$2" = /* ]] || die "--resource-publisher-credential needs an absolute path"
      RESOURCE_PUBLISHER_CREDENTIAL="$2"
      shift 2
      ;;
    --self-test) self_test; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; ARTIFACTS+=("$@"); break ;;
    -*) die "unknown option: $1" ;;
    *) ARTIFACTS+=("$1"); shift ;;
  esac
done

[ "$PREPARE_RPMS" -eq 0 ] || [ -z "$EVIDENCE" ] \
  || die "--prepare-rpms and --evidence are mutually exclusive"
[ "$PREPARE_RPMS" -eq 0 ] || [ -z "$RESOURCE_PUBLISHER_CREDENTIAL" ] \
  || die "--resource-publisher-credential is invalid with --prepare-rpms"
[ "$PREPARE_RPMS" -eq 0 ] || { prepare_rpms; exit 0; }
[ -z "$RESOURCE_PUBLISHER_CREDENTIAL" ] || [ -n "$EVIDENCE" ] \
  || die "--resource-publisher-credential requires --evidence"
[ -n "$EVIDENCE" ] \
  || die "--evidence is mandatory; unsigned evidence gaps cannot enter the release signing path"
[ "${#ARTIFACTS[@]}" -gt 0 ] || { usage >&2; exit 2; }
for artifact in "${ARTIFACTS[@]}"; do
  [ -s "$artifact" ] || die "missing or empty artifact: $artifact"
  [ -f "$artifact" ] || die "artifact is not a regular file: $artifact"
done
publish
