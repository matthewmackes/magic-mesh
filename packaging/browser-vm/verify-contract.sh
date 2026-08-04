#!/usr/bin/env bash
# Focused executable contract tests for the Browser VM package boundary.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BROWSER_VM="$ROOT/packaging/browser-vm"
PROFILE="$BROWSER_VM/profile.env"
PROFILE_VERIFY="$BROWSER_VM/verify-profile.sh"
VALIDATOR="$BROWSER_VM/validate-runtime-inputs.sh"
ACTIVATION_VERIFY="$BROWSER_VM/verify-activation-contract.sh"
ATTACH_VERIFY="$BROWSER_VM/verify-transport-attach.sh"
RUNTIME_EVIDENCE_VERIFY="$ROOT/install-helpers/verify-browser-vm-runtime-evidence.py"
IMAGE_BUILD="$BROWSER_VM/build-image.sh"
IMAGE_VERIFY="$BROWSER_VM/verify-image.sh"
PRODUCTION_CONTROL_VERIFY="$BROWSER_VM/verify-production-control-image.py"
RUNTIME="$BROWSER_VM/mcnf-browser-vm-runtime.sh"
RUNTIME_UNIT="$BROWSER_VM/mcnf-browser-vm-runtime.service"
XRDP_STARTWM="$BROWSER_VM/mcnf-browser-vm-xrdp-startwm.sh"
SESSION="$BROWSER_VM/mcnf-browser-vm-session.sh"
SESSION_INPUT_VERIFY="$BROWSER_VM/verify-session-input-contract.sh"
MEDIA_PROBE="$BROWSER_VM/mcnf-browser-vm-media-probe.sh"
MEDIA_EVIDENCE_VERIFY="$ROOT/install-helpers/verify-browser-vm-media-evidence.py"
PERFORMANCE_EVIDENCE_VERIFY="$ROOT/install-helpers/verify-browser-vm-performance.py"
LIVE_ACCEPTANCE_VERIFY="$ROOT/install-helpers/verify-browser-vm-live-acceptance.py"
VDI_LIVE_PROOF_VERIFY="$ROOT/install-helpers/verify-vdi-live-proof.py"
DEPLOYMENT_VERIFY="$ROOT/install-helpers/verify-browser-vm-deployment.py"
EPHEMERAL_NOCLOUD="$BROWSER_VM/prepare-ephemeral-nocloud.sh"
DEPLOY_IMAGE="$BROWSER_VM/deploy-image.sh"
PRODUCTION_CONTROL_UNIT="$ROOT/install-helpers/browser-vm-production-control/deploy/browser-vm-guest-audio-probe-controller.service"
PRODUCTION_CONTROL_CONFIG="$ROOT/install-helpers/browser-vm-production-control/deploy/controller-config.example.json"
PRODUCTION_CONTROL_POLICY="$BROWSER_VM/mcnf-browser-vm-managed-policy.json"

fail() {
    echo "verify-browser-vm-contract: $*" >&2
    exit 1
}

