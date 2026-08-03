#!/usr/bin/env bash
# Publish the immutable Browser VM qcow2 to a KVM host.
# Inspection is the default; remote mutation requires `publish --apply`.
set -euo pipefail

SCRIPT_NAME=$(basename "$0")
DEFAULT_REMOTE_IMAGE=/var/lib/libvirt/images/browser-vm-chromium.qcow2
DEFAULT_REMOTE_STAGING=/var/tmp/mcnf-browser-vm-chromium.qcow2
readonly MIN_VIRTUAL_BYTES=$((10 * 1024 * 1024 * 1024))

usage() {
    cat <<'USAGE'
Usage:
  deploy-image.sh preflight --image PATH --target HOST [options]
  deploy-image.sh publish --image PATH --target HOST [options] [--apply]
  deploy-image.sh --self-test

Options:
  --user USER                 SSH user (default: mm)
  --identity PATH             SSH identity file (optional)
  --remote-image PATH         final qcow2 path
  --remote-staging PATH       temporary remote qcow2 path
  --expected-digest DIGEST    require sha256:<64 hex> and verify the image
  --apply                     publish; otherwise the command is dry-run

The remote preflight requires KVM, qemu-img, and passwordless sudo. Existing
images are copied to a timestamped backup before atomic installation.
USAGE
}

fail() { echo "$SCRIPT_NAME: $*" >&2; return 1; }
require_command() { command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"; }
valid_host() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,252}$ ]]; }
valid_user() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$ ]]; }
valid_path() {
    [[ "$1" =~ ^/[A-Za-z0-9._/@+-]+$ ]] || return 1
    [[ "$1" != *..* ]]
}
valid_digest() { [[ "$1" =~ ^sha256:[0-9a-f]{64}$ ]]; }

ssh_args() {
    SSH_ARGS=(ssh -o BatchMode=yes -o PasswordAuthentication=no
        -o KbdInteractiveAuthentication=no -o StrictHostKeyChecking=yes
        -o ConnectTimeout=8)
    [[ -n "${IDENTITY:-}" ]] && SSH_ARGS+=(-i "$IDENTITY")
}

target_label() { printf '%s@%s' "$USER_NAME" "$TARGET"; }

local_image_digest() {
    sha256sum -- "$IMAGE" | awk '{print "sha256:" $1}'
}

local_image_check() {
    require_command qemu-img
    require_command sha256sum
    require_command python3
    [[ -f "$IMAGE" && ! -L "$IMAGE" ]] || fail "image must be a regular non-symlink file: $IMAGE"
    local info format virtual backing digest
    info=$(qemu-img info --force-share --output=json -- "$IMAGE") || fail "qemu-img info failed"
    read -r format virtual backing < <(python3 -c 'import json,sys; x=json.load(sys.stdin); print(x.get("format",""),x.get("virtual-size",0),"yes" if x.get("backing-filename") else "no")' <<<"$info")
    [[ "$format" == qcow2 ]] || fail "image format is not qcow2"
    [[ "$virtual" =~ ^[0-9]+$ && "$virtual" -ge "$MIN_VIRTUAL_BYTES" ]] || fail "image virtual size is below 10 GiB"
    [[ "$backing" == no ]] || fail "image has an external backing file"
    qemu-img check --force-share -- "$IMAGE" >/dev/null 2>&1 || fail "qemu-img check failed"
    digest=$(local_image_digest)
    [[ -z "$EXPECTED_DIGEST" || "$EXPECTED_DIGEST" == "$digest" ]] || fail "image digest mismatch: expected $EXPECTED_DIGEST, got $digest"
    printf '%s\n' "$digest"
}

remote_preflight() {
    ssh_args
    local destination
    destination=$(target_label)
    [[ "$REMOTE_IMAGE" != "$REMOTE_STAGING" ]] || fail "remote image and staging paths must differ"
    "${SSH_ARGS[@]}" -- "$destination" "
        test -r /dev/kvm
        command -v qemu-img >/dev/null
        sudo -n true
        test -d '$(dirname -- "$REMOTE_IMAGE")'
        test ! -L '$(dirname -- "$REMOTE_IMAGE")'
        test -d '$(dirname -- "$REMOTE_STAGING")'
        test ! -L '$(dirname -- "$REMOTE_STAGING")'
        if sudo -n test -e '$REMOTE_IMAGE'; then
            sudo -n test -f '$REMOTE_IMAGE'
            sudo -n test ! -L '$REMOTE_IMAGE'
        fi
    " >/dev/null || fail "remote target failed KVM/qemu-img/sudo/path preflight: $destination"
}

