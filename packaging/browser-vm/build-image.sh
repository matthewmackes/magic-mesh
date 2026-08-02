#!/usr/bin/env bash
# WL-ARCH-008 — build and statically verify the immutable Chromium Browser VM.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="$REPO/packaging/browser-vm"
IMAGE="localhost/magic-mesh-browser-vm-chromium:latest"
BASE=""
LANE=repo
RPMS=()
DISK=""
OUT="$DIR/out"
BIB_IMAGE="${MCNF_BIB_IMAGE:-quay.io/centos-bootc/bootc-image-builder@sha256:2b52843ea2bfda73b0a08d97e76b734393b1d3a804681b9fabb26723bd3a2f0b}"
# ext4 is the portable bootc-image-builder rootfs across the Fedora 44 farm
# and Rocky orchestration host; the XFS lane fails in osbuild's loop mount
# stage on the current builder image.
BROWSER_VM_ROOTFS="${MCNF_BROWSER_VM_ROOTFS:-ext4}"
PULL_TIMEOUT="${MCNF_PULL_TIMEOUT:-120}"

usage() { echo "Usage: $0 [--rpm PATH]... [--base IMAGE] [--tag IMAGE] [--disk qcow2|raw|anaconda-iso] [--out DIR]"; }

resolve_image() {
    local ref=$1 label=$2 err rc=0
    podman image exists "$ref" && return 0
    err="$(timeout "$PULL_TIMEOUT" podman pull "$ref" 2>&1 >/dev/null)" || rc=$?
    if [ "$rc" -eq 124 ] || grep -Eqi 'no such host|timed out|connection refused|network is unreachable|temporary failure' <<<"$err"; then
        echo "GATED[WL-ARCH-008/base-image]: registry unavailable for $label ($ref)" >&2
        exit 3
    fi
    [ "$rc" -eq 0 ] || { echo "FATAL: cannot pull $label: $err" >&2; exit 2; }
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --rpm) RPMS+=("${2:?--rpm needs a path}"); LANE=local; shift 2 ;;
        --base) BASE="${2:?--base needs an image}"; shift 2 ;;
        --tag) IMAGE="${2:?--tag needs an image}"; shift 2 ;;
        --disk) DISK="${2:?--disk needs a type}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "FATAL: unknown argument: $1" >&2; exit 2 ;;
    esac
done

command -v podman >/dev/null 2>&1 || { echo 'FATAL: podman is required' >&2; exit 2; }
[ -f "$DIR/Containerfile" ] || { echo 'FATAL: Browser VM Containerfile is missing' >&2; exit 2; }
mkdir -p "$DIR/rpms"
find "$DIR/rpms" -maxdepth 1 -type f -name '*.rpm' -delete
for rpm in "${RPMS[@]}"; do cp "$rpm" "$DIR/rpms/"; done

effective_base="${BASE:-$(sed -n 's/^ARG BROWSER_VM_BASE=//p' "$DIR/Containerfile" | head -n1)}"
resolve_image "$effective_base" 'Browser VM base image'
base_id="$(podman image inspect --format '{{.Digest}}' "$effective_base")"
if [[ ! "$base_id" =~ ^sha256:[0-9a-fA-F]{64}$ ]]; then
    base_id="$(podman image inspect --format '{{.Id}}' "$effective_base")"
    [[ "$base_id" =~ ^[0-9a-fA-F]{64}$ ]] && base_id="sha256:$base_id"
fi
[[ "$base_id" =~ ^sha256:[0-9a-fA-F]{64}$ ]] || { echo 'FATAL: base image has no immutable digest' >&2; exit 2; }

args=(--build-arg "MCNF_RPM_LANE=$LANE")
[ -n "$BASE" ] && args+=(--build-arg "BROWSER_VM_BASE=$BASE")
podman build "${args[@]}" \
    --label 'org.mcnf.browser-vm.profile=browser-vm-chromium-v1' \
    --label "org.mcnf.browser-vm.base-image-id=$base_id" \
    -t "$IMAGE" --ignorefile "$DIR/context.containerignore" -f "$DIR/Containerfile" "$REPO"

"$DIR/verify-image.sh" "$IMAGE"
if [ -n "$DISK" ]; then
    [ "$(id -u)" -eq 0 ] || { echo 'FATAL: --disk requires root' >&2; exit 2; }
    resolve_image "$BIB_IMAGE" bootc-image-builder
    mkdir -p "$OUT"
    podman run --rm --privileged --security-opt label=type:unconfined_t \
        -v "$OUT:/output" -v /var/lib/containers/storage:/var/lib/containers/storage \
        "$BIB_IMAGE" --rootfs "$BROWSER_VM_ROOTFS" --type "$DISK" --local "$IMAGE"
fi
