#!/usr/bin/env bash
# WL-FUNC-018 — build the immutable App VM guest profile.
#
# Exit codes:
#   0  image (and disk, if requested) built and statically verified
#   2  REFUSED — invalid or missing inputs
#   3  GATED — the requested base or image-builder container is unavailable
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
APP_VM_DIR="$REPO/packaging/app-vm"
CONTAINERFILE="$APP_VM_DIR/Containerfile"
RPMS_DIR="$APP_VM_DIR/rpms"
IMAGE="localhost/magic-mesh-app-vm-wayland:latest"
BASE=""
LANE="repo"
RPMS=()
DISK=""
OUT="$APP_VM_DIR/out"
BIB_IMAGE="${MCNF_BIB_IMAGE:-quay.io/centos-bootc/bootc-image-builder:latest}"
PULL_TIMEOUT="${MCNF_PULL_TIMEOUT:-120}"

SOURCE_COMMIT="${MCNF_APP_VM_SOURCE_COMMIT:-$(git -C "$REPO" rev-parse --verify HEAD 2>/dev/null || true)}"

if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || [[ "$SOURCE_COMMIT" == 0000000000000000000000000000000000000000 ]]; then
    echo "FATAL: App VM source provenance is not a non-null 40-character Git revision" >&2
    exit 2
fi

usage() {
    cat <<'EOF'
Usage: packaging/app-vm/build-image.sh [--rpm PATH]... [--base IMAGE]
       [--tag IMAGE] [--disk qcow2|raw|anaconda-iso] [--out DIR]

Builds the fixed wayland-standard App VM guest image. A disk output uses
bootc-image-builder and is suitable for the app_base_image_source OpenTofu
variable. The curated Flatpak remote is provisioned separately and is never
selected from catalog data.

The build is fail-closed: a missing base image is pulled once with a bounded
timeout, and an unreachable registry exits 3 before podman build starts. A
successful image build is always passed through verify-image.sh before a disk
artifact is emitted. The source revision is recorded in both the image label
and the guest-readable provenance file.
EOF
}

resolve_image() { # $1 = image ref, $2 = human-readable purpose
    local ref="$1" label="$2" err rc=0
    if podman image exists "$ref"; then
        echo "==> $label image already in local storage (offline OK): $ref"
        return 0
    fi

    local -a pull=(podman pull "$ref")
    command -v timeout >/dev/null 2>&1 && pull=(timeout "$PULL_TIMEOUT" "${pull[@]}")
    echo "==> pulling $label image: $ref (bounded: ${PULL_TIMEOUT}s)"
    err="$("${pull[@]}" 2>&1 >/dev/null)" || rc=$?
    [ "$rc" -eq 0 ] && return 0
    if [ "$rc" -eq 124 ] || grep -Eqi \
        'no such host|dial tcp|i/o timeout|timed out|connection refused|network is unreachable|no route to host|tls handshake|proxyconnect|temporary failure in name resolution' \
        <<<"$err"; then
        {
            echo "GATED[WL-FUNC-018/base-image]: registry unavailable for $label"
            echo "  ref: $ref"
            printf '%s\n' "$err" | tail -n 3 | sed 's/^/  podman: /'
            echo "Side-load the image with 'podman load -i <image.tar>' or use a farm lane with registry egress."
        } >&2
        exit 3
    fi
    echo "FATAL: podman pull failed for $label image $ref (not network-shaped):" >&2
    printf '%s\n' "$err" | tail -n 5 | sed 's/^/  podman: /' >&2
    exit 2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --rpm) RPMS+=("${2:?--rpm needs a path}"); LANE=local; shift 2 ;;
        --base) BASE="${2:?--base needs an image}"; shift 2 ;;
        --tag) IMAGE="${2:?--tag needs an image}"; shift 2 ;;
        --disk) DISK="${2:?--disk needs a type}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "FATAL: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

missing=()
command -v podman >/dev/null 2>&1 || missing+=("podman is required")
[ -f "$CONTAINERFILE" ] || missing+=("Containerfile missing: $CONTAINERFILE")
[ -d "$RPMS_DIR" ] || missing+=("RPM staging directory missing: $RPMS_DIR")
[ -f "$REPO/packaging/repo/magic-mesh.repo" ] || missing+=("channel repo file missing: $REPO/packaging/repo/magic-mesh.repo")

if [ "$LANE" = "local" ]; then
    for rpm in "${RPMS[@]}"; do
        [ -f "$rpm" ] || missing+=("--rpm path does not exist: $rpm")
        case "$(basename "$rpm")" in
            magic-mesh-*.rpm) ;;
            *) missing+=("--rpm must be a magic-mesh-*.rpm (got: $(basename "$rpm"))") ;;
        esac
    done
fi

[ -n "$OUT" ] && [ -z "$DISK" ] && [ "$OUT" != "$APP_VM_DIR/out" ] && \
    missing+=("--out only applies to --disk; add --disk or omit --out")

if [ "${#missing[@]}" -gt 0 ]; then
    echo "REFUSING to run — missing inputs:" >&2
    for item in "${missing[@]}"; do echo "  - $item" >&2; done
    exit 2
fi

