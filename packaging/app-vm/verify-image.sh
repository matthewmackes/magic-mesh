#!/usr/bin/env bash
# WL-FUNC-018 — static acceptance checks for a built App VM image.
#
# This inspects image contents without booting a guest. Live Flatpak, Sway, and
# VDI convergence remain separate runtime gates; an image missing the fixed
# guest contract must never reach those gates.
set -euo pipefail

TAG="${1:-localhost/magic-mesh-app-vm-wayland:latest}"

PROVENANCE_KEYS="schema_version profile base_image_id source_commit"
READINESS_KEYS="schema_version profile compositor compositor_executable compositor_ownership supervisor_unit supervisor_entrypoint supervisor_ownership readiness_topic ready_state not_ready_states host_fallback"

valid_sha256_digest() {
    [[ "$1" =~ ^sha256:[0-9a-fA-F]{64}$ ]]
}

valid_source_commit() {
    [[ "$1" =~ ^[0-9a-f]{40}$ && "$1" != 0000000000000000000000000000000000000000 ]]
}

manifest_has_exact_keys() {
    local file=$1 expected=$2 line key value count=0 expected_count=0
    local -A seen=()

    [ -r "$file" ] || return 1
    for key in $expected; do
        expected_count=$((expected_count + 1))
    done
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            [a-z_]*=*) ;;
            *) return 1 ;;
        esac
        key=${line%%=*}
        value=${line#*=}
        [[ "$key" =~ ^[a-z_]+$ ]] || return 1
        [ -n "$value" ] || return 1
        case "$value" in
            *[[:cntrl:]=]*) return 1 ;;
        esac
        case " $expected " in
            *" $key "*) ;;
            *) return 1 ;;
        esac
        [ -z "${seen[$key]+present}" ] || return 1
        seen["$key"]=$value
        count=$((count + 1))
    done < "$file"

    [ "$count" -eq "$expected_count" ] || return 1
    for key in $expected; do
        [ -n "${seen[$key]+present}" ] || return 1
    done
}