[ -x "$PROFILE_VERIFY" ] || fail "profile verifier is not executable"
[ -x "$VALIDATOR" ] || fail "runtime validator is not executable"
[ -x "$ACTIVATION_VERIFY" ] || fail "activation verifier is not executable"
[ -x "$ATTACH_VERIFY" ] || fail "transport attach verifier is not executable"
[ -x "$RUNTIME_EVIDENCE_VERIFY" ] || fail "runtime evidence verifier is not executable"
[ -x "$IMAGE_BUILD" ] || fail "image builder is not executable"
[ -x "$IMAGE_VERIFY" ] || fail "image verifier is not executable"
[ -x "$PRODUCTION_CONTROL_VERIFY" ] || fail "production-control image verifier is not executable"
[ -x "$RUNTIME" ] || fail "guest runtime is not executable"
[ -x "$XRDP_STARTWM" ] || fail "xrdp session entrypoint is not executable"
[ -x "$SESSION" ] || fail "media session supervisor is not executable"
[ -x "$SESSION_INPUT_VERIFY" ] || fail "session/input contract verifier is not executable"
[ -x "$MEDIA_PROBE" ] || fail "guest media probe is not executable"
[ -x "$MEDIA_EVIDENCE_VERIFY" ] || fail "media evidence verifier is not executable"
[ -x "$PERFORMANCE_EVIDENCE_VERIFY" ] || fail "performance evidence verifier is not executable"
[ -x "$LIVE_ACCEPTANCE_VERIFY" ] || fail "live acceptance verifier is not executable"
[ -x "$VDI_LIVE_PROOF_VERIFY" ] || fail "VDI live proof verifier is not executable"
[ -x "$DEPLOYMENT_VERIFY" ] || fail "Browser VM deployment verifier is not executable"
[ -x "$EPHEMERAL_NOCLOUD" ] || fail "ephemeral NoCloud helper is not executable"
[ -x "$DEPLOY_IMAGE" ] || fail "image deploy helper is not executable"
[ -f "$RUNTIME_UNIT" ] || fail "guest runtime unit is missing"
[ -f "$PRODUCTION_CONTROL_UNIT" ] || fail "production-control guest service unit is missing"
[ -f "$PRODUCTION_CONTROL_CONFIG" ] || fail "production-control guest config is missing"
[ -f "$PRODUCTION_CONTROL_POLICY" ] || fail "production-control Chromium policy is missing"
bash -n "$PROFILE_VERIFY" "$VALIDATOR" "$ACTIVATION_VERIFY" "$ATTACH_VERIFY" "$IMAGE_BUILD" "$IMAGE_VERIFY" "$SESSION_INPUT_VERIFY" "$EPHEMERAL_NOCLOUD" "$DEPLOY_IMAGE" "$0"
sh -n "$RUNTIME" "$XRDP_STARTWM" "$SESSION" "$MEDIA_PROBE"
python3 -m py_compile "$RUNTIME_EVIDENCE_VERIFY" "$MEDIA_EVIDENCE_VERIFY" "$PERFORMANCE_EVIDENCE_VERIFY" "$LIVE_ACCEPTANCE_VERIFY" "$VDI_LIVE_PROOF_VERIFY" "$DEPLOYMENT_VERIFY" "$PRODUCTION_CONTROL_VERIFY"
grep -Fq 'runtime-evidence.json' "$RUNTIME" || fail "guest runtime does not emit bounded evidence"
grep -Fq 'audio_status=wired' "$RUNTIME" || fail "guest runtime omits typed audio wiring status"
grep -Fq 'gpu_status=passed' "$RUNTIME" || fail "guest runtime omits VA-API status"
grep -Fq 'mcnf-browser-vm-media-probe' "$RUNTIME" || fail "guest runtime omits Chromium media probe"
for runtime_path in "$VALIDATOR" "$RUNTIME" "$MEDIA_PROBE"; do
    grep -Fq '/etc/mcnf-browser-vm' "$runtime_path" \
        || fail "Browser runtime component omits the dedicated readable input root: $runtime_path"
    if grep -Fq '/etc/mackesd/browser-vm' "$runtime_path"; then
        fail "Browser runtime component uses the protected daemon configuration root: $runtime_path"
    fi
done
"$SESSION_INPUT_VERIFY" --source "$BROWSER_VM" >/dev/null
grep -Fq "use_fastpath=input" "$BROWSER_VM/Containerfile" \
    || fail "Browser image does not keep graphics on slow-path bitmap updates"
grep -Fq 'xrdp-selinux' "$BROWSER_VM/Containerfile" \
    || fail "Browser image omits the Fedora xrdp SELinux policy package"
for image_path in \
    '/usr/libexec/mcnf/browser-vm-guest-audio-probe-controller' \
    '/usr/lib/systemd/system/browser-vm-guest-audio-probe-controller.service' \
    '/etc/mcnf/browser-vm-guest-audio-probe-controller.json' \
    '/etc/chromium/policies/managed/mcnf-browser-vm.json'; do
    grep -Fq "$image_path" "$BROWSER_VM/Containerfile" \
        || fail "Browser image omits production-control path: $image_path"
done
grep -Fq 'mcnf-browser-probe' "$BROWSER_VM/Containerfile" \
    || fail "Browser image omits the dedicated production-control account"
grep -Eq 'systemctl[[:space:]]+enable[[:space:]]+browser-vm-guest-audio-probe-controller\.service' "$BROWSER_VM/Containerfile" \
    || fail "Browser image does not enable the production-control guest service"
for directive in \
    'User=mcnf-browser-probe' \
    'Group=mcnf-browser-probe' \
    'ExecStart=/usr/libexec/mcnf/browser-vm-guest-audio-probe-controller' \
    'IPAddressDeny=any' \
    'IPAddressAllow=192.168.122.1/32'; do
    grep -Fxq "$directive" "$PRODUCTION_CONTROL_UNIT" \
        || fail "production-control guest service omits: $directive"