if [ -n "$DISK" ] && [ "$(id -u)" -ne 0 ]; then
    echo "FATAL: --disk requires root for bootc-image-builder" >&2
    exit 2
fi
case "$DISK" in
    ""|qcow2|raw|anaconda-iso) ;;
    *) echo "FATAL: invalid --disk" >&2; exit 2 ;;
esac

find "$RPMS_DIR" -maxdepth 1 -type f -name '*.rpm' -delete
for rpm in "${RPMS[@]}"; do
    cp -v "$rpm" "$RPMS_DIR/"
done

EFFECTIVE_BASE="${BASE:-$(sed -n 's/^ARG APP_VM_BASE=//p' "$CONTAINERFILE" | head -n 1)}"
[ -n "$EFFECTIVE_BASE" ] || {
    echo "FATAL: cannot determine App VM base (no --base and no ARG APP_VM_BASE=)" >&2
    exit 2
}
resolve_image "$EFFECTIVE_BASE" "App VM base"

# Capture the resolved base identity before the build. A mutable tag is useful
# for selecting a farm cache, but it is not sufficient provenance for an image
# that may be admitted as a VM root. The immutable image ID is carried as a
# label and checked again before any disk conversion.
BASE_ID_RAW="$(podman image inspect --format '{{.Id}}' "$EFFECTIVE_BASE" 2>/dev/null || true)"
if [[ "$BASE_ID_RAW" == sha256:* ]]; then
    BASE_ID="$BASE_ID_RAW"
elif [[ "$BASE_ID_RAW" =~ ^[0-9a-fA-F]{64}$ ]]; then
    # Rootless Podman on some farm images omits the algorithm prefix. Restore
    # the canonical OCI form before carrying the identity into provenance.
    BASE_ID="sha256:$BASE_ID_RAW"
else
    echo "FATAL: resolved App VM base has no immutable image ID: $EFFECTIVE_BASE" >&2
    exit 2
fi

CONTRACT_ID="wayland-standard-v1"

args=(
    --build-arg "MCNF_RPM_LANE=$LANE"
    --build-arg "MCNF_APP_VM_SOURCE_COMMIT=$SOURCE_COMMIT"
    --build-arg "MCNF_APP_VM_BASE_IMAGE_ID=$BASE_ID"
)
[ -n "$BASE" ] && args+=(--build-arg "APP_VM_BASE=$BASE")
podman build "${args[@]}" \
    --label "org.mcnf.app-vm.profile=$CONTRACT_ID" \
    --label "org.mcnf.app-vm.base-image=$EFFECTIVE_BASE" \
    --label "org.mcnf.app-vm.base-image-id=$BASE_ID" \
    --label "org.mcnf.app-vm.source-commit=$SOURCE_COMMIT" \
    -t "$IMAGE" \
    --ignorefile "$APP_VM_DIR/context.containerignore" \
    -f "$CONTAINERFILE" \
    "$REPO"

# Inspect the built image before producing a disk artifact. This is a contents
# gate, not a boot claim: the image must contain the fixed guest contract and
# must not silently acquire a public Flatpak remote.
"$REPO/packaging/app-vm/verify-image.sh" "$IMAGE"

IMAGE_ID="$(podman image inspect --format '{{.Id}}' "$IMAGE" 2>/dev/null || true)"
IMAGE_PROFILE="$(podman image inspect --format '{{index .Config.Labels \"org.mcnf.app-vm.profile\"}}' "$IMAGE" 2>/dev/null || true)"
IMAGE_BASE_ID="$(podman image inspect --format '{{index .Config.Labels \"org.mcnf.app-vm.base-image-id\"}}' "$IMAGE" 2>/dev/null || true)"
IMAGE_SOURCE_COMMIT="$(podman image inspect --format '{{index .Config.Labels \"org.mcnf.app-vm.source-commit\"}}' "$IMAGE" 2>/dev/null || true)"
if [ -z "$IMAGE_ID" ] || [ "$IMAGE_PROFILE" != "$CONTRACT_ID" ] || \
   [ "$IMAGE_BASE_ID" != "$BASE_ID" ] || [ "$IMAGE_SOURCE_COMMIT" != "$SOURCE_COMMIT" ]; then
    echo "FATAL: built App VM image failed immutable provenance verification" >&2
    echo "  image_id=$IMAGE_ID profile=$IMAGE_PROFILE base_id=$IMAGE_BASE_ID expected_base_id=$BASE_ID" >&2
    echo "  source_commit=$IMAGE_SOURCE_COMMIT expected_source_commit=$SOURCE_COMMIT" >&2
    exit 2
fi
echo "==> App VM image verified: id=$IMAGE_ID base_id=$BASE_ID source_commit=$SOURCE_COMMIT profile=$CONTRACT_ID"

if [ -n "$DISK" ]; then
    resolve_image "$BIB_IMAGE" "bootc-image-builder"
    mkdir -p "$OUT"
    podman run --rm --privileged \
        --security-opt label=type:unconfined_t \
        -v "$OUT:/output" \
        -v /var/lib/containers/storage:/var/lib/containers/storage \
        "$BIB_IMAGE" \
        --type "$DISK" --local "$IMAGE"
fi
