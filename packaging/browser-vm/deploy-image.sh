#!/usr/bin/env bash
# Publish the immutable Browser VM qcow2 base to a KVM host.
# Inspection is the default; remote mutation requires `publish --apply`.
set -euo pipefail

SCRIPT_NAME=$(basename "$0")
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DEPLOYMENT_VERIFY="$ROOT/install-helpers/verify-browser-vm-deployment.py"
DEFAULT_REMOTE_IMAGE=/var/lib/libvirt/images/browser-vm-chromium.qcow2
DEFAULT_REMOTE_STAGING=/var/tmp/mcnf-browser-vm-chromium.qcow2
readonly MIN_VIRTUAL_BYTES=$((64 * 1024 * 1024 * 1024))

usage() {
    cat <<'USAGE'
Usage:
  deploy-image.sh preflight --image PATH --target HOST [options]
  deploy-image.sh publish --image PATH --target HOST [options] [--apply]
  deploy-image.sh receipt --target HOST --source-commit SHA \
    --expected-digest sha256:<64-hex> --receipt PATH [options]
  deploy-image.sh --self-test

Options:
  --user USER                 SSH user (default: mm)
  --identity PATH             SSH identity file (optional)
  --remote-image PATH         final qcow2 path
  --remote-staging PATH       temporary remote qcow2 path
  --expected-digest DIGEST    require sha256:<64 hex> and verify the image
  --source-commit SHA         Browser source revision for a deployment receipt
  --domain NAME               libvirt domain name (default: browser-vm)
  --receipt PATH               private deployment receipt output path
  --apply                     publish; otherwise the command is dry-run

The remote preflight requires KVM, qemu-img, the qemu group, and passwordless
sudo. Existing bases are copied to a timestamped backup before atomic
installation as root:qemu mode 0440. A schema-v2 receipt requires a running
domain whose sole vda is a writable qcow2 overlay backed directly by this base.
USAGE
}