manifest_value() {
    local file=$1 wanted_key=$2 line key value found=0
    while IFS= read -r line || [ -n "$line" ]; do
        key=${line%%=*}
        if [ "$key" = "$wanted_key" ]; then
            [ "$found" -eq 0 ] || return 1
            value=${line#*=}
            printf '%s\n' "$value"
            found=1
        fi
    done < "$file"
    [ "$found" -eq 1 ]
}

manifest_value_is() {
    local file=$1 key=$2 expected=$3 actual
    actual=$(manifest_value "$file" "$key") || return 1
    [ "$actual" = "$expected" ]
}

valid_provenance_manifest() {
    local file=$1 expected_base_id=$2 expected_source_commit=$3
    manifest_has_exact_keys "$file" "$PROVENANCE_KEYS" && \
        manifest_value_is "$file" schema_version 1 && \
        manifest_value_is "$file" profile wayland-standard-v1 && \
        manifest_value_is "$file" base_image_id "$expected_base_id" && \
        manifest_value_is "$file" source_commit "$expected_source_commit"
}

valid_readiness_manifest() {
    local file=$1
    manifest_has_exact_keys "$file" "$READINESS_KEYS" && \
        manifest_value_is "$file" schema_version 1 && \
        manifest_value_is "$file" profile wayland-standard-v1 && \
        manifest_value_is "$file" compositor sway && \
        manifest_value_is "$file" compositor_executable /usr/bin/sway && \
        manifest_value_is "$file" compositor_ownership guest && \
        manifest_value_is "$file" supervisor_unit mcnf-app-vm-runtime.service && \
        manifest_value_is "$file" supervisor_entrypoint /usr/local/libexec/mcnf-app-vm-launch && \
        manifest_value_is "$file" supervisor_ownership guest && \
        manifest_value_is "$file" readiness_topic state/vdi/app-runtime && \
        manifest_value_is "$file" ready_state connected && \
        manifest_value_is "$file" not_ready_states installing,starting_app,reconnecting,unavailable,failed && \
        manifest_value_is "$file" host_fallback disabled
}

if [[ "${1:-}" == "--self-test" ]]; then
    valid_sha256_digest "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" || {
        echo "FATAL: valid sha256 digest rejected" >&2
        exit 1
    }
    for invalid in \
        "" \
        "sha256:" \
        "sha256:deadbeef" \
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg" \
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-extra"; do
        if valid_sha256_digest "$invalid"; then
            echo "FATAL: malformed sha256 digest accepted: $invalid" >&2
            exit 1
        fi
    done
    valid_source_commit "0123456789abcdef0123456789abcdef01234567" || {
        echo "FATAL: valid source commit rejected" >&2
        exit 1
    }
    for invalid_commit in \
        "" \
        "not-a-revision" \
        "0000000000000000000000000000000000000000" \
        "0123456789abcdef0123456789abcdef0123456G" \
        "0123456789abcdef0123456789abcdef0123456789"; do
        if valid_source_commit "$invalid_commit"; then
            echo "FATAL: malformed source commit accepted: $invalid_commit" >&2
            exit 1
        fi
    done

    fixture=$(mktemp -d)
    trap 'rm -rf "$fixture"' EXIT
    valid_base_id="sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    valid_source_commit="0123456789abcdef0123456789abcdef01234567"
    printf '%s\n' \
        'schema_version=1' \
        'profile=wayland-standard-v1' \
        "base_image_id=$valid_base_id" \
        "source_commit=$valid_source_commit" \
        > "$fixture/provenance.valid"
    valid_provenance_manifest "$fixture/provenance.valid" "$valid_base_id" "$valid_source_commit" || {
        echo "FATAL: valid immutable provenance manifest rejected" >&2
        exit 1
    }

    printf '%s\n' \
        'schema_version=1' \
        'profile=wayland-standard-v1' \
        "source_commit=$valid_source_commit" \
        > "$fixture/provenance.missing-base"
    if valid_provenance_manifest "$fixture/provenance.missing-base" "$valid_base_id" "$valid_source_commit"; then
        echo "FATAL: provenance manifest with missing base digest accepted" >&2
        exit 1
    fi

    printf '%s\n' \
        'schema_version=1' \
        'profile=wayland-standard-v1' \
        "base_image_id=$valid_base_id" \
        "source_commit=$valid_source_commit" \
        "source_commit=$valid_source_commit" \
        > "$fixture/provenance.duplicate"
    if valid_provenance_manifest "$fixture/provenance.duplicate" "$valid_base_id" "$valid_source_commit"; then
        echo "FATAL: provenance manifest with duplicate source evidence accepted" >&2
        exit 1
    fi

    printf '%s\n' \
        'schema_version=1' \
        'profile=wayland-standard-v1' \
        "base_image_id=sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210" \
        "source_commit=$valid_source_commit" \
        > "$fixture/provenance.mismatch"
    if valid_provenance_manifest "$fixture/provenance.mismatch" "$valid_base_id" "$valid_source_commit"; then
        echo "FATAL: mismatched immutable base provenance accepted" >&2
        exit 1
    fi

    printf '%s\n' \
        'schema_version=1' \
        'profile=wayland-standard-v1' \
        'compositor=sway' \
        'compositor_executable=/usr/bin/sway' \
        'compositor_ownership=guest' \
        'supervisor_unit=mcnf-app-vm-runtime.service' \
        'supervisor_entrypoint=/usr/local/libexec/mcnf-app-vm-launch' \
        'supervisor_ownership=guest' \
        'readiness_topic=state/vdi/app-runtime' \
        'ready_state=connected' \
        'not_ready_states=installing,starting_app,reconnecting,unavailable,failed' \
        'host_fallback=disabled' \
        > "$fixture/readiness.valid"
    valid_readiness_manifest "$fixture/readiness.valid" || {
        echo "FATAL: valid compositor/supervisor readiness manifest rejected" >&2
        exit 1
    }

    sed '/^compositor=/d' "$fixture/readiness.valid" > "$fixture/readiness.missing-compositor"
    if valid_readiness_manifest "$fixture/readiness.missing-compositor"; then
        echo "FATAL: readiness manifest with missing compositor evidence accepted" >&2
        exit 1
    fi
    sed 's/^compositor=sway$/compositor=sway,weston/' "$fixture/readiness.valid" > "$fixture/readiness.ambiguous-compositor"
    if valid_readiness_manifest "$fixture/readiness.ambiguous-compositor"; then
        echo "FATAL: ambiguous compositor readiness evidence accepted" >&2
        exit 1
    fi
    sed 's/^host_fallback=disabled$/host_fallback=enabled/' "$fixture/readiness.valid" > "$fixture/readiness.host-fallback"
    if valid_readiness_manifest "$fixture/readiness.host-fallback"; then
        echo "FATAL: host fallback readiness evidence accepted" >&2
        exit 1
    fi
    {
        cat "$fixture/readiness.valid"
        printf '%s\n' 'supervisor_unit=mcnf-app-vm-runtime.service'
    } > "$fixture/readiness.duplicate-supervisor"
    if valid_readiness_manifest "$fixture/readiness.duplicate-supervisor"; then
        echo "FATAL: duplicate supervisor readiness evidence accepted" >&2
        exit 1
    fi

    echo "App VM image provenance/readiness self-tests passed"
    exit 0
fi

command -v podman >/dev/null 2>&1 || {
    echo "FATAL: podman not on PATH" >&2
    exit 2
}
podman image exists "$TAG" || {
    echo "FATAL: App VM image is not in local storage: $TAG (build it first)" >&2
    exit 1
}

profile="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.profile"}}' "$TAG" 2>/dev/null || true)"
base_id="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.base-image-id"}}' "$TAG" 2>/dev/null || true)"
source_commit="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.source-commit"}}' "$TAG" 2>/dev/null || true)"
[ "$profile" = "wayland-standard-v1" ] || {
    echo "FATAL: App VM image is missing immutable profile provenance" >&2
    exit 1
}
if ! valid_sha256_digest "$base_id"; then
    echo "FATAL: App VM image is missing a complete immutable base-image digest" >&2
    exit 1
