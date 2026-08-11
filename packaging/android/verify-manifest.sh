#!/usr/bin/env bash
# WL-FUNC-020 — verify the immutable AOSP starter image/package manifest.
#
# This is a contents/provenance gate, not a guest-inventory or Cuttlefish boot
# claim. The manifest says what the pinned image was built to contain; only a
# guest-owned AndroidAppInventory may report an installed or launchable app.
set -euo pipefail

MAX_MANIFEST_BYTES=65536

fail() {
    echo "android image manifest: $*" >&2
    return 1
}

verify_manifest() {
    local manifest=${1:?manifest path is required}
    python3 - "$manifest" "$MAX_MANIFEST_BYTES" <<'PY'
import json
import os
import re
import stat
import sys
from pathlib import Path

path = Path(sys.argv[1])
MAX_MANIFEST_BYTES = int(sys.argv[2])
MAX_ID_BYTES = 128
MAX_VERSION_BYTES = 128
MAX_U64 = (1 << 64) - 1
IDENTITY = re.compile(r"[A-Za-z0-9._:-]{1,128}\Z")
VERSION = re.compile(r"[A-Za-z0-9._+:-]{1,128}\Z")
DIGEST = re.compile(r"sha256:([0-9a-f]{64})\Z")
STARTER = (
    ("browser", "com.android.browser"),
    ("calendar", "com.android.calendar"),
    ("camera", "com.android.camera2"),
    ("clock", "com.android.deskclock"),
    ("contacts", "com.android.contacts"),
    ("files", "com.android.documentsui"),
    ("gallery", "com.android.gallery3d"),
    ("calculator", "com.android.calculator2"),
    ("settings", "com.android.settings"),
)


def reject(message):
    raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            reject(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def exact_fields(value, expected, label):
    if not isinstance(value, dict):
        reject(f"{label} must be an object")
    actual = set(value)
    expected = set(expected)
    if actual != expected:
        omitted = sorted(expected - actual)
        unknown = sorted(actual - expected)
        reject(f"{label} fields are not exact (omitted={omitted}, unknown={unknown})")


def bounded_identity(value, label):
    if not isinstance(value, str) or len(value.encode("ascii", "ignore")) != len(value):
        reject(f"{label} is not an ASCII identity")
    if len(value.encode("ascii")) > MAX_ID_BYTES or not IDENTITY.fullmatch(value):
        reject(f"{label} is blank, oversized, or unsafe")


def descriptor_identity(metadata):
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_nlink,
        metadata.st_uid,
        metadata.st_gid,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def read_manifest():
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            reject("manifest is not a regular file")
        if before.st_nlink != 1:
            reject("manifest has an external hard-link alias")
        if before.st_size <= 0 or before.st_size > MAX_MANIFEST_BYTES:
            reject(f"manifest size is outside the bounded range: {before.st_size}")
        body = os.read(descriptor, MAX_MANIFEST_BYTES + 1)
        if len(body) != before.st_size:
            reject("manifest changed or was not read completely")
        after = os.fstat(descriptor)
        if descriptor_identity(after) != descriptor_identity(before):
            reject("manifest identity changed while being read")
        return body.decode("utf-8")
    finally:
        os.close(descriptor)


def main():
    try:
        document = json.loads(
            read_manifest(),
            object_pairs_hook=unique_object,
            parse_constant=lambda value: reject(f"non-finite JSON number: {value}"),
        )
        exact_fields(document, {"schema_version", "image_provenance", "packages"}, "manifest")
        if type(document["schema_version"]) is not int or document["schema_version"] != 1:
            reject("unsupported schema_version")

        provenance = document["image_provenance"]
        exact_fields(
            provenance,
            {"catalog_revision", "image_digest", "image_id", "source_revision"},
            "image_provenance",
        )
        for field in ("image_id", "source_revision", "catalog_revision"):
            bounded_identity(provenance[field], f"image_provenance.{field}")
        digest = provenance["image_digest"]
        match = DIGEST.fullmatch(digest) if isinstance(digest, str) else None
        if match is None or set(match.group(1)) == {"0"}:
            reject("image_provenance.image_digest is not a non-zero lowercase sha256 digest")

        packages = document["packages"]
        if not isinstance(packages, list) or len(packages) != len(STARTER):
            reject(f"packages must contain exactly {len(STARTER)} entries")
        seen_apps = set()
        seen_package_ids = set()
        for index, package in enumerate(packages):
            label = f"packages[{index}]"
            exact_fields(package, {"app", "package_id", "version"}, label)
            app = package["app"]
            package_id = package["package_id"]
            if (app, package_id) != STARTER[index]:
                reject(
                    f"{label} is not the canonical starter entry "
                    f"{STARTER[index][0]}/{STARTER[index][1]}"
                )
            if app in seen_apps:
                reject(f"duplicate starter app identity: {app}")
            if package_id in seen_package_ids:
                reject(f"duplicate package identity: {package_id}")
            seen_apps.add(app)
            seen_package_ids.add(package_id)

            version = package["version"]
            exact_fields(version, {"version_name", "version_code"}, f"{label}.version")
            version_name = version["version_name"]
            if (
                not isinstance(version_name, str)
                or len(version_name.encode("ascii", "ignore")) != len(version_name)
                or len(version_name.encode("ascii")) > MAX_VERSION_BYTES
                or not VERSION.fullmatch(version_name)
            ):
                reject(f"{label}.version.version_name is malformed")
            version_code = version["version_code"]
            if type(version_code) is not int or not 0 < version_code <= MAX_U64:
                reject(f"{label}.version.version_code is not a positive u64")

        print(f"OK: {path} — schema=1 provenance-bound starter packages={len(packages)}")
    except (OSError, UnicodeError, ValueError, TypeError) as error:
        print(f"FATAL: {path}: {error}", file=sys.stderr)
        return 1
    return 0


raise SystemExit(main())
PY
}

hostile_manifest_hardlink_cannot_alias_package_authority() {
    local fixture=$1
    ln -- "$fixture/valid.json" "$fixture/aliased.json"
    if verify_manifest "$fixture/valid.json" >/dev/null 2>&1; then
        fail "self-test admitted a manifest with a hostile hard-link alias"
    fi
}

self_test() {
    android_manifest_fixture=$(mktemp -d)
    trap 'rm -rf -- "$android_manifest_fixture"' EXIT

    python3 - "$android_manifest_fixture/valid.json" <<'PY'
import json
import sys
from pathlib import Path

starter = [
    ("browser", "com.android.browser"),
    ("calendar", "com.android.calendar"),
    ("camera", "com.android.camera2"),
    ("clock", "com.android.deskclock"),
    ("contacts", "com.android.contacts"),
    ("files", "com.android.documentsui"),
    ("gallery", "com.android.gallery3d"),
    ("calculator", "com.android.calculator2"),
    ("settings", "com.android.settings"),
]
document = {
    "schema_version": 1,
    "image_provenance": {
        "image_id": "aosp-cuttlefish-test",
        "image_digest": "sha256:" + "0123456789abcdef" * 4,
        "source_revision": "aosp-source-test",
        "catalog_revision": "starter-catalog-v1",
    },
    "packages": [
        {
            "app": app,
            "package_id": package_id,
            "version": {"version_name": "2026.08.1", "version_code": 1},
        }
        for app, package_id in starter
    ],
}
Path(sys.argv[1]).write_text(json.dumps(document, separators=(",", ":")), encoding="utf-8")
PY

    verify_manifest "$android_manifest_fixture/valid.json" >/dev/null

    python3 - "$android_manifest_fixture/valid.json" "$android_manifest_fixture/omitted.json" \
        "$android_manifest_fixture/omitted-identity.json" "$android_manifest_fixture/unknown.json" \
        "$android_manifest_fixture/duplicate.json" "$android_manifest_fixture/installed.json" \
        "$android_manifest_fixture/provenance.json" <<'PY'
import copy
import json
import sys
from pathlib import Path

valid = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))

