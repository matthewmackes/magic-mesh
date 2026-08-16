#!/usr/bin/env bash
# WL-FUNC-020 — produce the canonical signed Cuttlefish guest-payload contract.
set -euo pipefail
umask 077

PRODUCER_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
readonly PRODUCER_DIR
readonly PROJECT_SIGNING_IDENTITY=${MAGIC_MESH_SIGN_KEY:-Magic Mesh Release Signing}
readonly PROJECT_PRIMARY_FINGERPRINT=06B1C27EA0E08A225155EB3314018AA1497DDC7C
readonly IMAGE_RECEIPT_TOOL="$PRODUCER_DIR/produce-image-receipt.py"

producer_fail() {
    echo "cuttlefish guest payload producer: $*" >&2
    return 1
}

usage() {
    cat <<'EOF'
Usage: produce-guest-payload-declaration.sh \
  --release-id ID --compatibility-version VERSION \
  --source-revision FULL_GIT_COMMIT --provider-identity ID \
  --image-receipt FILE --image-source-kind registry|artifact \
  --image-original-source SOURCE --image-architecture amd64|arm64 \
  --android-release-id ID --image-compatibility-id ID \
  --source-epoch EPOCH [--image-media-type TYPE] [--image-artifact-format FORMAT] \
  --readiness-relay FILE --vdi-agent FILE --guest-package FILE [...] \
  --output-dir NEW_DIRECTORY
       produce-guest-payload-declaration.sh --self-test

Hashes the exact Cuttlefish packages, readiness relay, and VDI agent, emits a
canonical release.json, and signs it with the existing project release
authority selected by MAGIC_MESH_SIGN_KEY (default: Magic Mesh Release Signing).
The output directory must not exist. Private-key paths are intentionally not
accepted.
EOF
}