done
if ! grep -Fxq 'IPAddressAllow=localhost' "$PRODUCTION_CONTROL_UNIT"; then
    if ! grep -Fxq 'IPAddressAllow=127.0.0.0/8' "$PRODUCTION_CONTROL_UNIT" \
        || ! grep -Fxq 'IPAddressAllow=::1/128' "$PRODUCTION_CONTROL_UNIT"; then
        fail "production-control guest service does not admit localhost only"
    fi
fi
"$PRODUCTION_CONTROL_VERIFY" --source-assets \
    --service-unit "$PRODUCTION_CONTROL_UNIT" \
    --controller-config "$PRODUCTION_CONTROL_CONFIG" \
    --chromium-policy "$PRODUCTION_CONTROL_POLICY" >/dev/null
if grep -Eq '^[[:space:]]*(ADD|COPY)[[:space:]].*controller-secret' "$BROWSER_VM/Containerfile"; then
    fail "Browser image attempts to embed the production-control shared secret"
fi
grep -Fq 'BROWSER_VM_DISK_GB' "$IMAGE_BUILD" || fail "image builder does not bind disk size to the profile"
grep -Fq 'qemu-img resize' "$IMAGE_BUILD" || fail "image builder does not resize the disk output"
grep -Fq '64 GiB' "$DEPLOY_IMAGE" || fail "deployment helper does not enforce the 64-GiB image floor"
"$IMAGE_VERIFY" --self-test >/dev/null
"$PRODUCTION_CONTROL_VERIFY" --self-test >/dev/null
"$RUNTIME_EVIDENCE_VERIFY" --self-test >/dev/null
"$MEDIA_EVIDENCE_VERIFY" --self-test >/dev/null
"$PERFORMANCE_EVIDENCE_VERIFY" --self-test >/dev/null
"$VDI_LIVE_PROOF_VERIFY" --self-test >/dev/null
"$DEPLOYMENT_VERIFY" --self-test >/dev/null
"$LIVE_ACCEPTANCE_VERIFY" --self-test >/dev/null
"$EPHEMERAL_NOCLOUD" --self-test >/dev/null
"$DEPLOY_IMAGE" --self-test >/dev/null
"$PROFILE_VERIFY" --source "$PROFILE" >/dev/null
"$ACTIVATION_VERIFY" >/dev/null
"$ATTACH_VERIFY" >/dev/null

profile_fixture=$(mktemp)
trap 'rm -rf "$fixture" "$profile_fixture"' EXIT
sed 's/^BROWSER_VM_SOURCE_COMMIT=.*/BROWSER_VM_SOURCE_COMMIT=not-a-revision/' \
    "$PROFILE" > "$profile_fixture"
if "$PROFILE_VERIFY" --source "$profile_fixture" >/dev/null 2>&1; then
    fail "accepted a non-immutable source commit"
fi

sed 's/^BROWSER_VM_SOURCE_COMMIT=.*/BROWSER_VM_SOURCE_COMMIT=0000000000000000000000000000000000000000/' \
    "$PROFILE" > "$profile_fixture"
if "$PROFILE_VERIFY" --source "$profile_fixture" >/dev/null 2>&1; then
    fail "accepted the null Git revision as source provenance"
fi

ln -s "$PROFILE" "$profile_fixture.symlink"
if "$PROFILE_VERIFY" --source "$profile_fixture.symlink" >/dev/null 2>&1; then
    fail "accepted a symlinked profile input"
fi
rm "$profile_fixture.symlink"

for field in BROWSER_VM_RUNTIME_FAILURE_POLICY BROWSER_VM_GUEST_TERMINAL_STATES; do
    cp "$PROFILE" "$profile_fixture"
    case "$field" in
        BROWSER_VM_RUNTIME_FAILURE_POLICY)
            sed -i 's/^BROWSER_VM_RUNTIME_FAILURE_POLICY=.*/BROWSER_VM_RUNTIME_FAILURE_POLICY=best-effort/' "$profile_fixture"
            ;;
        BROWSER_VM_GUEST_TERMINAL_STATES)
            sed -i 's/^BROWSER_VM_GUEST_TERMINAL_STATES=.*/BROWSER_VM_GUEST_TERMINAL_STATES=failed,retry/' "$profile_fixture"
            ;;
    esac
    if "$PROFILE_VERIFY" --source "$profile_fixture" >/dev/null 2>&1; then
        fail "accepted an unsafe $field policy"
    fi
