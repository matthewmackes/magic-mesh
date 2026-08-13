#!/usr/bin/env bash
# WL-FUNC-020 — authenticate and stage the Cuttlefish guest payload handoff.
#
# The production Ansible role consumes only the private staged copies emitted by
# this gate.  That closes the gap between checking controller-side paths and apt
# (or the guest runtime) reopening mutable source names later.
set -euo pipefail
umask 077

readonly PROJECT_KEY=/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh
readonly PROJECT_FINGERPRINT=B546CC2EF9489F1899657AC9E6C820DAFBD1B07A
readonly MAX_DECLARATION_BYTES=262144
readonly MAX_SIGNATURE_BYTES=65536
readonly MAX_PAYLOAD_BYTES=$((1024 * 1024 * 1024))

fail() {
    echo "cuttlefish guest payload: $*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: verify-guest-payload.sh --declaration FILE --signature FILE \
  --readiness-relay FILE --vdi-agent FILE --stage-dir DIR \
  [--guest-package FILE ...]
       verify-guest-payload.sh --self-test

Authenticates one signed Cuttlefish release declaration and atomically stages
the exact declared readiness relay, VDI agent, and guest package bytes.
EOF
}

verify_payload() {
    local declaration=$1 signature=$2 relay=$3 agent=$4 stage_dir=$5
    local key=$6 fingerprint=$7
    shift 7

    [[ ! -e "$stage_dir" ]] || fail "stage directory already exists"
    local parent
    parent=$(dirname -- "$stage_dir")
    [[ -d "$parent" && ! -L "$parent" ]] || fail "stage parent is missing or substituted"

    local work
    work=$(mktemp -d -- "$parent/.cuttlefish-payload.XXXXXX")

    python3 - "$declaration" "$signature" "$relay" "$agent" "$work" \
        "$MAX_DECLARATION_BYTES" "$MAX_SIGNATURE_BYTES" "$MAX_PAYLOAD_BYTES" "$@" <<'PY'
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path

declaration, signature, relay, agent, work = map(Path, sys.argv[1:6])
max_declaration, max_signature, max_payload = map(int, sys.argv[6:9])
packages = [Path(value) for value in sys.argv[9:]]
digest_re = re.compile(r"sha256:[0-9a-f]{64}\Z")
name_re = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+:-]{0,254}\Z")


