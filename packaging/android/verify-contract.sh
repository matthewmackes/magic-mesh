#!/usr/bin/env bash
# WL-FUNC-020 — focused Android/Cuttlefish packaging contract entrypoint.
#
# This is a static and fixture gate. It proves that the packaging manifest,
# guest-tool receipt, and placement-readiness verifier are wired together. It
# never starts Cuttlefish and never upgrades tooling evidence into guest boot,
# package-installation, display, or launch proof.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ANDROID="$ROOT/packaging/android"
MANIFEST_VERIFY="$ANDROID/verify-manifest.sh"
TOOL_READINESS="$ANDROID/record-guest-tool-readiness.sh"
PLACEMENT_READINESS="$ROOT/install-helpers/verify-cuttlefish-readiness.py"
PACKAGE_MANIFEST="$ROOT/crates/mesh/mackesd/Cargo.toml"
PROJECT_RELEASE_KEY="$ROOT/packaging/repo/RPM-GPG-KEY-magic-mesh"
PROJECT_RELEASE_FINGERPRINT="B546CC2EF9489F1899657AC9E6C820DAFBD1B07A"

fail() {
    echo "verify-android-contract: $*" >&2
    exit 1
}

[ -x "$MANIFEST_VERIFY" ] || fail "Android manifest verifier is not executable"
[ -x "$TOOL_READINESS" ] || fail "guest-tool readiness recorder is not executable"
[ -x "$PLACEMENT_READINESS" ] || fail "Cuttlefish placement verifier is not executable"

bash -n "$MANIFEST_VERIFY" "$TOOL_READINESS" "$0"
python3 -m py_compile "$PLACEMENT_READINESS"

# Keep the packaging path explicit: readiness must call the real Android
# manifest verifier, not a second permissive parser or an always-success stub.
grep -Fq 'packaging/android/verify-manifest.sh' "$PLACEMENT_READINESS" \
    || fail "placement verifier is not wired to the Android manifest verifier"

# The shipped producer and sole placement consumer must move together. Keep the
# exact v3 release/payload fields visible here so a partial schema change fails
# this package contract before reaching a seat.
grep -Fq 'readonly SCHEMA_VERSION=3' "$TOOL_READINESS" \
    || fail "guest-tool receipt producer is not on schema v3"
grep -Fq 'SCHEMA_VERSION = 3' "$PLACEMENT_READINESS" \
    || fail "placement verifier is not on schema v3"
for field in release_artifact_digest package_manifest_digest installed_guest_payload_digest compatibility_version; do
    grep -Fq "\"$field\"" "$TOOL_READINESS" \
        || fail "guest-tool receipt producer is missing v3 field: $field"
    grep -Fq "\"$field\"" "$PLACEMENT_READINESS" \
        || fail "placement verifier is missing v3 field: $field"
done
grep -Fq '"signature_path"' "$PLACEMENT_READINESS" \
    || fail "placement verifier is missing signed-artifact field: signature_path"
grep -Fq '"--dearmor"' "$PLACEMENT_READINESS" \
    || fail "placement verifier does not materialize the shipped armored key safely"
grep -Fq '"gpgv"' "$PLACEMENT_READINESS" \
    || fail "placement verifier does not authenticate release signatures"
grep -Fq "$PROJECT_RELEASE_FINGERPRINT" "$PLACEMENT_READINESS" \
    || fail "placement verifier does not pin the project release signer"
grep -Fq '/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh' "$PLACEMENT_READINESS" \
    || fail "placement verifier does not pin the installed project keyring"
gpg --batch --no-options --with-colons --show-keys "$PROJECT_RELEASE_KEY" 2>/dev/null \
    | grep -Fq "fpr:::::::::$PROJECT_RELEASE_FINGERPRINT:" \
    || fail "shipped project key does not match the pinned release signer"

# Fedora 44 splits `gpg` and `gpgv` across two packages. Both the Workstation
# base RPM and headless Server RPM can admit Android compute, so neither package
# may ship this verifier without both hard runtime dependencies.
python3 - "$PACKAGE_MANIFEST" <<'PY'
import sys
import re
from pathlib import Path

required_sections = {
    "package.metadata.generate-rpm.requires": "base",
    "package.metadata.generate-rpm.variants.server.requires": "server",
}
observed = {section: set() for section in required_sections}
section = None
for raw_line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    line = raw_line.strip()
    if line.startswith("[") and line.endswith("]"):
        section = line[1:-1]
        continue
    if section not in observed or line.startswith("#"):
        continue
    match = re.fullmatch(r'(gnupg2(?:-verify)?)\s*=\s*"\*"', line)
    if match:
        observed[section].add(match.group(1))

for section, package_name in required_sections.items():
    missing = [
        name
        for name in ("gnupg2", "gnupg2-verify")
        if name not in observed[section]
    ]
    if missing:
        raise SystemExit(
            f"verify-android-contract: {package_name} RPM lacks hard Requires: {missing}"
        )
PY

"$MANIFEST_VERIFY" --self-test >/dev/null
"$TOOL_READINESS" --self-test >/dev/null
"$PLACEMENT_READINESS" --self-test >/dev/null

# Exercise the exact armored-key -> binary-keyring -> gpgv path with an
# ephemeral Ed25519 release signer. gpgv cannot consume the shipped ASCII armor
# directly; production admission performs this same dearmor step in a private
# temporary directory before signature verification.
crypto_fixture=$(mktemp -d)
trap 'rm -rf -- "$crypto_fixture"' EXIT
export GNUPGHOME="$crypto_fixture/gnupg"
mkdir -m 700 "$GNUPGHOME"
printf 'signed Android release artifact fixture\n' >"$crypto_fixture/artifact"
gpg --batch --pinentry-mode loopback --passphrase '' \
    --quick-generate-key 'MCNF Android contract fixture <fixture@example.invalid>' \
    ed25519 sign 1d >/dev/null 2>&1
gpg --batch --armor --detach-sign "$crypto_fixture/artifact"
gpg --batch --armor --export >"$crypto_fixture/trusted.asc"
gpg --batch --no-options --dearmor \
    --output "$crypto_fixture/trusted.gpg" "$crypto_fixture/trusted.asc"
gpgv --keyring "$crypto_fixture/trusted.gpg" \
    "$crypto_fixture/artifact.asc" "$crypto_fixture/artifact" >/dev/null 2>&1 \
    || fail "armored project-key admission path failed its real gpgv fixture"

echo "Android/Cuttlefish packaging contract checks passed"
