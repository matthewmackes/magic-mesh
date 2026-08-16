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
GUEST_PAYLOAD_VERIFY="$ANDROID/verify-guest-payload.sh"
IMAGE_RECEIPT="$ANDROID/produce-image-receipt.py"
IMAGE_RECEIPT_TEST="$ANDROID/test-produce-image-receipt.py"
DECLARATION_PRODUCER="$ANDROID/produce-guest-payload-declaration.sh"
PACKAGE_MANIFEST="$ROOT/crates/mesh/mackesd/Cargo.toml"
PROJECT_RELEASE_KEY="$ROOT/packaging/repo/RPM-GPG-KEY-magic-mesh"
PROJECT_RELEASE_FINGERPRINT="06B1C27EA0E08A225155EB3314018AA1497DDC7C"
CUTTLEFISH_TASKS="$ROOT/automation/ansible/roles/cuttlefish_host/tasks/main.yml"
CUTTLEFISH_DEFAULTS="$ROOT/automation/ansible/roles/cuttlefish_host/defaults/main.yml"

fail() {
    echo "verify-android-contract: $*" >&2
    exit 1
}

verify_package_manifest() {
    python3 - "$1" <<'PY'
import os
import re
import stat
import sys

path = sys.argv[1]
flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
try:
    fd = os.open(path, flags)
except OSError as exc:
    raise SystemExit(
        f"verify-android-contract: cannot open package manifest safely: {exc}"
    ) from exc

try:
    opened = os.fstat(fd)
    named = os.stat(path, follow_symlinks=False)
    if not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1:
        raise SystemExit(
            "verify-android-contract: package manifest must be a single-link regular file"
        )
    if (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino):
        raise SystemExit(
            "verify-android-contract: package manifest changed before validation"
        )
    payload = os.read(fd, 262145)
    if len(payload) > 262144:
        raise SystemExit("verify-android-contract: package manifest exceeds 256 KiB")
    if os.read(fd, 1):
        raise SystemExit("verify-android-contract: package manifest grew during validation")
    closed = os.stat(path, follow_symlinks=False)
    if (opened.st_dev, opened.st_ino) != (closed.st_dev, closed.st_ino):
        raise SystemExit(
            "verify-android-contract: package manifest changed during validation"
        )
finally:
    os.close(fd)

try:
    manifest = payload.decode("utf-8")
except UnicodeDecodeError as exc:
    raise SystemExit(
        "verify-android-contract: package manifest is not valid UTF-8"
    ) from exc

required_sections = {
    "package.metadata.generate-rpm.requires": "base",
    "package.metadata.generate-rpm.variants.server.requires": "server",
}
observed = {section: set() for section in required_sections}
section = None
for raw_line in manifest.splitlines():
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
}