def reject(message):
    raise SystemExit(f"cuttlefish guest payload: {message}")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            reject(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def exact(value, fields, label):
    if not isinstance(value, dict) or set(value) != set(fields):
        reject(f"{label} fields are not exact")


def stable_copy(source, destination, maximum, label):
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(source, flags)
    except OSError as error:
        reject(f"{label} cannot be opened safely: {error}")
    digest = hashlib.sha256()
    try:
        before = os.fstat(descriptor)
        named = os.stat(source, follow_symlinks=False)
        identity = lambda value: (
            value.st_dev, value.st_ino, value.st_mode, value.st_nlink,
            value.st_uid, value.st_gid, value.st_size, value.st_mtime_ns,
            value.st_ctime_ns,
        )
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            reject(f"{label} must be a single-link regular file")
        if before.st_size <= 0 or before.st_size > maximum:
            reject(f"{label} size is outside the bounded range")
        if identity(before) != identity(named):
            reject(f"{label} changed before admission")
        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with os.fdopen(os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as output:
            remaining = before.st_size
            while remaining:
                chunk = os.read(descriptor, min(1024 * 1024, remaining))
                if not chunk:
                    reject(f"{label} was truncated during admission")
                output.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            output.flush()
            os.fsync(output.fileno())
        if os.read(descriptor, 1):
            reject(f"{label} grew during admission")
        after = os.fstat(descriptor)
        closed_name = os.stat(source, follow_symlinks=False)
        if identity(after) != identity(before) or identity(closed_name) != identity(before):
            reject(f"{label} changed during admission")
    finally:
        os.close(descriptor)
    return "sha256:" + digest.hexdigest()


stable_copy(declaration, work / "release.json", max_declaration, "release declaration")
stable_copy(signature, work / "release.json.asc", max_signature, "release signature")

try:
    document = json.loads(
        (work / "release.json").read_text(encoding="utf-8"),
        object_pairs_hook=unique_object,
        parse_constant=lambda value: reject(f"non-finite JSON number: {value}"),
    )
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    reject(f"release declaration is invalid: {error}")

exact(document, {
    "schema_version", "kind", "release_id", "compatibility_version",
    "source_revision", "provider_identity", "image_identity", "artifacts",
}, "declaration")
if type(document["schema_version"]) is not int or document["schema_version"] != 2:
    reject("unsupported declaration schema_version")
if document["kind"] != "cuttlefish_guest_payload_release":
    reject("unsupported declaration kind")
for field in ("release_id", "compatibility_version"):
    if not isinstance(document[field], str) or not name_re.fullmatch(document[field]):
        reject(f"{field} is malformed")
revision_re = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
if not isinstance(document["source_revision"], str) or not revision_re.fullmatch(document["source_revision"]):
    reject("source_revision is not a full lowercase Git object ID")
if not isinstance(document["provider_identity"], str) or not name_re.fullmatch(document["provider_identity"]):
    reject("provider_identity is malformed")
image_identity = document["image_identity"]
exact(image_identity, {"id", "sha256"}, "image_identity")
if not isinstance(image_identity["id"], str) or not name_re.fullmatch(image_identity["id"]):
    reject("image_identity.id is malformed")
if not isinstance(image_identity["sha256"], str) or not digest_re.fullmatch(image_identity["sha256"]):
    reject("image_identity.sha256 is malformed")
if image_identity["sha256"] == "sha256:" + "0" * 64:
    reject("image_identity.sha256 is null")

artifacts = document["artifacts"]
exact(artifacts, {"readiness_relay", "vdi_agent", "guest_packages"}, "artifacts")


def descriptor(value, label):
    exact(value, {"name", "sha256"}, label)
    if not isinstance(value["name"], str) or not name_re.fullmatch(value["name"]):
        reject(f"{label}.name is malformed")
    if not isinstance(value["sha256"], str) or not digest_re.fullmatch(value["sha256"]):
        reject(f"{label}.sha256 is malformed")
    if value["sha256"] == "sha256:" + "0" * 64:
        reject(f"{label}.sha256 is null")
    return value["name"], value["sha256"]


relay_name, relay_digest = descriptor(artifacts["readiness_relay"], "readiness_relay")
agent_name, agent_digest = descriptor(artifacts["vdi_agent"], "vdi_agent")
package_entries = artifacts["guest_packages"]
if not isinstance(package_entries, list) or not package_entries:
    reject("guest_packages must be a non-empty array")
declared_packages = [descriptor(value, f"guest_packages[{index}]") for index, value in enumerate(package_entries)]
if len({name for name, _ in declared_packages}) != len(declared_packages):
    reject("guest package names are not unique")
if len(packages) != len(declared_packages):
    reject("configured guest package count differs from the signed declaration")

actual = [(relay, relay_name, relay_digest, work / "payload" / "readiness-relay"),
          (agent, agent_name, agent_digest, work / "payload" / "vdi-agent")]
for source, declared_name, expected, destination in actual:
    if source.name != declared_name:
        reject(f"configured payload name differs from declaration: {source.name}")
    if stable_copy(source, destination, max_payload, declared_name) != expected:
        reject(f"payload digest mismatch: {declared_name}")

for index, (source, declared) in enumerate(zip(packages, declared_packages)):
    name, expected = declared
    if source.name != name:
        reject(f"configured guest package order/name differs at index {index}")
    if stable_copy(source, work / "payload" / "packages" / name, max_payload, name) != expected:
        reject(f"guest package digest mismatch: {name}")
PY
    local admission_status=$?
    if ((admission_status != 0)); then
        rm -rf -- "$work"
        return "$admission_status"
    fi

    local keyring="$work/release-key.gpg" status="$work/gpg.status"
    if ! gpg --batch --no-options --dearmor --output "$keyring" "$key" >/dev/null 2>&1; then
        echo "cuttlefish guest payload: trusted release key cannot be materialized" >&2
        rm -rf -- "$work"
        return 1
    fi
    gpgv --status-fd 1 --keyring "$keyring" "$work/release.json.asc" "$work/release.json" \
        >"$status" 2>/dev/null || {
            echo "cuttlefish guest payload: release declaration signature is invalid" >&2
            rm -rf -- "$work"
            return 1
        }
    grep -Fq "[GNUPG:] VALIDSIG $fingerprint " "$status" || {
        echo "cuttlefish guest payload: release declaration signer is not the pinned project authority" >&2
        rm -rf -- "$work"
        return 1
    }

    mv -- "$work/payload" "$stage_dir"
    chmod 0700 "$stage_dir" "$stage_dir/packages"
    rm -rf -- "$work"
}

self_test() {
    MCNF_CUTTLEFISH_FIXTURE=$(mktemp -d)
    trap 'rm -rf -- "$MCNF_CUTTLEFISH_FIXTURE"' EXIT
    local fixture=$MCNF_CUTTLEFISH_FIXTURE
    export GNUPGHOME="$fixture/gnupg"
    mkdir -m 700 "$GNUPGHOME" "$fixture/source" "$fixture/stages"
    gpg --batch --pinentry-mode loopback --passphrase '' \
        --quick-generate-key 'Cuttlefish payload fixture <fixture@example.invalid>' \
        ed25519 sign 1d >/dev/null 2>&1
    local fingerprint
    fingerprint=$(gpg --batch --with-colons --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')
    gpg --batch --armor --export >"$fixture/key.asc"
    printf 'readiness relay bytes\n' >"$fixture/source/readiness-relay.sh"
    printf 'vdi agent bytes\n' >"$fixture/source/mcnf-cuttlefish-vdi-agent"
    printf 'guest package bytes\n' >"$fixture/source/cuttlefish-base.deb"
    local relay_digest agent_digest package_digest
    relay_digest=sha256:$(sha256sum "$fixture/source/readiness-relay.sh" | awk '{print $1}')
    agent_digest=sha256:$(sha256sum "$fixture/source/mcnf-cuttlefish-vdi-agent" | awk '{print $1}')
    package_digest=sha256:$(sha256sum "$fixture/source/cuttlefish-base.deb" | awk '{print $1}')
    python3 - "$fixture/release.json" "$relay_digest" "$agent_digest" "$package_digest" <<'PY'
import json, sys
document = {
    "schema_version": 2,
    "kind": "cuttlefish_guest_payload_release",
    "release_id": "fixture-r1",
    "compatibility_version": "2026.08.1",
    "source_revision": "0123456789abcdef0123456789abcdef01234567",
    "provider_identity": "provider-fixture",
    "image_identity": {
        "id": "android-image-fixture",
        "sha256": "sha256:" + "1" * 64,
    },
    "artifacts": {
        "readiness_relay": {"name": "readiness-relay.sh", "sha256": sys.argv[2]},
        "vdi_agent": {"name": "mcnf-cuttlefish-vdi-agent", "sha256": sys.argv[3]},
        "guest_packages": [{"name": "cuttlefish-base.deb", "sha256": sys.argv[4]}],
    },
}
open(sys.argv[1], "w", encoding="utf-8").write(json.dumps(document, separators=(",", ":")))
PY
    gpg --batch --armor --detach-sign "$fixture/release.json"
    verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/good" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb"
    [[ -f "$fixture/stages/good/readiness-relay" && -f "$fixture/stages/good/vdi-agent" \
        && -f "$fixture/stages/good/packages/cuttlefish-base.deb" ]] \
        || fail "self-test did not stage the complete payload"

    printf 'substituted package\n' >"$fixture/source/cuttlefish-base.deb"
    if verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/substituted" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb" >/dev/null 2>&1; then
        fail "self-test admitted a substituted guest package"
    fi
    [[ ! -e "$fixture/stages/substituted" ]] \
        || fail "rejected payload escaped into an installable stage"
    rm "$fixture/source/cuttlefish-base.deb"
    if verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/missing" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb" >/dev/null 2>&1; then
        fail "self-test admitted a missing guest package"
    fi

    printf 'guest package bytes\n' >"$fixture/source/cuttlefish-base.deb"
    cp "$fixture/source/mcnf-cuttlefish-vdi-agent" "$fixture/source/agent.original"
    printf 'same signed name, different agent bytes\n' >"$fixture/source/mcnf-cuttlefish-vdi-agent"
    if verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/agent-mismatch" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb" >/dev/null 2>&1; then
        fail "self-test admitted substituted VDI-agent bytes"
    fi
    mv "$fixture/source/agent.original" "$fixture/source/mcnf-cuttlefish-vdi-agent"

    ln "$fixture/source/readiness-relay.sh" "$fixture/source/readiness-relay.alias"
    if verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/mutable-relay" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb" >/dev/null 2>&1; then
        fail "self-test admitted a readiness relay with a mutable hard-link alias"
    fi
    rm "$fixture/source/readiness-relay.alias"

    cp "$fixture/release.json" "$fixture/release.signed"
    printf '\n' >>"$fixture/release.json"
    if verify_payload "$fixture/release.json" "$fixture/release.json.asc" \
        "$fixture/source/readiness-relay.sh" "$fixture/source/mcnf-cuttlefish-vdi-agent" \
        "$fixture/stages/tampered-declaration" "$fixture/key.asc" "$fingerprint" \
        "$fixture/source/cuttlefish-base.deb" >/dev/null 2>&1; then
        fail "self-test admitted a declaration changed after signing"
    fi
    mv "$fixture/release.signed" "$fixture/release.json"
    for rejected in missing agent-mismatch mutable-relay tampered-declaration; do
        [[ ! -e "$fixture/stages/$rejected" ]] \
            || fail "rejected $rejected payload escaped into an installable stage"
    done
    echo "Cuttlefish signed guest payload self-test passed"
}

if [[ ${BASH_SOURCE[0]} != "$0" ]]; then
    return 0
fi

if [[ ${1:-} == --self-test ]]; then
    [[ $# -eq 1 ]] || { usage >&2; exit 2; }
    self_test
    exit 0
fi

declaration='' signature='' relay='' agent='' stage_dir=''
packages=()
while (($#)); do
    case $1 in
        --declaration) declaration=${2:-}; shift 2 ;;
        --signature) signature=${2:-}; shift 2 ;;
        --readiness-relay) relay=${2:-}; shift 2 ;;
        --vdi-agent) agent=${2:-}; shift 2 ;;
        --stage-dir) stage_dir=${2:-}; shift 2 ;;
        --guest-package) packages+=("${2:-}"); shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
[[ -n $declaration && -n $signature && -n $relay && -n $agent && -n $stage_dir ]] \
    || { usage >&2; exit 2; }
[[ -r $PROJECT_KEY ]] || fail "installed project release key is unavailable"
verify_payload "$declaration" "$signature" "$relay" "$agent" "$stage_dir" \
    "$PROJECT_KEY" "$PROJECT_FINGERPRINT" "${packages[@]}"
echo "Cuttlefish signed guest payload staged: $stage_dir"