omitted = copy.deepcopy(valid)
omitted["packages"].pop()

omitted_identity = copy.deepcopy(valid)
del omitted_identity["packages"][0]["package_id"]

unknown = copy.deepcopy(valid)
unknown["packages"][0]["package_id"] = "com.android.unknown"

duplicate = copy.deepcopy(valid)
duplicate["packages"][1]["package_id"] = duplicate["packages"][0]["package_id"]

installed = copy.deepcopy(valid)
installed["packages"][0]["installed"] = True

bad_provenance = copy.deepcopy(valid)
del bad_provenance["image_provenance"]["image_digest"]

for output, document in zip(
    sys.argv[2:], (omitted, omitted_identity, unknown, duplicate, installed, bad_provenance)
):
    Path(output).write_text(json.dumps(document, separators=(",", ":")), encoding="utf-8")
PY

    local candidate
    for candidate in omitted omitted-identity unknown duplicate installed provenance; do
        if verify_manifest "$android_manifest_fixture/$candidate.json" >/dev/null 2>&1; then
            fail "self-test accepted $candidate manifest"
        fi
    done
    hostile_manifest_hardlink_cannot_alias_package_authority "$android_manifest_fixture"
    echo "Android image manifest verification self-tests passed"
}

usage() {
    cat <<'EOF'
Usage: packaging/android/verify-manifest.sh PATH
       packaging/android/verify-manifest.sh --self-test

Validate the immutable AOSP starter image/package manifest. This gate does not
claim that a Cuttlefish guest is booted or that any package is installed.
EOF
}

case "${1:-}" in
    --self-test)
        [ "$#" -eq 1 ] || { usage >&2; exit 2; }
        self_test
        ;;
    -h|--help)
        [ "$#" -eq 1 ] || { usage >&2; exit 2; }
        usage
        ;;
    "")
        usage >&2
        exit 2
        ;;
    *)
        [ "$#" -eq 1 ] || { usage >&2; exit 2; }
        verify_manifest "$1"
        ;;
esac