fi
if ! valid_source_commit "$source_commit"; then
    echo "FATAL: App VM image is missing a non-null source commit provenance label" >&2
    exit 1
fi
echo "  OK   immutable image provenance: profile=$profile base_id=$base_id source_commit=$source_commit"

INNER_SCRIPT="$(cat <<'INNER'
set -u
fail=0
ok()  { echo "  OK   $1"; }
bad() { echo "  FAIL $1"; fail=1; }

PROVENANCE_KEYS="schema_version profile base_image_id source_commit"
READINESS_KEYS="schema_version profile compositor compositor_executable compositor_ownership supervisor_unit supervisor_entrypoint supervisor_ownership readiness_topic ready_state not_ready_states host_fallback"

manifest_has_exact_keys() {
    local file=$1 expected=$2 line key value count=0 expected_count=0
    local -A seen=()

    [ -r "$file" ] || return 1
    for key in $expected; do
        expected_count=$((expected_count + 1))
    done
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            [a-z_]*=*) ;;
            *) return 1 ;;
        esac
        key=${line%%=*}
        value=${line#*=}
        [[ "$key" =~ ^[a-z_]+$ ]] || return 1
        [ -n "$value" ] || return 1
        case "$value" in
            *[[:cntrl:]=]*) return 1 ;;
        esac
        case " $expected " in
            *" $key "*) ;;
            *) return 1 ;;
        esac
        [ -z "${seen[$key]+present}" ] || return 1
        seen["$key"]=$value
        count=$((count + 1))
    done < "$file"

    [ "$count" -eq "$expected_count" ] || return 1
    for key in $expected; do
        [ -n "${seen[$key]+present}" ] || return 1
    done
}

