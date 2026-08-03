#!/usr/bin/env bash
# Prepare a bounded NoCloud seed and preflight a direct QEMU Browser VM proof.
# This helper never starts QEMU, invokes libvirt, contacts a host, or accepts
# credentials. TCG is intentionally not an allowed fallback.
set -euo pipefail

SCRIPT_NAME=$(basename "$0")
MIN_MEMORY_KIB=$((9 * 1024 * 1024))
MIN_CPU_COUNT=4
MIN_VIRTUAL_BYTES=$((10 * 1024 * 1024 * 1024))
SCRATCH_HEADROOM_BYTES=$((2 * 1024 * 1024 * 1024))
SEED_TMP=

usage() {
    cat <<'USAGE'
Usage:
  prepare-ephemeral-nocloud.sh preflight --image PATH [--run-dir DIR] [--require-gpu]
  prepare-ephemeral-nocloud.sh seed --out DIR --image-digest sha256:HEX64 \
      --session-id session:ID --transport rdp|spice
  prepare-ephemeral-nocloud.sh --self-test

preflight is read-only. It requires KVM and refuses TCG; it does not start
QEMU or modify libvirt, hosts, images, or network configuration.
seed creates a new directory containing fixed NoCloud metadata and seed.iso.
It accepts no user, password, SSH key, token, or other credential input.
USAGE
}

fail() {
    echo "$SCRIPT_NAME: $*" >&2
    return 1
}

cleanup_seed_tmp() {
    if [[ -n "${SEED_TMP:-}" && -d "$SEED_TMP" ]]; then
        rm -rf -- "$SEED_TMP"
    fi
}

trap cleanup_seed_tmp EXIT

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command is unavailable: $1"
}

validate_digest() {
    [[ "$1" =~ ^sha256:[0-9a-fA-F]{64}$ ]] || fail "image digest must match sha256:<64 hex characters>"
}

validate_session_id() {
    [[ "$1" =~ ^session:[A-Za-z0-9._:-]{1,127}$ ]] || \
        fail "session ID must match session:<1-127 safe characters>"
}

validate_transport() {
    case "$1" in
        rdp|spice) ;;
        *) fail "transport must be rdp or spice" ;;
    esac
}