resolve_signer() {
    local identity=$1 expected=$2 listing primary count
    listing=$(gpg --batch --with-colons --fingerprint --list-secret-keys "$identity" 2>/dev/null) \
        || { producer_fail "secret key '$identity' is unavailable; run on the release operator machine"; return 1; }
    primary=$(awk -F: '
        $1 == "sec" { need_fingerprint = 1; next }
        need_fingerprint && $1 == "fpr" { print toupper($10); need_fingerprint = 0 }
    ' <<<"$listing")
    count=$(sed '/^$/d' <<<"$primary" | wc -l)
    [[ $count -eq 1 ]] \
        || { producer_fail "release identity must resolve to exactly one primary secret key"; return 1; }
    primary=$(sed '/^$/d' <<<"$primary")
    [[ $primary == "$expected" ]] \
        || { producer_fail "release identity does not match the governed project authority"; return 1; }
    printf '%s\n' "$primary"
}

verify_signature_identity() {
    local signature=$1 declaration=$2 expected=$3 status identities
    status=$(gpg --batch --status-fd 1 --verify "$signature" "$declaration" 2>/dev/null) \
        || { producer_fail "detached declaration signature did not verify"; return 1; }
    identities=$(awk '
        $1 == "[GNUPG:]" && $2 == "VALIDSIG" {
            print toupper($3) ":" toupper($NF)
        }
    ' <<<"$status")
    [[ $(sed '/^$/d' <<<"$identities" | wc -l) -eq 1 ]] \
        || { producer_fail "declaration signature did not yield exactly one signer"; return 1; }
    [[ ${identities%%:*} == "$expected" || ${identities#*:} == "$expected" ]] \
        || { producer_fail "declaration signature was produced by an unexpected signer"; return 1; }
}

publish_noreplace() {
    python3 - "$1" "$2" <<'PY'
import ctypes
import errno
import os
import sys

source, destination = os.fsencode(sys.argv[1]), os.fsencode(sys.argv[2])
libc = ctypes.CDLL(None, use_errno=True)
renameat2 = getattr(libc, "renameat2", None)
if renameat2 is None:
    raise SystemExit("cuttlefish guest payload producer: renameat2 is unavailable")
renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
renameat2.restype = ctypes.c_int
if renameat2(-100, source, -100, destination, 1) != 0:  # RENAME_NOREPLACE
    error = ctypes.get_errno()
    if error == errno.EEXIST:
        raise SystemExit("cuttlefish guest payload producer: output directory already exists")
    raise SystemExit(f"cuttlefish guest payload producer: atomic publication failed: {os.strerror(error)}")
PY
}

produce_declaration() {
    local release_id=$1 compatibility=$2 source_revision=$3 provider_identity=$4
    local image_receipt=$5 relay=$6 agent=$7 output_dir=$8
    local signer_identity=$9 expected_fingerprint=${10}
    shift 10
    local -a packages=("$@")
    local parent output_name work signer

    command -v gpg >/dev/null 2>&1 \
        || { producer_fail "required command not found: gpg"; return 1; }
    command -v python3 >/dev/null 2>&1 \
        || { producer_fail "required command not found: python3"; return 1; }
    [[ ${#packages[@]} -gt 0 ]] \
        || { producer_fail "at least one guest package is required"; return 1; }
    [[ ! -e $output_dir && ! -L $output_dir ]] \
        || { producer_fail "output directory already exists"; return 1; }
    parent=$(dirname -- "$output_dir")
    output_name=$(basename -- "$output_dir")
    [[ -d $parent && ! -L $parent && $output_name != . && $output_name != .. ]] \
        || { producer_fail "output parent is missing, substituted, or unsafe"; return 1; }

    # Resolve and pin the existing operator key before creating any candidate
    # output. There is intentionally no private-key path or alternate keyring.
    signer=$(resolve_signer "$signer_identity" "$expected_fingerprint") || return 1
    work=$(mktemp -d -- "$parent/.cuttlefish-declaration.XXXXXX") \
        || { producer_fail "could not create private candidate directory"; return 1; }

    if ! python3 - "$work/release.json" "$release_id" "$compatibility" \
        "$source_revision" "$provider_identity" "$image_receipt" \
        "$relay" "$agent" "${packages[@]}" <<'PY'
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path

output = Path(sys.argv[1])
release_id, compatibility, source_revision, provider_identity = sys.argv[2:6]
image_receipt = Path(sys.argv[6])
relay, agent = map(Path, sys.argv[7:9])
packages = [Path(value) for value in sys.argv[9:]]
identity_re = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+:-]{0,254}\Z")
revision_re = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
digest_re = re.compile(r"sha256:([0-9a-f]{64})\Z")

def reject(message):
    raise SystemExit(f"cuttlefish guest payload producer: {message}")

for label, value in (("release_id", release_id), ("compatibility_version", compatibility),
                     ("provider_identity", provider_identity)):
    if not identity_re.fullmatch(value):
        reject(f"{label} is malformed")
if not revision_re.fullmatch(source_revision):
    reject("source_revision must be a full lowercase Git object ID")
try:
    image = json.loads(image_receipt.read_text(encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    reject(f"inspected image receipt is invalid: {error}")
required_image = {
    "android_release_id", "architecture", "commit_epoch", "compatibility_id",
    "digest", "format", "kind", "media_type", "original_source",
    "platform_digest", "provider_identity", "schema_version", "source_kind",
    "source_revision",
}
if not isinstance(image, dict) or set(image) != required_image:
    reject("inspected image receipt fields are not exact")
if image["kind"] != "mcnf-cuttlefish-image-receipt" or image["schema_version"] != 1:
    reject("inspected image receipt schema is unsupported")
if image["source_revision"] != source_revision or image["provider_identity"] != provider_identity:
    reject("image receipt authority does not match the declaration")
match = digest_re.fullmatch(image.get("digest", ""))
if match is None or set(match.group(1)) == {"0"}:
    reject("image receipt digest must be a non-zero lowercase sha256 digest")

def descriptor(path, label):
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as error:
        reject(f"{label} cannot be opened safely: {error}")
    digest = hashlib.sha256()
    try:
        before = os.fstat(fd)
        named = os.stat(path, follow_symlinks=False)
        identity = lambda value: (value.st_dev, value.st_ino, value.st_mode, value.st_nlink,
                                  value.st_uid, value.st_gid, value.st_size,
                                  value.st_mtime_ns, value.st_ctime_ns)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size <= 0:
            reject(f"{label} must be a non-empty single-link regular file")
        if before.st_size > 1024 * 1024 * 1024:
            reject(f"{label} exceeds the 1 GiB producer bound")
        if identity(before) != identity(named):
            reject(f"{label} changed before hashing")
        remaining = before.st_size
        while remaining:
            chunk = os.read(fd, min(1024 * 1024, remaining))
            if not chunk:
                reject(f"{label} was truncated while hashing")
            digest.update(chunk)
            remaining -= len(chunk)
        if os.read(fd, 1):
            reject(f"{label} grew while hashing")
        after = os.fstat(fd)
        closed_name = os.stat(path, follow_symlinks=False)
        if identity(after) != identity(before) or identity(closed_name) != identity(before):
            reject(f"{label} changed while hashing")
    finally:
        os.close(fd)
    if not identity_re.fullmatch(path.name):
        reject(f"{label} basename is malformed")
    return {"name": path.name, "sha256": "sha256:" + digest.hexdigest()}

package_descriptors = [descriptor(path, f"guest package {index}") for index, path in enumerate(packages)]
package_names = [entry["name"] for entry in package_descriptors]
if len(set(package_names)) != len(package_names):
    reject("guest package basenames are not unique")

document = {
    "schema_version": 3,
    "kind": "cuttlefish_guest_payload_release",
    "release_id": release_id,
    "compatibility_version": compatibility,
    "source_revision": source_revision,
    "provider_identity": provider_identity,
    "image_identity": image,
    "artifacts": {
        "readiness_relay": descriptor(relay, "readiness relay"),
        "vdi_agent": descriptor(agent, "VDI agent"),
        "guest_packages": package_descriptors,
    },
}
with output.open("x", encoding="utf-8") as stream:
    json.dump(document, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
    stream.flush()
    os.fsync(stream.fileno())
PY
    then
        rm -rf -- "$work"
        return 1
    fi

    if ! gpg --batch --armor --detach-sign --local-user "$signer" --yes \
        --output "$work/release.json.asc" "$work/release.json"; then
        rm -rf -- "$work"
        producer_fail "could not sign the declaration with the governed authority"
        return 1
    fi
    if ! verify_signature_identity "$work/release.json.asc" "$work/release.json" "$expected_fingerprint"; then
        rm -rf -- "$work"
        return 1
    fi
    chmod 0600 "$work/release.json" "$work/release.json.asc"
    if ! publish_noreplace "$work" "$output_dir"; then
        rm -rf -- "$work"
        return 1
    fi
    echo "Cuttlefish guest payload declaration produced: $output_dir"
}

self_test() {
    local fixture fingerprint revision image_digest before
    fixture=$(mktemp -d)
    MCNF_CUTTLEFISH_PRODUCER_FIXTURE=$fixture
    trap 'rm -rf -- "$MCNF_CUTTLEFISH_PRODUCER_FIXTURE"' EXIT
    export GNUPGHOME="$fixture/gnupg"
    mkdir -m 700 "$GNUPGHOME" "$fixture/input" "$fixture/output"
    gpg --batch --pinentry-mode loopback --passphrase '' \
        --quick-generate-key 'Cuttlefish producer fixture <fixture@example.invalid>' \
        ed25519 sign 1d >/dev/null 2>&1
    fingerprint=$(gpg --batch --with-colons --fingerprint --list-secret-keys \
        'Cuttlefish producer fixture' | awk -F: '$1 == "fpr" { print toupper($10); exit }')
    gpg --batch --armor --export "$fingerprint" >"$fixture/key.asc"
    printf 'relay bytes\n' >"$fixture/input/readiness-relay.sh"
    printf 'agent bytes\n' >"$fixture/input/mcnf-cuttlefish-vdi-agent"
    printf 'base package bytes\n' >"$fixture/input/cuttlefish-base.deb"
    printf 'user package bytes\n' >"$fixture/input/cuttlefish-user.deb"
    revision=0123456789abcdef0123456789abcdef01234567
    image_digest=sha256:$(printf 'governed image\n' | sha256sum | awk '{print $1}')
    python3 - "$fixture/image-receipt.json" "$revision" "$image_digest" <<'PY'
import json, sys
json.dump({
    "android_release_id":"android-fixture-r1", "architecture":"amd64",
    "commit_epoch":1700000000, "compatibility_id":"cuttlefish-fixture-v1",
    "digest":sys.argv[3], "format":"android-cuttlefish-image-archive",
    "kind":"mcnf-cuttlefish-image-receipt",
    "media_type":"application/vnd.mcnf.cuttlefish.image.v1+tar",
    "original_source":"/fixture/cuttlefish-image.tar", "platform_digest":None,
    "provider_identity":"provider-fixture", "schema_version":1,
    "source_kind":"artifact", "source_revision":sys.argv[2],
}, open(sys.argv[1], "w"), sort_keys=True, separators=(",", ":"))
PY

    produce_declaration fixture-r1 2026.08.1 "$revision" provider-fixture \
        "$fixture/image-receipt.json" "$fixture/input/readiness-relay.sh" \
        "$fixture/input/mcnf-cuttlefish-vdi-agent" "$fixture/output/good" \
        'Cuttlefish producer fixture' "$fingerprint" \
        "$fixture/input/cuttlefish-base.deb" "$fixture/input/cuttlefish-user.deb" >/dev/null

    # Source the production verifier's function without weakening its fixed CLI
    # trust boundary, then consume the producer's exact contract end to end.
    # shellcheck source=packaging/android/verify-guest-payload.sh
    source "$PRODUCER_DIR/verify-guest-payload.sh"
    verify_payload "$fixture/output/good/release.json" "$fixture/output/good/release.json.asc" \
        "$fixture/input/readiness-relay.sh" "$fixture/input/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stage" "$fixture/key.asc" "$fingerprint" \
        "$fixture/input/cuttlefish-base.deb" "$fixture/input/cuttlefish-user.deb"
    [[ -f $fixture/stage/packages/cuttlefish-base.deb \
        && -f $fixture/stage/packages/cuttlefish-user.deb ]] \
        || { producer_fail "producer/verifier integration did not stage the complete package set"; return 1; }

    before=$(sha256sum "$fixture/output/good/release.json" | awk '{print $1}')
    if produce_declaration fixture-r2 2026.08.1 "$revision" provider-fixture \
        "$fixture/image-receipt.json" "$fixture/input/readiness-relay.sh" \
        "$fixture/input/mcnf-cuttlefish-vdi-agent" "$fixture/output/good" \
        'Cuttlefish producer fixture' "$fingerprint" \
        "$fixture/input/cuttlefish-base.deb" >/dev/null 2>&1; then
        producer_fail "producer replaced an existing output directory"
    fi
    [[ $(sha256sum "$fixture/output/good/release.json" | awk '{print $1}') == "$before" ]] \
        || { producer_fail "rejected replacement changed the published declaration"; return 1; }

    if produce_declaration fixture-r3 2026.08.1 "$revision" provider-fixture \
        "$fixture/image-receipt.json" "$fixture/input/missing.deb" \
        "$fixture/input/mcnf-cuttlefish-vdi-agent" "$fixture/output/missing" \
        'Cuttlefish producer fixture' "$fingerprint" \
        "$fixture/input/cuttlefish-base.deb" >/dev/null 2>&1; then
        producer_fail "producer admitted a missing artifact"
    fi
    [[ ! -e $fixture/output/missing ]] \
        || { producer_fail "missing input published an output"; return 1; }

    if produce_declaration fixture-r4 2026.08.1 "$revision" provider-fixture \
        "$fixture/image-receipt.json" "$fixture/input/readiness-relay.sh" \
        "$fixture/input/mcnf-cuttlefish-vdi-agent" "$fixture/output/no-key" \
        'missing release key' "$fingerprint" "$fixture/input/cuttlefish-base.deb" \
        >/dev/null 2>&1; then
        producer_fail "producer admitted a missing signing key"
    fi
    [[ ! -e $fixture/output/no-key ]] \
        || { producer_fail "missing key published an output"; return 1; }

    python3 - "$fixture/output/good/release.json" <<'PY'
import json, sys
path = sys.argv[1]
document = json.load(open(path, encoding="utf-8"))
for field, replacement in (
    ("source_revision", "f" * 40),
    ("provider_identity", "substituted-provider"),
):
    changed = json.loads(json.dumps(document))
    changed[field] = replacement
    with open(path + ".tampered", "w", encoding="utf-8") as stream:
        json.dump(changed, stream, sort_keys=True, separators=(",", ":"))
    # The caller checks the original detached signature against each mutation.
    if __import__("subprocess").run(
        ["gpg", "--batch", "--verify", path + ".asc", path + ".tampered"],
        stdout=__import__("subprocess").DEVNULL,
        stderr=__import__("subprocess").DEVNULL,
    ).returncode == 0:
        raise SystemExit(f"signed declaration did not bind {field}")
changed = json.loads(json.dumps(document))
changed["image_identity"]["android_release_id"] = "substituted-image"
with open(path + ".tampered", "w", encoding="utf-8") as stream:
    json.dump(changed, stream, sort_keys=True, separators=(",", ":"))
if __import__("subprocess").run(
    ["gpg", "--batch", "--verify", path + ".asc", path + ".tampered"],
    stdout=__import__("subprocess").DEVNULL,
    stderr=__import__("subprocess").DEVNULL,
).returncode == 0:
    raise SystemExit("signed declaration did not bind image identity")
PY
    echo "Cuttlefish guest payload producer/verifier hostile integration passed"
}

if [[ ${1:-} == --self-test ]]; then
    [[ $# -eq 1 ]] || { usage >&2; exit 2; }
    self_test
    exit 0
fi

release_id='' compatibility='' source_revision='' source_epoch='' provider_identity=''
image_receipt='' image_source_kind='' image_original_source='' image_architecture=''
android_release_id='' image_compatibility_id='' image_media_type='application/octet-stream'
image_artifact_format='android-cuttlefish-host-package'
relay='' agent='' output_dir=''
packages=()
while (($#)); do
    case $1 in
        --release-id) release_id=${2:-}; shift 2 ;;
        --compatibility-version) compatibility=${2:-}; shift 2 ;;
        --source-revision) source_revision=${2:-}; shift 2 ;;
        --provider-identity) provider_identity=${2:-}; shift 2 ;;
        --source-epoch) source_epoch=${2:-}; shift 2 ;;
        --image-receipt) image_receipt=${2:-}; shift 2 ;;
        --image-source-kind) image_source_kind=${2:-}; shift 2 ;;
        --image-original-source) image_original_source=${2:-}; shift 2 ;;
        --image-architecture) image_architecture=${2:-}; shift 2 ;;
        --android-release-id) android_release_id=${2:-}; shift 2 ;;
        --image-compatibility-id) image_compatibility_id=${2:-}; shift 2 ;;
        --image-media-type) image_media_type=${2:-}; shift 2 ;;
        --image-artifact-format) image_artifact_format=${2:-}; shift 2 ;;
        --readiness-relay) relay=${2:-}; shift 2 ;;
        --vdi-agent) agent=${2:-}; shift 2 ;;
        --guest-package) packages+=("${2:-}"); shift 2 ;;
        --output-dir) output_dir=${2:-}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