parse_options() {
    IMAGE='' TARGET='' USER_NAME=mm IDENTITY='' REMOTE_IMAGE=$DEFAULT_REMOTE_IMAGE
    REMOTE_STAGING=$DEFAULT_REMOTE_STAGING EXPECTED_DIGEST='' APPLY=0
    while (($#)); do
        case "$1" in
            --image) (($# >= 2)) || fail "--image requires a value"; IMAGE=$2; shift 2 ;;
            --target) (($# >= 2)) || fail "--target requires a value"; TARGET=$2; shift 2 ;;
            --user) (($# >= 2)) || fail "--user requires a value"; USER_NAME=$2; shift 2 ;;
            --identity) (($# >= 2)) || fail "--identity requires a value"; IDENTITY=$2; shift 2 ;;
            --remote-image) (($# >= 2)) || fail "--remote-image requires a value"; REMOTE_IMAGE=$2; shift 2 ;;
            --remote-staging) (($# >= 2)) || fail "--remote-staging requires a value"; REMOTE_STAGING=$2; shift 2 ;;
            --expected-digest) (($# >= 2)) || fail "--expected-digest requires a value"; EXPECTED_DIGEST=$2; shift 2 ;;
            --apply) APPLY=1; shift ;;
            *) fail "unknown option: $1" ;;
        esac
    done
    [[ -n "$IMAGE" && -n "$TARGET" ]] || fail "--image and --target are required"
    valid_host "$TARGET" || fail "unsafe target host"
    valid_user "$USER_NAME" || fail "unsafe SSH user"
    valid_path "$IMAGE" || fail "unsafe local image path"
    valid_path "$REMOTE_IMAGE" || fail "unsafe remote image path"
    valid_path "$REMOTE_STAGING" || fail "unsafe remote staging path"
    [[ -z "$IDENTITY" || ( -f "$IDENTITY" && ! -L "$IDENTITY" ) ]] || fail "identity must be a regular non-symlink file"
    if [[ -n "$EXPECTED_DIGEST" ]]; then
        valid_digest "$EXPECTED_DIGEST" || fail "invalid expected digest"
    fi
}

run_action() {
    local action=$1
    shift
    parse_options "$@"
    if [[ "$action" != publish && "$APPLY" -eq 1 ]]; then
        fail "--apply is only valid with the publish action"
    fi
    local digest
    digest=$(local_image_check)
    remote_preflight
    printf 'Browser VM image preflight passed\nimage=%s\ndigest=%s\ntarget=%s\nremote_image=%s\n' \
        "$IMAGE" "$digest" "$(target_label)" "$REMOTE_IMAGE"
    (( APPLY == 1 )) || { echo 'dry-run: no remote files changed'; return 0; }
    require_command rsync
    ssh_args
    local destination backup remote_tmp rsync_ssh
    destination=$(target_label)
    remote_tmp="${REMOTE_STAGING}.${digest#sha256:}"
    backup="${REMOTE_IMAGE}.backup.$(date -u +%Y%m%dT%H%M%SZ)"
    printf -v rsync_ssh '%q ' "${SSH_ARGS[@]}"
    rsync -az --partial --chmod=F600 -e "$rsync_ssh" -- "$IMAGE" "$destination:$remote_tmp"
    "${SSH_ARGS[@]}" -- "$destination" "
        sudo -n qemu-img check --force-share '$remote_tmp'
        if sudo -n test -e '$REMOTE_IMAGE'; then sudo -n cp -a -- '$REMOTE_IMAGE' '$backup'; fi
        sudo -n install -o root -g root -m 0640 '$remote_tmp' '${REMOTE_IMAGE}.new'
        sudo -n mv -T -- '${REMOTE_IMAGE}.new' '$REMOTE_IMAGE'
        sudo -n rm -f -- '$remote_tmp'
        test \"\$(sudo -n sha256sum -- '$REMOTE_IMAGE' | awk '{print \"sha256:\" \$1}')\" = '$digest'
    "
    echo "published: $destination:$REMOTE_IMAGE (backup=$backup)"
}

self_test() {
    expect_reject() { "$@" && return 1 || return 0; }
    valid_host 172.20.146.225
    valid_user mm
    valid_path /var/lib/libvirt/images/browser-vm-chromium.qcow2
    expect_reject valid_path /tmp/../escape
    expect_reject valid_path '/tmp/unsafe path'
    expect_reject valid_host 'host;touch /tmp/pwned'
    expect_reject valid_host '[::1]'
    valid_digest sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    expect_reject valid_digest sha256:not-a-digest
    echo 'deploy-image: self-test passed'
}

if (($# == 1)) && [[ "$1" == --self-test ]]; then self_test; exit 0; fi
if (($# == 1)) && [[ "$1" == --help || "$1" == -h ]]; then usage; exit 0; fi
(($# >= 1)) || { usage >&2; exit 2; }
action=$1; shift
case "$action" in
    preflight) run_action preflight "$@" ;;
    publish) run_action publish "$@" ;;
    *) usage >&2; exit 2 ;;
esac