done

fixture=$(mktemp -d)
mkdir -p "$fixture/browser-vm"
input="$fixture/browser-vm"
digest=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef

write_valid() {
    printf '%s\n' browser-vm-chromium > "$input/profile-id"
    printf '%s\n' browser-vm-chromium > "$input/image-id"
    printf '%s\n' "$digest" > "$input/image-digest"
    printf '%s\n' session:00000000-0000-4000-8000-000000000001 > "$input/session-id"
    printf '%s\n' spice > "$input/transport"
    printf '%s\n' connected > "$input/transport-health"
}

run_validator() {
    MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" --test "$@"
}

write_valid
run_validator >/dev/null

chmod 666 "$input/session-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted a runtime identity file writable by group or other"
fi
chmod 600 "$input/session-id"

chmod 755 "$input/session-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted an executable runtime identity file"
fi
chmod 600 "$input/session-id"

chmod 777 "$input"
if run_validator >/dev/null 2>&1; then
    fail "accepted a runtime input directory writable by group or other"
fi
chmod 700 "$input"

printf '%s\n' 'sha256:not-a-digest' > "$input/image-digest"
if run_validator >/dev/null 2>&1; then
    fail "accepted a non-immutable image digest"
fi
printf '%s\n' "$digest" > "$input/image-digest"

printf '%s\n' 'flatpak run --command=sh' > "$input/session-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted a command-shaped session identity"
fi
printf '%s\n' session:00000000-0000-4000-8000-000000000001 > "$input/session-id"

printf '%s\n' 'https://attacker.invalid' > "$input/transport"
if run_validator >/dev/null 2>&1; then
    fail "accepted a URL-shaped transport"
fi
printf '%s\n' sunshine > "$input/transport"
if run_validator >/dev/null 2>&1; then
    fail "accepted the unimplemented Sunshine transport"
fi
printf '%s\n' rdp > "$input/transport"
run_validator >/dev/null

for health in connected reconnecting failed unavailable; do
    printf '%s\n' "$health" > "$input/transport-health"
    run_validator >/dev/null
done

for invalid_health in connecting unknown 'failed; reboot' 'https://attacker.invalid' '/tmp/health'; do
    printf '%s\n' "$invalid_health" > "$input/transport-health"
    if run_validator >/dev/null 2>&1; then
        fail "accepted invalid transport-health state: $invalid_health"
    fi
done
printf '%s\n' connected > "$input/transport-health"

rm "$input/transport-health"
if run_validator >/dev/null 2>&1; then
    fail "accepted missing transport-health evidence"
fi
printf '%s\n' connected > "$input/transport-health"

printf '%s\n' 'connected extra' > "$input/transport-health"
if run_validator >/dev/null 2>&1; then
    fail "accepted multi-token transport-health evidence"
fi
printf '%s\n' connected > "$input/transport-health"

printf '%s\n' 'exec /tmp/host-command' > "$input/command"
if run_validator >/dev/null 2>&1; then
    fail "accepted an extra command input"
fi
rm "$input/command"

for fallback in browser-engine host-browser fallback-url guest-state; do
    printf '%s\n' failed > "$input/$fallback"
    if run_validator >/dev/null 2>&1; then
        fail "accepted host fallback or lifecycle input: $fallback"
    fi
    rm "$input/$fallback"
done

ln -s "$input/missing" "$input/extra-link"
if run_validator >/dev/null 2>&1; then
    fail "accepted a dangling symlink in the runtime input directory"
fi
rm "$input/extra-link"

long_id=$(printf 'x%.0s' {1..129})
printf '%s\n' "$long_id" > "$input/session-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted an overlong session identity"
fi

printf '%s\n' session:00000000-0000-4000-8000-000000000001 > "$input/session-id"
printf '%s\n' browser-vm-chromium > "$fixture/image-id-source"
rm "$input/image-id"
ln -s "$fixture/image-id-source" "$input/image-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted a symlinked identity input"
fi
rm "$input/image-id"
printf '%s\n' browser-vm-chromium > "$input/image-id"

rm "$input/session-id"
if run_validator >/dev/null 2>&1; then
    fail "accepted a missing session identity"
fi

echo "Browser VM contract checks passed"