[[ -n $release_id && -n $compatibility && -n $source_revision \
    && -n $source_epoch && -n $provider_identity && -n $image_receipt \
    && -n $image_source_kind && -n $image_original_source && -n $image_architecture \
    && -n $android_release_id && -n $image_compatibility_id \
    && -n $relay && -n $agent && -n $output_dir ]] || { usage >&2; exit 2; }

checkout_revision=$(git -C "$PRODUCER_DIR/../.." rev-parse --verify 'HEAD^{commit}' 2>/dev/null) \
    || { producer_fail "could not determine the producer checkout revision"; exit 1; }
[[ $source_revision == "$checkout_revision" ]] \
    || { producer_fail "source revision does not match the producer checkout"; exit 1; }

inspected_receipt=$(mktemp)
trap 'rm -f -- "$inspected_receipt"' EXIT
python3 "$IMAGE_RECEIPT_TOOL" --repo "$PRODUCER_DIR/../.." inspect \
    --source-kind "$image_source_kind" --original-source "$image_original_source" \
    --architecture "$image_architecture" --provider-identity "$provider_identity" \
    --android-release-id "$android_release_id" --compatibility-id "$image_compatibility_id" \
    --source-revision "$source_revision" --commit-epoch "$source_epoch" \
    --media-type "$image_media_type" --artifact-format "$image_artifact_format" \
    --receipt "$image_receipt" >"$inspected_receipt" \
    || { producer_fail "Cuttlefish image receipt admission failed"; exit 1; }

produce_declaration "$release_id" "$compatibility" "$source_revision" \
    "$provider_identity" "$inspected_receipt" "$relay" "$agent" \
    "$output_dir" "$PROJECT_SIGNING_IDENTITY" "$PROJECT_PRIMARY_FINGERPRINT" \
    "${packages[@]}"
