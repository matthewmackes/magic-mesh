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
CUTTLEFISH_TASKS="$ROOT/automation/ansible/roles/cuttlefish_host/tasks/main.yml"
CUTTLEFISH_DEFAULTS="$ROOT/automation/ansible/roles/cuttlefish_host/defaults/main.yml"

fail() {
    echo "verify-android-contract: $*" >&2
    exit 1
}

if [[ ${1:-} == "--self-test" ]]; then
    [[ $# -eq 1 ]] || fail "--self-test takes no arguments"
    ! grep -Eq '(signature_path|gpgv|trusted release signer)' "$PLACEMENT_READINESS" \
        || fail "placement readiness still treats signatures as admission authority"
    echo "Android/Cuttlefish packaging contract self-test passed"
    exit 0
fi

[[ $# -eq 0 ]] || fail "unexpected arguments: $*"

[ -x "$MANIFEST_VERIFY" ] || fail "Android manifest verifier is not executable"
[ -x "$TOOL_READINESS" ] || fail "guest-tool readiness recorder is not executable"
[ -x "$PLACEMENT_READINESS" ] || fail "Cuttlefish placement verifier is not executable"
[ -x "$GUEST_PAYLOAD_VERIFY" ] || fail "guest-payload verifier is not executable"
[ -x "$IMAGE_RECEIPT" ] || fail "Cuttlefish image-receipt producer is not executable"
[ -x "$IMAGE_RECEIPT_TEST" ] || fail "Cuttlefish image-receipt hostile test is not executable"
[ -x "$DECLARATION_PRODUCER" ] || fail "content declaration producer is not executable"
[ -r "$CUTTLEFISH_TASKS" ] || fail "production Cuttlefish assembly tasks are missing"
[ -r "$CUTTLEFISH_DEFAULTS" ] || fail "production Cuttlefish assembly defaults are missing"

bash -n "$MANIFEST_VERIFY" "$TOOL_READINESS" "$GUEST_PAYLOAD_VERIFY" \
    "$DECLARATION_PRODUCER" "$0"
python3 -m py_compile "$PLACEMENT_READINESS" "$IMAGE_RECEIPT" "$IMAGE_RECEIPT_TEST"

grep -Fq 'produce-image-receipt.py' "$DECLARATION_PRODUCER" \
    || fail "content declaration producer bypasses Cuttlefish image receipt validation"
grep -Fq '"schema_version": 3' "$DECLARATION_PRODUCER" \
    || fail "content declaration producer does not emit governed image schema v3"
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
! grep -Eq '(signature_path|gpgv|trusted release signer)' "$PLACEMENT_READINESS" \
    || fail "placement verifier still treats signatures as workload authority"

# The production image assembly must consume only verifier-staged bytes. Keep
# this seam in the package contract so a future role edit cannot quietly restore
# direct apt/runtime reads from mutable controller-provided paths.
grep -Fq 'verify-guest-payload.sh' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish assembly bypasses payload verification"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/packages/{{ item | basename }}' "$CUTTLEFISH_TASKS" \
    || fail "production Cuttlefish apt path is not bound to the verified stage"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/readiness-relay' "$CUTTLEFISH_TASKS" \
    || fail "readiness relay is not installed from the verified stage"
grep -Fq '{{ cuttlefish_payload_stage_dir }}/vdi-agent' "$CUTTLEFISH_TASKS" \
    || fail "VDI agent is not installed from the verified stage"
for variable in cuttlefish_release_declaration \
    cuttlefish_readiness_relay cuttlefish_vdi_agent cuttlefish_mesh_host \
    cuttlefish_webrtc_port cuttlefish_session_id cuttlefish_guest_environment_path; do
    grep -Fq "$variable" "$CUTTLEFISH_DEFAULTS" \
        || fail "production Cuttlefish assembly lacks content handoff input: $variable"
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
admission_line=$(grep -n -m1 'Verify and stage the exact content-declared Cuttlefish guest payload' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
apt_line=$(grep -n -m1 'Install the android-cuttlefish host .deb packages' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
cvd_line=$(grep -n -m1 'Start the Cuttlefish device with a VNC server' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
relay_line=$(grep -n -m1 'Enable the readiness relay after Cuttlefish is available' \
    "$CUTTLEFISH_TASKS" | cut -d: -f1)
[[ -n $admission_line && -n $apt_line && -n $cvd_line && -n $relay_line \
    && $admission_line -lt $apt_line && $admission_line -lt $cvd_line \
    && $cvd_line -lt $relay_line ]] \
    || fail "payload verification does not precede package/backend effects"

"$MANIFEST_VERIFY" --self-test >/dev/null
"$TOOL_READINESS" --self-test >/dev/null
"$PLACEMENT_READINESS" --self-test >/dev/null
"$GUEST_PAYLOAD_VERIFY" --self-test >/dev/null
python3 "$IMAGE_RECEIPT_TEST" >/dev/null

echo "Android/Cuttlefish packaging contract checks passed"
