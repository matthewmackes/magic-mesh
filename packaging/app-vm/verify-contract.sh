#!/usr/bin/env bash
# Static and executable checks for the App VM image/cloud-init contract.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
APP_VM="$ROOT/packaging/app-vm"
TEMPLATE="$ROOT/infra/tofu/cloud/cloud-init/mesh-join.yaml.tftpl"
VALIDATOR="$APP_VM/validate-runtime-inputs.sh"
LAUNCHER="$APP_VM/mcnf-app-vm-launch.sh"
IMAGE_VERIFY="$APP_VM/verify-image.sh"
BUILD="$APP_VM/build-image.sh"

require() {
    local needle=$1 file=$2
    grep -Fq -- "$needle" "$file" || {
        echo "FATAL: missing contract '$needle' in $file" >&2
        exit 1
    }
}

[ -x "$VALIDATOR" ] || { echo "FATAL: validator is not executable" >&2; exit 1; }
[ -x "$LAUNCHER" ] || { echo "FATAL: launcher is not executable" >&2; exit 1; }
bash -n "$BUILD" "$IMAGE_VERIFY" "$0"
sh -n "$VALIDATOR" "$LAUNCHER"
"$IMAGE_VERIFY" --self-test

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/app-vm"
printf '%s\n' '"org.example.App"' > "$fixture/app-vm/app-id"
printf '%s\n' '"catalog-2026.07"' > "$fixture/app-vm/catalog-revision"
printf '%s\n' '"wayland-standard"' > "$fixture/app-vm/guest-profile"
printf '%s\n' '"00000000-0000-4000-8000-000000000001"' > "$fixture/app-vm/session-id"
printf '%s\n' 'clipboard' > "$fixture/app-vm/capabilities"
printf '%s\n' 'app-vm-test' > "$fixture/hostname"
MCNF_APP_VM_INPUT_ROOT="$fixture/app-vm" \
MCNF_APP_VM_HOSTNAME_FILE="$fixture/hostname" "$VALIDATOR"

printf '%s\n' '"org.example.App;touch /tmp/pwned"' > "$fixture/app-vm/app-id"
if MCNF_APP_VM_INPUT_ROOT="$fixture/app-vm" \
   MCNF_APP_VM_HOSTNAME_FILE="$fixture/hostname" "$VALIDATOR" >/dev/null 2>&1; then
    echo "FATAL: validator accepted hostile app identity" >&2
    exit 1
fi

printf '%s\n' 'Editor' > "$fixture/app-vm/app-id"
if MCNF_APP_VM_INPUT_ROOT="$fixture/app-vm" \
   MCNF_APP_VM_HOSTNAME_FILE="$fixture/hostname" "$VALIDATOR" >/dev/null 2>&1; then
    echo "FATAL: validator accepted a non-reverse-DNS app identity" >&2
    exit 1
fi

printf '%s\n' 'org..example.Editor' > "$fixture/app-vm/app-id"
if MCNF_APP_VM_INPUT_ROOT="$fixture/app-vm" \
   MCNF_APP_VM_HOSTNAME_FILE="$fixture/hostname" "$VALIDATOR" >/dev/null 2>&1; then
    echo "FATAL: validator accepted an empty reverse-DNS component" >&2
    exit 1
fi

