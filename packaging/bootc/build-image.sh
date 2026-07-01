#!/usr/bin/env bash
# E12-13 — build the ONE immutable MCNF bootc image (§5: one image, every role).
#
# Wraps `podman build` on packaging/bootc/Containerfile (context = repo root)
# and, optionally, bootc-image-builder for a bootable disk image.
#
# Usage:
#   build-image.sh                         # channel lane: install magic-mesh from the gh-pages dnf repo
#   build-image.sh --rpm <path>            # local lane: bake a locally-built magic-mesh-*.rpm
#   build-image.sh --tag <image:tag>       # default localhost/magic-mesh-bootc:latest
#   build-image.sh --base <bootc-base>     # default quay.io/fedora/fedora-bootc:42
#   build-image.sh --disk <qcow2|raw|anaconda-iso> [--out <dir>]
#                                          # ALSO run bootc-image-builder (needs root podman)
#
# Typed-gated: every missing input is collected and printed before refusing —
# no silent half-runs.
set -euo pipefail

usage() { sed -n '2,17p' "$0"; }

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BOOTC_DIR="$REPO/packaging/bootc"
CONTAINERFILE="$BOOTC_DIR/Containerfile"
RPMS_DIR="$BOOTC_DIR/rpms"

TAG="localhost/magic-mesh-bootc:latest"
BASE=""
LANE="repo"
RPM_PATH=""
DISK_TYPE=""
OUT_DIR="$BOOTC_DIR/out"
# bootc-image-builder — the upstream disk-image builder for bootc images.
BIB_IMAGE="${MCNF_BIB_IMAGE:-quay.io/centos-bootc/bootc-image-builder:latest}"

while [ $# -gt 0 ]; do
    case "$1" in
        --rpm)  RPM_PATH="${2:?--rpm needs a path}"; LANE="local"; shift 2 ;;
        --tag)  TAG="${2:?--tag needs an image:tag}"; shift 2 ;;
        --base) BASE="${2:?--base needs an image ref}"; shift 2 ;;
        --disk) DISK_TYPE="${2:?--disk needs qcow2|raw|anaconda-iso}"; shift 2 ;;
        --out)  OUT_DIR="${2:?--out needs a dir}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "FATAL: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# ── Pre-flight: collect EVERY missing input, then refuse in one shot ─────────
missing=()
command -v podman >/dev/null 2>&1 || missing+=("podman is not installed / not on PATH")
[ -f "$CONTAINERFILE" ]           || missing+=("Containerfile missing: $CONTAINERFILE")
[ -d "$RPMS_DIR" ]                || missing+=("staging dir missing: $RPMS_DIR")
[ -f "$REPO/packaging/repo/magic-mesh.repo" ] || missing+=("channel repo file missing: $REPO/packaging/repo/magic-mesh.repo")

if [ "$LANE" = "local" ]; then
    if [ ! -f "$RPM_PATH" ]; then
        missing+=("--rpm path does not exist: $RPM_PATH")
    else
        case "$(basename "$RPM_PATH")" in
            magic-mesh-*.rpm) : ;;
            *) missing+=("--rpm must be a magic-mesh-*.rpm (got: $(basename "$RPM_PATH"))") ;;
        esac
    fi
fi

if [ -n "$DISK_TYPE" ]; then
    case "$DISK_TYPE" in
        qcow2|raw|anaconda-iso) : ;;
        *) missing+=("--disk must be qcow2|raw|anaconda-iso (got: $DISK_TYPE)") ;;
    esac
    # bootc-image-builder reads the just-built image from ROOT podman storage
    # and needs a privileged container.
    [ "$(id -u)" -eq 0 ] || missing+=("--disk requires root (bootc-image-builder needs privileged root podman)")
fi

if [ ${#missing[@]} -gt 0 ]; then
    echo "REFUSING to run — missing inputs:" >&2
    for m in "${missing[@]}"; do echo "  - $m" >&2; done
    exit 2
fi

# ── Stage the local RPM (local lane) ─────────────────────────────────────────
# Clean stale staged RPMs first so the Containerfile's glob can never pick up
# an old build; keep .gitkeep so the repo-lane COPY still has a dir to copy.
find "$RPMS_DIR" -maxdepth 1 -name '*.rpm' -delete
if [ "$LANE" = "local" ]; then
    cp -v "$RPM_PATH" "$RPMS_DIR/"
fi

# ── Build the bootc image ─────────────────────────────────────────────────────
build_args=(--build-arg "MCNF_RPM_LANE=$LANE")
[ -n "$BASE" ] && build_args+=(--build-arg "BOOTC_BASE=$BASE")

echo "==> bootc image build: lane=$LANE tag=$TAG base=${BASE:-<Containerfile default>}"
podman build "${build_args[@]}" -t "$TAG" -f "$CONTAINERFILE" "$REPO"
echo "==> built: $TAG"

# ── Optional: a bootable disk image via bootc-image-builder ──────────────────
if [ -n "$DISK_TYPE" ]; then
    mkdir -p "$OUT_DIR"
    echo "==> bootc-image-builder: type=$DISK_TYPE out=$OUT_DIR"
    podman run --rm --privileged \
        --security-opt label=type:unconfined_t \
        -v "$OUT_DIR:/output" \
        -v /var/lib/containers/storage:/var/lib/containers/storage \
        "$BIB_IMAGE" \
        --type "$DISK_TYPE" --local "$TAG"
    echo "==> disk image(s) under: $OUT_DIR"
fi