fail() { echo "$SCRIPT_NAME: $*" >&2; return 1; }
require_command() { command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"; }
valid_host() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,252}$ ]]; }
valid_user() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$ ]]; }
valid_commit() { [[ "$1" =~ ^[0-9a-f]{40}$ && "$1" != 0000000000000000000000000000000000000000 ]]; }
valid_domain() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$ ]]; }
valid_uuid() { [[ "$1" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]]; }
valid_path() {
    [[ "$1" =~ ^/([A-Za-z0-9._@+-]+/)*[A-Za-z0-9._@+-]+$ ]] || return 1
    local component remainder=${1#/}
    while [[ -n "$remainder" ]]; do
        component=${remainder%%/*}
        [[ -n "$component" && "$component" != . && "$component" != .. ]] || return 1
        [[ "$remainder" == */* ]] || break
        remainder=${remainder#*/}
    done
    return 0
}
valid_digest() { [[ "$1" =~ ^sha256:[0-9a-f]{64}$ ]]; }

valid_backing_contract() {
    local attached_disk=$1 backing_image=$2 remote_image=$3
    local remote_format=$4 attached_format=$5 chain_depth=$6
    valid_path "$remote_image" || return 1
    valid_path "$attached_disk" || return 1
    valid_path "$backing_image" || return 1
    [[ "$attached_disk" != "$remote_image" ]] || return 1
    [[ "$backing_image" == "$remote_image" ]] || return 1
    [[ "$remote_format" == qcow2 && "$attached_format" == qcow2 ]] || return 1
    [[ "$chain_depth" == 1 ]]
}

ssh_args() {
    SSH_ARGS=(ssh -o BatchMode=yes -o PasswordAuthentication=no
        -o KbdInteractiveAuthentication=no -o StrictHostKeyChecking=yes
        -o ConnectTimeout=8)
    if [[ -n "${IDENTITY:-}" ]]; then
        SSH_ARGS+=(-i "$IDENTITY")
    fi
    return 0
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
    [[ "$virtual" =~ ^[0-9]+$ && "$virtual" -ge "$MIN_VIRTUAL_BYTES" ]] || fail "image virtual size is below 64 GiB"
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
        set -eu
        test -r /dev/kvm
        command -v qemu-img >/dev/null
        command -v getent >/dev/null
        qemu_gid=\$(getent group qemu | awk -F: 'NR == 1 && \$1 == \"qemu\" && \$3 ~ /^[0-9]+\$/ { print \$3 }')
        test -n \"\$qemu_gid\"
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
    REMOTE_STAGING=$DEFAULT_REMOTE_STAGING EXPECTED_DIGEST='' SOURCE_COMMIT=''
    DOMAIN_NAME=browser-vm RECEIPT='' APPLY=0
    while (($#)); do
        case "$1" in
            --image) (($# >= 2)) || fail "--image requires a value"; IMAGE=$2; shift 2 ;;
            --target) (($# >= 2)) || fail "--target requires a value"; TARGET=$2; shift 2 ;;
            --user) (($# >= 2)) || fail "--user requires a value"; USER_NAME=$2; shift 2 ;;
            --identity) (($# >= 2)) || fail "--identity requires a value"; IDENTITY=$2; shift 2 ;;
            --remote-image) (($# >= 2)) || fail "--remote-image requires a value"; REMOTE_IMAGE=$2; shift 2 ;;
            --remote-staging) (($# >= 2)) || fail "--remote-staging requires a value"; REMOTE_STAGING=$2; shift 2 ;;
            --expected-digest) (($# >= 2)) || fail "--expected-digest requires a value"; EXPECTED_DIGEST=$2; shift 2 ;;
            --source-commit) (($# >= 2)) || fail "--source-commit requires a value"; SOURCE_COMMIT=$2; shift 2 ;;
            --domain) (($# >= 2)) || fail "--domain requires a value"; DOMAIN_NAME=$2; shift 2 ;;
            --receipt) (($# >= 2)) || fail "--receipt requires a value"; RECEIPT=$2; shift 2 ;;
            --apply) APPLY=1; shift ;;
            *) fail "unknown option: $1" ;;
        esac
    done
    [[ -n "$TARGET" ]] || fail "--target is required"
    valid_host "$TARGET" || fail "unsafe target host"
    valid_user "$USER_NAME" || fail "unsafe SSH user"
    if [[ -n "$IMAGE" ]]; then
        valid_path "$IMAGE" || fail "unsafe local image path"
    fi
    valid_path "$REMOTE_IMAGE" || fail "unsafe remote image path"
    valid_path "$REMOTE_STAGING" || fail "unsafe remote staging path"
    valid_domain "$DOMAIN_NAME" || fail "unsafe libvirt domain name"
    if [[ -n "$RECEIPT" ]]; then
        valid_path "$RECEIPT" || fail "unsafe receipt path"
    fi
    [[ -z "$IDENTITY" || ( -f "$IDENTITY" && ! -L "$IDENTITY" ) ]] || fail "identity must be a regular non-symlink file"
    if [[ -n "$EXPECTED_DIGEST" ]]; then
        valid_digest "$EXPECTED_DIGEST" || fail "invalid expected digest"
    fi
}

write_receipt() {
    local node_hostname="$1" domain_uuid="$2" domain_state="$3" attached_disk="$4"
    local remote_digest="$5" backing_image="$6" remote_format="$7"
    local attached_format="$8" chain_depth="$9" destination
    destination=$(target_label)
    [[ -n "$RECEIPT" ]] || fail "receipt action requires --receipt"
    [[ -n "$SOURCE_COMMIT" ]] || fail "receipt action requires --source-commit"
    [[ -n "$EXPECTED_DIGEST" ]] || fail "receipt action requires --expected-digest"
    valid_commit "$SOURCE_COMMIT" || fail "invalid source commit"
    valid_digest "$EXPECTED_DIGEST" || fail "invalid expected digest"
    valid_host "$node_hostname" || fail "remote node hostname is malformed"
    valid_uuid "$domain_uuid" || fail "remote domain UUID is malformed"
    [[ "$remote_digest" == "$EXPECTED_DIGEST" ]] || fail "remote image digest is not the expected digest"
    [[ "$domain_state" == running ]] || fail "Browser VM domain is not running: $domain_state"
    valid_backing_contract "$attached_disk" "$backing_image" "$REMOTE_IMAGE" \
        "$remote_format" "$attached_format" "$chain_depth" || \
        fail "Browser VM does not use one writable qcow2 overlay backed directly by the immutable base"
    [[ -x "$DEPLOYMENT_VERIFY" ]] || fail "deployment receipt verifier is unavailable"
    python3 - "$RECEIPT" "$TARGET" "$node_hostname" "$DOMAIN_NAME" "$domain_uuid" \
        "$domain_state" "$REMOTE_IMAGE" "$remote_format" "$attached_disk" \
        "$attached_format" "$backing_image" "$chain_depth" "$SOURCE_COMMIT" \
        "$EXPECTED_DIGEST" "$remote_digest" <<'PY'
import json
import os
from pathlib import Path
import sys
from datetime import datetime, timezone

out = Path(sys.argv[1])
if out.is_symlink() or out.exists():
    raise SystemExit(f"receipt output already exists or is a symlink: {out}")
if not out.is_absolute() or ".." in out.parts:
    raise SystemExit("receipt output must be an absolute path without traversal")
if not out.parent.is_dir() or out.parent.is_symlink():
    raise SystemExit("receipt parent must be an existing non-symlink directory")
payload = {
    "schema_version": 2,
    "kind": "browser_vm_deployment_receipt",
    "profile": "browser-vm-chromium",
    "image": "browser-vm-chromium",
    "status": "observed",
    "source": "deploy-image.sh",
    "target_host": sys.argv[2],
    "node_hostname": sys.argv[3],
    "domain_name": sys.argv[4],
    "domain_uuid": sys.argv[5],
    "domain_state": sys.argv[6],
    "remote_image": sys.argv[7],
    "remote_image_format": sys.argv[8],
    "attached_disk": sys.argv[9],
    "attached_disk_format": sys.argv[10],
    "backing_image": sys.argv[11],
    "backing_chain_depth": int(sys.argv[12]),
    "source_commit": sys.argv[13],
    "image_digest": sys.argv[14],
    "remote_image_digest": sys.argv[15],
    "recorded_at": datetime.now(timezone.utc).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ"),
}
tmp = out.with_name(f".{out.name}.tmp")
if tmp.exists() or tmp.is_symlink():
    raise SystemExit(f"receipt temporary output already exists: {tmp}")
old_umask = os.umask(0o077)
try:
    with tmp.open("x", encoding="utf-8") as handle:
        json.dump(payload, handle, sort_keys=True, separators=(",", ":"))
        handle.write("\n")
    os.chmod(tmp, 0o600)
    os.replace(tmp, out)
finally:
    os.umask(old_umask)
    if tmp.exists() or tmp.is_symlink():
        tmp.unlink()
PY
    "$DEPLOYMENT_VERIFY" validate "$RECEIPT" >/dev/null || fail "written deployment receipt did not validate"
    printf 'deployment receipt written\nreceipt=%s\ntarget=%s\nbase=%s\noverlay=%s\ndigest=%s\ndomain=%s\n' \
        "$RECEIPT" "$destination" "$REMOTE_IMAGE" "$attached_disk" "$remote_digest" "$DOMAIN_NAME"
}

receipt_action() {
    local destination probe parsed node_hostname domain_uuid domain_state remote_image
    local remote_format attached_disk attached_format backing_image chain_depth remote_digest
    destination=$(target_label)
    [[ -n "$EXPECTED_DIGEST" ]] || fail "receipt action requires --expected-digest"
    valid_digest "$EXPECTED_DIGEST" || fail "invalid expected digest"
    ssh_args
    probe=$("${SSH_ARGS[@]}" -- "$destination" sudo -n python3 - probe-live \
        --remote-image "$REMOTE_IMAGE" --domain "$DOMAIN_NAME" \
        --expected-digest "$EXPECTED_DIGEST" < "$DEPLOYMENT_VERIFY") || \
        fail "remote deployment receipt probe failed: $destination"
    parsed=$(python3 -c '
import json, re, sys

expected = {
    "node_hostname", "domain_uuid", "domain_state", "remote_image",
    "remote_image_format", "attached_disk", "attached_disk_format",
    "backing_image", "backing_chain_depth", "remote_image_digest",
}
try:
    value = json.load(sys.stdin)
except (UnicodeError, json.JSONDecodeError) as exc:
    raise SystemExit(f"malformed remote probe JSON: {exc}")
if not isinstance(value, dict) or set(value) != expected:
    raise SystemExit("remote probe fields do not match the schema-v2 contract")
ordered = [
    value["node_hostname"], value["domain_uuid"], value["domain_state"],
    value["remote_image"], value["remote_image_format"], value["attached_disk"],
    value["attached_disk_format"], value["backing_image"],
    value["backing_chain_depth"], value["remote_image_digest"],
]
if any(isinstance(item, bool) or not isinstance(item, (str, int)) for item in ordered):
    raise SystemExit("remote probe values have invalid types")
rendered = [str(item) for item in ordered]
if any(re.search(r"[\t\r\n]", item) for item in rendered):
    raise SystemExit("remote probe values contain control separators")
print("\t".join(rendered))
' <<<"$probe") || fail "remote receipt probe returned malformed identity"
    IFS=$'\t' read -r node_hostname domain_uuid domain_state remote_image \
        remote_format attached_disk attached_format backing_image chain_depth \
        remote_digest <<<"$parsed"
    [[ "$remote_image" == "$REMOTE_IMAGE" ]] || fail "remote probe changed the immutable base path"
    [[ -n "$node_hostname" && -n "$domain_uuid" && -n "$domain_state" && \
        -n "$attached_disk" && -n "$backing_image" && -n "$remote_digest" ]] || \
        fail "remote receipt probe returned incomplete identity"
    write_receipt "$node_hostname" "$domain_uuid" "$domain_state" "$attached_disk" \
        "$remote_digest" "$backing_image" "$remote_format" "$attached_format" \
        "$chain_depth"
}

run_action() {
    local action=$1
    shift
    parse_options "$@"
    if [[ "$action" == receipt ]]; then
        receipt_action
        return 0
    fi
    [[ -n "$IMAGE" ]] || fail "--image is required for $action"
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
        set -eu
        cleanup_publish() {
            sudo -n rm -f -- '$remote_tmp' '${REMOTE_IMAGE}.new'
        }
        trap cleanup_publish EXIT
        qemu_gid=\$(getent group qemu | awk -F: 'NR == 1 && \$1 == \"qemu\" && \$3 ~ /^[0-9]+\$/ { print \$3 }')
        test -n \"\$qemu_gid\"
        sudo -n test -f '$remote_tmp'
        sudo -n test ! -L '$remote_tmp'
        sudo -n chown root:root '$remote_tmp'
        sudo -n chmod 0400 '$remote_tmp'
        test \"\$(sudo -n sha256sum -- '$remote_tmp' | awk '{print \"sha256:\" \$1}')\" = '$digest'
        sudo -n qemu-img check --force-share '$remote_tmp'
        if sudo -n test -e '$REMOTE_IMAGE'; then sudo -n cp -a -- '$REMOTE_IMAGE' '$backup'; fi
        sudo -n install -o root -g qemu -m 0440 '$remote_tmp' '${REMOTE_IMAGE}.new'
        sudo -n mv -T -- '${REMOTE_IMAGE}.new' '$REMOTE_IMAGE'
        if sudo -n sh -c 'command -v restorecon >/dev/null 2>&1'; then
            sudo -n restorecon -F -- '$REMOTE_IMAGE'
        fi
        sudo -n test -f '$REMOTE_IMAGE'
        sudo -n test ! -L '$REMOTE_IMAGE'
        test \"\$(sudo -n stat -c '%u:%g:%a' -- '$REMOTE_IMAGE')\" = \"0:\$qemu_gid:440\"
        test \"\$(sudo -n sha256sum -- '$REMOTE_IMAGE' | awk '{print \"sha256:\" \$1}')\" = '$digest'
    "
    echo "published: $destination:$REMOTE_IMAGE (backup=$backup)"
}

self_test() {
    expect_reject() { "$@" && return 1 || return 0; }
    IDENTITY=
    ssh_args
    [[ "${SSH_ARGS[0]}" == ssh ]]
    valid_host 172.20.146.225
    valid_user mm
    valid_path /var/lib/libvirt/images/browser-vm-chromium.qcow2
    expect_reject valid_path /tmp/../escape
    expect_reject valid_path /tmp/./escape
    expect_reject valid_path /tmp//escape
    expect_reject valid_path relative/image.qcow2
    expect_reject valid_path '/tmp/unsafe path'
    expect_reject valid_host 'host;touch /tmp/pwned'
    expect_reject valid_host '[::1]'
    valid_digest sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    expect_reject valid_digest sha256:not-a-digest
    valid_commit 0123456789abcdef0123456789abcdef01234567
    expect_reject valid_commit 0000000000000000000000000000000000000000
    valid_domain browser-vm
    expect_reject valid_domain 'browser vm'
    valid_backing_contract \
        /var/lib/libvirt/images/browser-vm-r1-overlay.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 qcow2 qcow2 1
    expect_reject valid_backing_contract \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 qcow2 qcow2 1
    expect_reject valid_backing_contract \
        /var/lib/libvirt/images/browser-vm-r1-overlay.qcow2 \
        /var/lib/libvirt/images/alternate.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 qcow2 qcow2 1
    expect_reject valid_backing_contract \
        /var/lib/libvirt/images/browser-vm-r1-overlay.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 \
        /var/lib/libvirt/images/browser-vm-chromium.qcow2 qcow2 qcow2 2
    echo 'deploy-image: self-test passed'
}

if (($# == 1)) && [[ "$1" == --self-test ]]; then self_test; exit 0; fi
if (($# == 1)) && [[ "$1" == --help || "$1" == -h ]]; then usage; exit 0; fi
(($# >= 1)) || { usage >&2; exit 2; }
action=$1; shift
case "$action" in
    preflight) run_action preflight "$@" ;;
    publish) run_action publish "$@" ;;
    receipt) run_action receipt "$@" ;;
    *) usage >&2; exit 2 ;;
esac