# Exercise the terminal guest-runtime evidence boundary with a bounded,
# dependency-free fixture.  This intentionally mirrors the existing typed
# AppVmRuntimeEvidence envelope: the profile is fixed by the image contract,
# the guest supplies only identities/generation/state/reason, and the broker
# rejects a replayed generation or terminal failure for readiness.  An
# unavailable observation is admitted as truthful evidence, but is not a
# connected/readiness claim.
runtime_evidence_assert() {
    local label=$1 payload=$2 expected=$3 previous_generation=$4
    local expected_session=$5 expected_vm=$6 expected_app=$7
    local generation state key_count keys session_id vm_id app_id

    [ "${#payload}" -le 2048 ] || {
        echo "FATAL: $label runtime evidence exceeds the bounded body limit" >&2
        return 1
    }
    for forbidden in command path mount environment socket host_fallback; do
        if printf '%s' "$payload" | grep -Fq "\"$forbidden\""; then
            echo "FATAL: $label runtime evidence contains forbidden $forbidden input" >&2
            return 1
        fi
    done

    # The fixture is deliberately flat and has exactly the six required keys
    # plus an optional reason.  This catches accidental acceptance of a host
    # execution or transport instruction without pretending shell is a JSON
    # parser for arbitrary input.
    keys=$(printf '%s' "$payload" | grep -oE '"[a-z_]+":' | sort -u | tr '\n' ' ')
    key_count=$(printf '%s' "$keys" | wc -w)
    if [ "$key_count" -lt 6 ] || [ "$key_count" -gt 7 ]; then
        echo "FATAL: $label runtime evidence has an unexpected field set" >&2
        return 1
    fi
    for key in app_id generation session_id state vm_id; do
        printf '%s' "$keys" | grep -Fq "\"$key\":" || {
            echo "FATAL: $label runtime evidence is missing $key" >&2
            return 1
        }
    done

    # A reconnect or terminal report remains an observation for the admitted
    # guest. It cannot replace the session, VM, or application identity while
    # retaining a usable generation.
    session_id=$(printf '%s' "$payload" | sed -n 's/.*"session_id":"\([^"]*\)".*/\1/p')
    vm_id=$(printf '%s' "$payload" | sed -n 's/.*"vm_id":"\([^"]*\)".*/\1/p')
    app_id=$(printf '%s' "$payload" | sed -n 's/.*"app_id":"\([^"]*\)".*/\1/p')
    if [ "$session_id" != "$expected_session" ] ||
       [ "$vm_id" != "$expected_vm" ] ||
       [ "$app_id" != "$expected_app" ]; then
        [ "$expected" = identity-rejected ] || {
            echo "FATAL: $label runtime evidence changed its admitted identity" >&2
            return 1
        }
        return 0
    fi
    [ "$expected" != identity-rejected ] || {
        echo "FATAL: $label runtime evidence identity drift was admitted" >&2
        return 1
    }

    generation=$(printf '%s' "$payload" | sed -n 's/.*"generation":\([0-9][0-9]*\).*/\1/p')
    case "$generation" in
        ''|*[!0-9]*)
            echo "FATAL: $label runtime evidence has a non-numeric generation" >&2
            return 1
            ;;
    esac
    [ "${#generation}" -le 18 ] || {
        echo "FATAL: $label runtime evidence generation is unbounded" >&2
        return 1
    }
    if [ "$generation" -le "$previous_generation" ] 2>/dev/null; then
        [ "$expected" = replay-rejected ] || {
            echo "FATAL: $label runtime evidence regressed generation" >&2
            return 1
        }
        return 0
    fi
    [ "$expected" != replay-rejected ] || {
        echo "FATAL: $label runtime evidence replay was admitted" >&2
        return 1
    }

    state=$(printf '%s' "$payload" | sed -n 's/.*"state":"\([a-z_]*\)".*/\1/p')
    case "$state:$expected" in
        connected:admitted|reconnecting:admitted|unavailable:admitted|failed:readiness-rejected)
            ;;
        *)
            echo "FATAL: $label runtime evidence state/admission mismatch" >&2
            return 1
            ;;
    esac
}

runtime_evidence_assert connected \
    '{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":41,"state":"connected","reason":"application process started"}' \
    admitted 0 session-1 app-vm-1 org.example.Editor
runtime_evidence_assert reconnecting \
    '{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":42,"state":"reconnecting","reason":"surface reconnect requested"}' \
    admitted 41 session-1 app-vm-1 org.example.Editor
runtime_evidence_assert failed \
    '{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":43,"state":"failed","reason":"application process exited"}' \
    readiness-rejected 42 session-1 app-vm-1 org.example.Editor
runtime_evidence_assert unavailable \
    '{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":44,"state":"unavailable","reason":"guest transport unavailable"}' \
    admitted 43 session-1 app-vm-1 org.example.Editor
runtime_evidence_assert replay \
    '{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":42,"state":"reconnecting","reason":"replayed evidence"}' \
    replay-rejected 44 session-1 app-vm-1 org.example.Editor

identity_drift_runtime_evidence='{"session_id":"session-2","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":45,"state":"reconnecting","reason":"replacement guest"}'
runtime_evidence_assert identity-drift "$identity_drift_runtime_evidence" \
    identity-rejected 44 session-1 app-vm-1 org.example.Editor