manifest_value() {
    local file=$1 wanted_key=$2 line key value found=0
    while IFS= read -r line || [ -n "$line" ]; do
        key=${line%%=*}
        if [ "$key" = "$wanted_key" ]; then
            [ "$found" -eq 0 ] || return 1
            value=${line#*=}
            printf '%s\n' "$value"
            found=1
        fi
    done < "$file"
    [ "$found" -eq 1 ]
}

manifest_value_is() {
    local file=$1 key=$2 expected=$3 actual
    actual=$(manifest_value "$file" "$key") || return 1
    [ "$actual" = "$expected" ]
}

valid_readiness_manifest() {
    local file=$1
    manifest_has_exact_keys "$file" "$READINESS_KEYS" && \
        manifest_value_is "$file" schema_version 1 && \
        manifest_value_is "$file" profile "$MCNF_EXPECTED_PROFILE" && \
        manifest_value_is "$file" compositor sway && \
        manifest_value_is "$file" compositor_executable /usr/bin/sway && \
        manifest_value_is "$file" compositor_ownership guest && \
        manifest_value_is "$file" supervisor_unit mcnf-app-vm-runtime.service && \
        manifest_value_is "$file" supervisor_entrypoint /usr/local/libexec/mcnf-app-vm-launch && \
        manifest_value_is "$file" supervisor_ownership guest && \
        manifest_value_is "$file" readiness_topic state/vdi/app-runtime && \
        manifest_value_is "$file" ready_state connected && \
        manifest_value_is "$file" not_ready_states installing,starting_app,reconnecting,unavailable,failed && \
        manifest_value_is "$file" host_fallback disabled
}

for path in \
    /usr/local/libexec/mcnf-app-vm-validate \
    /usr/local/libexec/mcnf-app-vm-launch \
    /usr/local/libexec/mcnf-app-vm-runtime-probe \
    /usr/share/mcnf/app-vm/wayland-standard.profile \
    /usr/share/mcnf/app-vm/image-contract.json \
    /usr/share/mcnf/app-vm/source-commit \
    /usr/share/mcnf/app-vm/image-provenance \
    /usr/share/mcnf/app-vm/runtime-readiness; do
    [ -f "$path" ] && ok "image file present: $path" || bad "image file missing: $path"
done

provenance_file=/usr/share/mcnf/app-vm/image-provenance
readiness_file=/usr/share/mcnf/app-vm/runtime-readiness
if manifest_has_exact_keys "$provenance_file" "$PROVENANCE_KEYS"; then
    manifest_value_is "$provenance_file" schema_version 1 \
        && ok 'image provenance schema is version 1' \
        || bad 'image provenance schema is ambiguous or invalid'
    manifest_value_is "$provenance_file" profile "$MCNF_EXPECTED_PROFILE" \
        && ok 'image provenance profile matches image label' \
        || bad 'image provenance profile does not match image label'
    manifest_value_is "$provenance_file" base_image_id "$MCNF_EXPECTED_BASE_IMAGE_ID" \
        && ok 'guest base-image provenance matches image label' \
        || bad 'guest base-image provenance does not match image label'
    manifest_value_is "$provenance_file" source_commit "$MCNF_EXPECTED_SOURCE_COMMIT" \
        && ok 'guest source provenance matches image label' \
        || bad 'guest source provenance does not match image label'
else
    bad 'image provenance evidence is missing, duplicated, or ambiguous'
fi

if manifest_has_exact_keys "$readiness_file" "$READINESS_KEYS"; then
    valid_readiness_manifest "$readiness_file" \
        && ok 'guest compositor/supervisor readiness evidence is unambiguous' \
        || bad 'guest compositor/supervisor readiness evidence is missing or ambiguous'
else
    bad 'guest compositor/supervisor readiness evidence is missing, duplicated, or ambiguous'
fi

compositor_executable="$(manifest_value "$readiness_file" compositor_executable 2>/dev/null || true)"
supervisor_entrypoint="$(manifest_value "$readiness_file" supervisor_entrypoint 2>/dev/null || true)"
[ "$compositor_executable" = /usr/bin/sway ] && [ -x "$compositor_executable" ] \
    && ok 'guest compositor evidence points to executable Sway' \
    || bad 'guest compositor evidence does not point to executable Sway'
[ "$supervisor_entrypoint" = /usr/local/libexec/mcnf-app-vm-launch ] \
    && [ -x "$supervisor_entrypoint" ] \
    && ok 'guest supervisor evidence points to image-owned launcher' \
    || bad 'guest supervisor evidence does not point to image-owned launcher'

for binary in flatpak sway dbus-run-session dbus-send pw-cli pactl timeout; do
    command -v "$binary" >/dev/null 2>&1 \
        && ok "runtime binary present: $binary" \
        || bad "runtime binary missing: $binary"
done

for package in \
    magic-mesh flatpak sway xdg-desktop-portal xdg-desktop-portal-wlr \
    xdg-desktop-portal-gtk pipewire pipewire-pulseaudio wireplumber \
    libinput libxkbcommon; do
    rpm -q "$package" >/dev/null 2>&1 \
        && ok "package installed: $package" \
        || bad "package missing: $package"
done

grep -Fxq 'profile=wayland-standard' /usr/share/mcnf/app-vm/wayland-standard.profile \
    && ok 'profile selects wayland-standard' \
    || bad 'profile does not select wayland-standard'
grep -Fq '"schema_version":1' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract schema is version 1' \
    || bad 'image contract schema marker missing'
grep -Fq '"profile":"wayland-standard"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies wayland-standard' \
    || bad 'image contract profile missing'
grep -Fq '"compositor":"sway"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies Sway' \
    || bad 'image contract compositor missing'
grep -Fq '"flatpak_remote":"curated"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies curated remote' \
    || bad 'image contract remote policy missing'
[ "$(cat /usr/share/mcnf/app-vm/source-commit 2>/dev/null || true)" = "$MCNF_EXPECTED_SOURCE_COMMIT" ] \
    && ok 'legacy guest source marker matches image label' \
    || bad 'legacy guest source marker does not match image label'
! flatpak remotes --system --columns=name 2>/dev/null | grep -Fxq flathub \
    && ok 'image does not pre-admit public flathub' \
    || bad 'image pre-admits public flathub'
! grep -R -Fq 'flatpak remote-add' /usr/local/libexec /usr/share/mcnf/app-vm 2>/dev/null \
    && ok 'image has no unsigned remote-add helper' \
    || bad 'image contains an unsigned remote-add helper'

exit "$fail"
INNER
)"

rc=0
out="$(printf '%s\n' "$INNER_SCRIPT" | podman run --rm -i \
    -e "MCNF_EXPECTED_PROFILE=$profile" \
    -e "MCNF_EXPECTED_BASE_IMAGE_ID=$base_id" \
    -e "MCNF_EXPECTED_SOURCE_COMMIT=$source_commit" "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"
grep -q '^  OK ' <<<"$out" || {
    echo "FATAL: no App VM image checks executed" >&2
    rc=1
}
grep -q '^  FAIL ' <<<"$out" && rc=1
if [ "$rc" -eq 0 ]; then
    echo "==> verify-app-vm-image: ALL STATIC CHECKS PASS for $TAG"
else
    echo "==> verify-app-vm-image: FAILURES above for $TAG" >&2
fi
exit "$rc"