validate_absolute_path() {
    local path=$1
    [[ "$path" == /* ]] || fail "path must be absolute: $path"
    case "$path" in
        */../*|*/..) fail "path must not contain parent traversal: $path" ;;
    esac
}

write_seed_files() {
    local dir=$1
    local image_digest=$2
    local session_id=$3
    local transport=$4

    printf '%s\n' \
        'instance-id: mcnf-browser-vm-ephemeral' \
        'local-hostname: mcnf-browser-vm-ephemeral' > "$dir/meta-data"

    cat > "$dir/network-config" <<'NETWORK_CONFIG'
version: 2
ethernets:
  mcnf-dhcp:
    match:
      name: "e*"
    dhcp4: true
    dhcp6: false
NETWORK_CONFIG

    cat > "$dir/user-data" <<USER_DATA
#cloud-config
bootcmd:
  - [mkdir, -p, /etc/mackesd/browser-vm]
  # The image runtime is deliberately unprivileged (mcnf-browser). Keep the
  # root-owned identity records non-writable but readable by that account;
  # validate-runtime-inputs.sh still rejects every group/other-writable mode.
  - [chmod, "0755", /etc/mackesd/browser-vm]
write_files:
  - path: /etc/mackesd/browser-vm/profile-id
    owner: root:root
    permissions: "0644"
    content: |
      browser-vm-chromium
  - path: /etc/mackesd/browser-vm/image-digest
    owner: root:root
    permissions: "0644"
    content: |
      $image_digest
  - path: /etc/mackesd/browser-vm/session-id
    owner: root:root
    permissions: "0644"
    content: |
      $session_id
  - path: /etc/mackesd/browser-vm/transport
    owner: root:root
    permissions: "0644"
    content: |
      $transport
  - path: /etc/mackesd/browser-vm/transport-health
    owner: root:root
    permissions: "0644"
    content: |
      unavailable
USER_DATA
}

build_seed_iso() {
    local dir=$1
    require_command genisoimage
    genisoimage -quiet -output "$dir/seed.iso" -volid cidata -joliet -rock \
        "$dir/user-data" "$dir/meta-data" "$dir/network-config" >/dev/null 2>&1
    [[ -s "$dir/seed.iso" ]] || fail "genisoimage produced an empty seed.iso"
}

seed() {
    local out=
    local image_digest=
    local session_id=
    local transport=

    while (($#)); do
        case "$1" in
            --out)
                (($# >= 2)) || fail "--out requires a value"
                out=$2
                shift 2
                ;;
            --image-digest)
                (($# >= 2)) || fail "--image-digest requires a value"
                image_digest=$2
                shift 2
                ;;
            --session-id)
                (($# >= 2)) || fail "--session-id requires a value"
                session_id=$2
                shift 2
                ;;
            --transport)
                (($# >= 2)) || fail "--transport requires a value"
                transport=$2
                shift 2
                ;;
            *) fail "unknown seed option: $1" ;;
        esac
    done

    [[ -n "$out" ]] || fail "seed requires --out"
    [[ -n "$image_digest" ]] || fail "seed requires --image-digest"
    [[ -n "$session_id" ]] || fail "seed requires --session-id"
    [[ -n "$transport" ]] || fail "seed requires --transport"
    validate_absolute_path "$out"
    validate_digest "$image_digest"
    validate_session_id "$session_id"
    validate_transport "$transport"
    require_command sha256sum

    [[ ! -e "$out" && ! -L "$out" ]] || fail "refusing to overwrite existing seed path: $out"
    local parent
    parent=$(dirname -- "$out")
    [[ -d "$parent" ]] || fail "seed parent directory does not exist: $parent"
    [[ ! -L "$parent" ]] || fail "seed parent directory must not be a symlink: $parent"

    SEED_TMP=$(mktemp -d "$parent/.mcnf-browser-vm-seed.XXXXXX")
    write_seed_files "$SEED_TMP" "$image_digest" "$session_id" "$transport"
    build_seed_iso "$SEED_TMP"

    local seed_sha256
    seed_sha256=$(sha256sum "$SEED_TMP/seed.iso" | awk '{print $1}')
    printf '{"schema_version":1,"kind":"browser_vm_nocloud_seed","profile":"browser-vm-chromium","image_digest":"%s","session_id":"%s","transport":"%s","transport_health":"unavailable","seed_sha256":"%s"}\n' \
        "$image_digest" "$session_id" "$transport" "$seed_sha256" > "$SEED_TMP/seed-manifest.json"
    chmod 600 "$SEED_TMP/seed-manifest.json"

    mv -- "$SEED_TMP" "$out"
    SEED_TMP=
    printf 'seed prepared: %s\n' "$out"
    printf 'transport health is fail-closed until live proof: unavailable\n'
}

read_available_memory_kib() {
    awk '$1 == "MemAvailable:" { print $2; found = 1; exit } END { if (!found) exit 1 }' /proc/meminfo
}

preflight() {
    local image=
    local run_dir=/var/tmp/mcnf-browser-vm-ephemeral
    local require_gpu=0

    while (($#)); do
        case "$1" in
            --image)
                (($# >= 2)) || fail "--image requires a value"
                image=$2
                shift 2
                ;;
            --run-dir)
                (($# >= 2)) || fail "--run-dir requires a value"
                run_dir=$2
                shift 2
                ;;
            --require-gpu)
                require_gpu=1
                shift
                ;;
            *) fail "unknown preflight option: $1" ;;
        esac
    done

    [[ -n "$image" ]] || fail "preflight requires --image"
    validate_absolute_path "$image"
    validate_absolute_path "$run_dir"
    [[ -f "$image" && ! -L "$image" ]] || fail "image must be a regular non-symlink file: $image"

    local command_name
    for command_name in qemu-img qemu-system-x86_64 genisoimage sha256sum df free nproc awk stat python3; do
        require_command "$command_name"
    done

    local image_info_json
    image_info_json=$(qemu-img info --force-share --output=json "$image") || \
        fail "qemu-img info failed for image: $image"
    local image_fields
    image_fields=$(python3 -c 'import json, sys; i=json.load(sys.stdin); print(i.get("format", ""), i.get("virtual-size", 0), "yes" if i.get("backing-filename") else "no")' <<<"$image_info_json") || \
        fail "could not parse qemu-img metadata"
    local image_format image_virtual_bytes image_has_backing
    read -r image_format image_virtual_bytes image_has_backing <<<"$image_fields"
    [[ "$image_format" == qcow2 ]] || fail "image is not qcow2"
    [[ "$image_virtual_bytes" =~ ^[0-9]+$ ]] || fail "image virtual size is not numeric"
    ((image_virtual_bytes >= MIN_VIRTUAL_BYTES)) || \
        fail "image virtual size is too small: need at least $MIN_VIRTUAL_BYTES bytes, have $image_virtual_bytes"
    [[ "$image_has_backing" == no ]] || fail "image has an external backing file; refusing an undeclared image dependency"

    if ! qemu-img check --force-share "$image" >/dev/null 2>&1; then
        fail "qemu-img check failed; refusing an image with unknown consistency"
    fi

    local image_bytes
    image_bytes=$(stat -c '%s' -- "$image")
    local run_parent
    run_parent=$(dirname -- "$run_dir")
    [[ -d "$run_parent" ]] || fail "run directory parent does not exist: $run_parent"
    local available_bytes
    available_bytes=$(df -P -B1 -- "$run_parent" | awk 'NR == 2 { print $4 }')
    [[ "$available_bytes" =~ ^[0-9]+$ ]] || fail "could not determine free space for: $run_parent"
    local required_bytes=$((image_bytes + SCRATCH_HEADROOM_BYTES))
    ((available_bytes >= required_bytes)) || \
        fail "insufficient free space in $run_parent: need at least $required_bytes bytes, have $available_bytes"

    local memory_kib
    memory_kib=$(read_available_memory_kib) || fail "could not determine MemAvailable"
    ((memory_kib >= MIN_MEMORY_KIB)) || \
        fail "insufficient MemAvailable: need at least ${MIN_MEMORY_KIB} KiB (9 GiB safety floor), have ${memory_kib} KiB"

    local cpu_count
    cpu_count=$(nproc)
    ((cpu_count >= MIN_CPU_COUNT)) || fail "insufficient CPUs: need at least $MIN_CPU_COUNT, have $cpu_count"

    [[ -r /dev/kvm ]] || fail "KVM is unavailable; this helper refuses TCG and will not deploy an emulated proof"
    [[ -r /usr/share/edk2/ovmf/OVMF_CODE.fd ]] || \
        fail "OVMF firmware is unavailable: /usr/share/edk2/ovmf/OVMF_CODE.fd"

    local render_node=
    local node
    for node in /dev/dri/renderD*; do
        if [[ -r "$node" ]]; then
            render_node=$node
            break
        fi
    done
    if ((require_gpu)); then
        [[ -n "$render_node" ]] || fail "GPU proof requested but no readable DRM render node exists"
        qemu-system-x86_64 -device help 2>/dev/null | grep -Eq 'virtio-(gpu|vga).*gl' || \
            fail "QEMU has no GL-capable virtio GPU device"
    fi

    printf 'preflight passed: KVM-backed direct QEMU proof prerequisites are present\n'
    printf 'image: %s\n' "$image"
    printf 'run directory parent: %s\n' "$run_parent"
    printf 'gpu render node: %s\n' "${render_node:-unavailable (GPU proof not requested)}"
}

self_test() {
    local fixture
    fixture=$(mktemp -d)
    trap 'rm -rf -- "$fixture"' RETURN
    local digest=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    local session=session:00000000-0000-4000-8000-000000000001

    write_seed_files "$fixture" "$digest" "$session" spice
    grep -Fq 'transport-health' "$fixture/user-data" || fail "self-test seed omitted transport health"
    grep -Fq 'unavailable' "$fixture/user-data" || fail "self-test seed is not fail-closed"
    grep -Fq 'chmod, "0755"' "$fixture/user-data" || fail "self-test seed directory is not runtime-readable"
    [[ "$(grep -Fc 'permissions: "0644"' "$fixture/user-data")" -eq 5 ]] || \
        fail "self-test seed inputs are not all runtime-readable and non-writable"
    if grep -Eiq 'password|passwd|secret|credential|token|ssh' "$fixture/user-data" "$fixture/meta-data" "$fixture/network-config"; then
        fail "self-test found a credential-shaped field in the seed"
    fi
    build_seed_iso "$fixture"
    [[ -s "$fixture/seed.iso" ]] || fail "self-test seed.iso is empty"

    mkdir "$fixture/existing"
    if "$0" seed --out "$fixture/existing" --image-digest "$digest" --session-id "$session" --transport spice >/dev/null 2>&1; then
        fail "self-test overwrote a pre-existing output directory"
    fi
    if "$0" seed --out "$fixture/new-seed" --image-digest "$digest" --session-id invalid --transport spice >/dev/null 2>&1; then
        fail "self-test accepted an invalid session ID"
    fi
    if "$0" seed --out "$fixture/new-seed" --image-digest "$digest" --session-id "$session" --transport sunshine >/dev/null 2>&1; then
        fail "self-test accepted an unsupported transport"
    fi
    if "$0" seed --out "$fixture/new-seed" --image-digest sha256:not-a-digest --session-id "$session" --transport spice >/dev/null 2>&1; then
        fail "self-test accepted an invalid image digest"
    fi
    echo 'prepare-ephemeral-nocloud: self-test passed'
}

if (($# == 1)) && [[ "$1" == --self-test ]]; then
    self_test
    exit 0
fi

(($# >= 1)) || { usage >&2; exit 2; }
case "$1" in
    preflight)
        shift
        preflight "$@"
        ;;
    seed)
        shift
        seed "$@"
        ;;
    --help|-h)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