hostile_runtime_evidence='{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","generation":45,"state":"connected","command":"flatpak run","path":"/host"}'
if runtime_evidence_assert hostile "$hostile_runtime_evidence" admitted 44 \
   session-1 app-vm-1 org.example.Editor >/dev/null 2>&1; then
    echo "FATAL: runtime evidence accepted command/path host fallback input" >&2
    exit 1
else
    :
fi

if command -v shellcheck >/dev/null 2>&1; then
    shellcheck "$VALIDATOR" "$APP_VM/build-image.sh" "$0"
fi

require 'COPY packaging/app-vm/validate-runtime-inputs.sh /tmp/mcnf-app-vm-validate' "$APP_VM/Containerfile"
require 'ARG APP_VM_BASE=quay.io/fedora/fedora-bootc:44' "$APP_VM/Containerfile"
require 'COPY packaging/app-vm/mcnf-app-vm-launch.sh /tmp/mcnf-app-vm-launch' "$APP_VM/Containerfile"
require 'install -D -m 0755 /tmp/mcnf-app-vm-validate /usr/local/libexec/mcnf-app-vm-validate' "$APP_VM/Containerfile"
require 'install -D -m 0755 /tmp/mcnf-app-vm-launch /usr/local/libexec/mcnf-app-vm-launch' "$APP_VM/Containerfile"
require 'image-contract.json' "$APP_VM/Containerfile"
require 'verify-image.sh' "$APP_VM/build-image.sh"
require 'resolve_image' "$BUILD"
require 'MCNF_PULL_TIMEOUT' "$BUILD"
require '--ignorefile' "$BUILD"
require 'context.containerignore' "$BUILD"
require 'GATED[WL-FUNC-018/base-image]' "$BUILD"
require 'org.mcnf.app-vm.profile' "$BUILD"
require 'base-image-id' "$BUILD"
require 'immutable profile provenance' "$IMAGE_VERIFY"
require 'complete immutable base-image digest' "$IMAGE_VERIFY"
require 'valid_sha256_digest' "$IMAGE_VERIFY"
require '/usr/local/libexec/mcnf-app-vm-validate' "$TEMPLATE"
require 'image-contract.json' "$TEMPLATE"
require 'guest-profile' "$TEMPLATE"
require 'wayland-standard' "$TEMPLATE"
require 'App VM image profile contract is unavailable' "$TEMPLATE"
require 'dbus-run-session' "$TEMPLATE"
require '/usr/local/libexec/mcnf-app-vm-launch' "$TEMPLATE"
require "curated \"\$app_id\"" "$TEMPLATE"
require 'publish_runtime installing' "$TEMPLATE"
require 'publish_runtime starting_app' "$TEMPLATE"
require '"generation":%s' "$TEMPLATE"
require 'publish_runtime connected' "$APP_VM/mcnf-app-vm-launch.sh"
require 'publish_runtime failed' "$APP_VM/mcnf-app-vm-launch.sh"
require '"generation":%s' "$APP_VM/mcnf-app-vm-launch.sh"
require "flatpak run --system curated \"\$app_id\"" "$APP_VM/mcnf-app-vm-launch.sh"
require "trap 'handle_shutdown TERM' TERM" "$APP_VM/mcnf-app-vm-launch.sh"
require "trap 'handle_shutdown INT' INT" "$APP_VM/mcnf-app-vm-launch.sh"
require "trap 'handle_shutdown HUP' HUP" "$APP_VM/mcnf-app-vm-launch.sh"
require "kill -TERM \"\$app_pid\"" "$APP_VM/mcnf-app-vm-launch.sh"
require 'application stopped by guest supervisor' "$APP_VM/mcnf-app-vm-launch.sh"
require 'swaymsg exit' "$APP_VM/mcnf-app-vm-launch.sh"
require 'Type=simple' "$TEMPLATE"
require 'ExecStart=/usr/local/libexec/mcnf-app-vm-runtime' "$TEMPLATE"
if grep -Fq 'flatpak remote-add' "$APP_VM/Containerfile"; then
    echo "FATAL: image must not add an unsigned Flatpak remote" >&2
    exit 1
fi

echo "App VM contract checks passed"