if [[ ${1:-} == "--verify-package-manifest" ]]; then
    [[ $# -eq 2 ]] || fail "--verify-package-manifest requires one path"
    verify_package_manifest "$2"
    exit 0
fi

if [[ ${1:-} == "--self-test" ]]; then
    [[ $# -eq 1 ]] || fail "--self-test takes no arguments"
    fixture=$(mktemp -d)
    trap 'rm -rf -- "$fixture"' EXIT
    printf '%s\n' \
        '[package.metadata.generate-rpm.requires]' \
        'gnupg2 = "*"' \
        'gnupg2-verify = "*"' \
        '[package.metadata.generate-rpm.variants.server.requires]' \
        'gnupg2 = "*"' \
        'gnupg2-verify = "*"' >"$fixture/Cargo.toml"
    ln "$fixture/Cargo.toml" "$fixture/substituted-Cargo.toml"
    if "$0" --verify-package-manifest "$fixture/substituted-Cargo.toml" \
        >/dev/null 2>&1; then
        fail "hard-linked package manifest gained Android packaging authority"
    fi
    echo "Android/Cuttlefish packaging contract self-test passed"
    exit 0
fi

[[ $# -eq 0 ]] || fail "unexpected arguments: $*"

[ -x "$MANIFEST_VERIFY" ] || fail "Android manifest verifier is not executable"
[ -x "$TOOL_READINESS" ] || fail "guest-tool readiness recorder is not executable"
[ -x "$PLACEMENT_READINESS" ] || fail "Cuttlefish placement verifier is not executable"
[ -x "$GUEST_PAYLOAD_VERIFY" ] || fail "signed guest-payload verifier is not executable"
[ -x "$IMAGE_RECEIPT" ] || fail "Cuttlefish image-receipt producer is not executable"
[ -x "$IMAGE_RECEIPT_TEST" ] || fail "Cuttlefish image-receipt hostile test is not executable"
[ -x "$DECLARATION_PRODUCER" ] || fail "signed declaration producer is not executable"
[ -r "$CUTTLEFISH_TASKS" ] || fail "production Cuttlefish assembly tasks are missing"
[ -r "$CUTTLEFISH_DEFAULTS" ] || fail "production Cuttlefish assembly defaults are missing"

bash -n "$MANIFEST_VERIFY" "$TOOL_READINESS" "$GUEST_PAYLOAD_VERIFY" \
    "$DECLARATION_PRODUCER" "$0"
python3 -m py_compile "$PLACEMENT_READINESS" "$IMAGE_RECEIPT" "$IMAGE_RECEIPT_TEST"

grep -Fq 'produce-image-receipt.py' "$DECLARATION_PRODUCER" \
    || fail "signed declaration producer bypasses Cuttlefish image receipt admission"
grep -Fq '"schema_version": 3' "$DECLARATION_PRODUCER" \
    || fail "signed declaration producer does not emit governed image schema v3"
grep -Fq 'document["schema_version"] != 3' "$GUEST_PAYLOAD_VERIFY" \
    || fail "guest payload verifier does not require governed image schema v3"

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
verify_package_manifest "$PACKAGE_MANIFEST"

# The production image assembly must consume only verifier-staged bytes. Keep
# this seam in the package contract so a future role edit cannot quietly restore
# direct apt/runtime reads from mutable controller-provided paths.
grep -Fq 'verify-guest-payload.sh' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish assembly bypasses signed payload admission"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/packages/{{ item | basename }}' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish apt path is not bound to the authenticated stage"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/readiness-relay' "$CUTTLEFISH_TASKS" \
    || fail "readiness relay is not installed from the authenticated stage"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/vdi-agent' "$CUTTLEFISH_TASKS" \
    || fail "VDI agent is not installed from the authenticated stage"
for variable in cuttlefish_release_declaration cuttlefish_release_signature \
    cuttlefish_readiness_relay cuttlefish_vdi_agent cuttlefish_mesh_host \
    cuttlefish_webrtc_port cuttlefish_session_id cuttlefish_guest_environment_path; do
    grep -Fq "$variable" "$CUTTLEFISH_DEFAULTS" \
        || fail "production Cuttlefish assembly lacks signed handoff input: $variable"
done
grep -Fq 'cuttlefish_readiness_relay_install_path: /usr/libexec/mcnf-cuttlefish-readiness-relay' \
    "$CUTTLEFISH_DEFAULTS" \
    || fail "readiness relay install path differs from the packaged systemd unit"
grep -Fq 'cuttlefish_vdi_agent_install_path: /usr/libexec/mcnf-cuttlefish-vdi-agent' \
    "$CUTTLEFISH_DEFAULTS" \
    || fail "VDI agent install path differs from the packaged systemd unit"
grep -Fq 'MCNF_SESSION_ID={{ cuttlefish_session_id }}' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish assembly does not bind the relay to a session"
grep -Fq 'name: mcnf-cuttlefish-readiness-relay.service' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish assembly does not activate the readiness relay"
admission_line=$(grep -n -m1 'Authenticate and stage the exact signed Cuttlefish guest payload' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
apt_line=$(grep -n -m1 'Install the android-cuttlefish host .deb packages' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
cvd_line=$(grep -n -m1 'Start the Cuttlefish device with a VNC server' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
relay_line=$(grep -n -m1 'Enable the authenticated readiness relay after Cuttlefish is available' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
[[ -n $admission_line && -n $apt_line && -n $cvd_line && -n $relay_line \
    && $admission_line -lt $apt_line && $admission_line -lt $cvd_line \
    && $cvd_line -lt $relay_line ]] \
    || fail "signed payload admission does not precede package/backend effects"

"$MANIFEST_VERIFY" --self-test >/dev/null
"$TOOL_READINESS" --self-test >/dev/null
"$PLACEMENT_READINESS" --self-test >/dev/null
"$GUEST_PAYLOAD_VERIFY" --self-test >/dev/null
python3 "$IMAGE_RECEIPT_TEST" >/dev/null

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
