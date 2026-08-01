#!/usr/bin/env bash
# Focused executable contract tests for the Browser VM package boundary.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BROWSER_VM="$ROOT/packaging/browser-vm"
PROFILE="$BROWSER_VM/profile.env"
PROFILE_VERIFY="$BROWSER_VM/verify-profile.sh"
VALIDATOR="$BROWSER_VM/validate-runtime-inputs.sh"
ACTIVATION_VERIFY="$BROWSER_VM/verify-activation-contract.sh"

fail() {
    echo "verify-browser-vm-contract: $*" >&2
    exit 1
}

[ -x "$PROFILE_VERIFY" ] || fail "profile verifier is not executable"
[ -x "$VALIDATOR" ] || fail "runtime validator is not executable"
[ -x "$ACTIVATION_VERIFY" ] || fail "activation verifier is not executable"
bash -n "$PROFILE_VERIFY" "$VALIDATOR" "$ACTIVATION_VERIFY" "$0"
"$PROFILE_VERIFY" "$PROFILE" >/dev/null
"$ACTIVATION_VERIFY" >/dev/null

profile_fixture=$(mktemp)
trap 'rm -rf "$fixture" "$profile_fixture"' EXIT
sed 's/^BROWSER_VM_SOURCE_COMMIT=.*/BROWSER_VM_SOURCE_COMMIT=not-a-revision/' \
    "$PROFILE" > "$profile_fixture"
if "$PROFILE_VERIFY" "$profile_fixture" >/dev/null 2>&1; then
    fail "accepted a non-immutable source commit"
fi

sed 's/^BROWSER_VM_SOURCE_COMMIT=.*/BROWSER_VM_SOURCE_COMMIT=0000000000000000000000000000000000000000/' \
    "$PROFILE" > "$profile_fixture"
if "$PROFILE_VERIFY" "$profile_fixture" >/dev/null 2>&1; then
    fail "accepted the null Git revision as source provenance"
fi

ln -s "$PROFILE" "$profile_fixture.symlink"
if "$PROFILE_VERIFY" "$profile_fixture.symlink" >/dev/null 2>&1; then
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
    if "$PROFILE_VERIFY" "$profile_fixture" >/dev/null 2>&1; then
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
    printf '%s\n' sunshine > "$input/transport"
    printf '%s\n' connected > "$input/transport-health"
}

write_valid
MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null

chmod 666 "$input/session-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a runtime identity file writable by group or other"
fi
chmod 600 "$input/session-id"

chmod 755 "$input/session-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted an executable runtime identity file"
fi
chmod 600 "$input/session-id"

chmod 777 "$input"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a runtime input directory writable by group or other"
fi
chmod 700 "$input"

printf '%s\n' 'sha256:not-a-digest' > "$input/image-digest"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a non-immutable image digest"
fi
printf '%s\n' "$digest" > "$input/image-digest"

printf '%s\n' 'flatpak run --command=sh' > "$input/session-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a command-shaped session identity"
fi
printf '%s\n' session:00000000-0000-4000-8000-000000000001 > "$input/session-id"

printf '%s\n' 'https://attacker.invalid' > "$input/transport"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a URL-shaped transport"
fi
printf '%s\n' sunshine > "$input/transport"

for health in connected reconnecting failed unavailable; do
    printf '%s\n' "$health" > "$input/transport-health"
    MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null
done

for invalid_health in connecting unknown 'failed; reboot' 'https://attacker.invalid' '/tmp/health'; do
    printf '%s\n' "$invalid_health" > "$input/transport-health"
    if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
        fail "accepted invalid transport-health state: $invalid_health"
    fi
done
printf '%s\n' connected > "$input/transport-health"

rm "$input/transport-health"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted missing transport-health evidence"
fi
printf '%s\n' connected > "$input/transport-health"

printf '%s\n' 'connected extra' > "$input/transport-health"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted multi-token transport-health evidence"
fi
printf '%s\n' connected > "$input/transport-health"

printf '%s\n' 'exec /tmp/host-command' > "$input/command"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted an extra command input"
fi
rm "$input/command"

for fallback in browser-engine host-browser fallback-url guest-state; do
    printf '%s\n' failed > "$input/$fallback"
    if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
        fail "accepted host fallback or lifecycle input: $fallback"
    fi
    rm "$input/$fallback"
done

ln -s "$input/missing" "$input/extra-link"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a dangling symlink in the runtime input directory"
fi
rm "$input/extra-link"

long_id=$(printf 'x%.0s' {1..129})
printf '%s\n' "$long_id" > "$input/session-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted an overlong session identity"
fi

printf '%s\n' session:00000000-0000-4000-8000-000000000001 > "$input/session-id"
printf '%s\n' browser-vm-chromium > "$fixture/image-id-source"
rm "$input/image-id"
ln -s "$fixture/image-id-source" "$input/image-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a symlinked identity input"
fi
rm "$input/image-id"
printf '%s\n' browser-vm-chromium > "$input/image-id"

rm "$input/session-id"
if MCNF_BROWSER_VM_INPUT_ROOT="$input" "$VALIDATOR" >/dev/null 2>&1; then
    fail "accepted a missing session identity"
fi

echo "Browser VM contract checks passed"
