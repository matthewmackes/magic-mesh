#!/usr/bin/env bash
# E12-13 — build the ONE immutable MCNF bootc image (§5: one image, every role).
#
# Wraps `podman build` on packaging/bootc/Containerfile (context = repo root)
# and, optionally, bootc-image-builder for a bootable disk image.
#
# Usage:
#   build-image.sh                         # channel lane: install magic-mesh from the gh-pages dnf repo
#   build-image.sh --rpm <path> [--rpm <path>...]   # local lane: bake locally-built
#                                          # magic-mesh-*.rpm(s); repeat --rpm to stage
#                                          # the base + browser pair into one seat image
#   build-image.sh --tag <image:tag>       # default localhost/magic-mesh-bootc:latest
#   build-image.sh --base <bootc-base>     # must equal manifest's digest-pinned Fedora 44 base
#   build-image.sh --disk <qcow2|raw|anaconda-iso> [--out <dir>]
#                                          # ALSO run bootc-image-builder (needs root podman)
#
# Typed-gated: every missing input is collected and printed before refusing —
# no silent half-runs, and never a raw podman splat for the expected airgap case.
#
# Exit codes:
#   0  image (and disk, if requested) built
#   2  REFUSED — bad/missing inputs (author error; itemized list on stderr)
#   3  GATED[E12-13/base-image] — the registry is unreachable from this host.
#      On the airgap-ish farm this is the EXPECTED outcome, not a bug: gain
#      egress or side-load the base once (`podman load`) and re-run.
#      (MCNF_PULL_TIMEOUT=<secs> bounds the probe; default 120.)
#      Also used for a valid but explicitly blocked Surface provenance manifest.
set -euo pipefail

# Print the header comment block (line 2 → first non-comment) so usage never
# drifts from the doc when the header grows.
usage() { awk 'NR>1 && !/^#/{exit} NR>1{sub(/^# ?/,""); print}' "$0"; }

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BOOTC_DIR="$REPO/packaging/bootc"
CONTAINERFILE="$BOOTC_DIR/Containerfile"
RPMS_DIR="$BOOTC_DIR/rpms"
SURFACE_VERIFY="$REPO/install-helpers/verify-surface-stack.sh"
SURFACE_MANIFEST="$REPO/packaging/surface/surface-stack.f44.json"
SURFACE_ARTIFACTS="$REPO/packaging/surface/artifacts"
SURFACE_LOCK="$REPO/packaging/surface/.surface-install.lock"

TAG="localhost/magic-mesh-bootc:latest"
BASE=""
LANE="repo"
RPM_PATHS=()
DISK_TYPE=""
OUT_DIR="$BOOTC_DIR/out"
OUT_DIR_SET=""
# bootc-image-builder — the upstream disk-image builder for bootc images.
BIB_IMAGE="${MCNF_BIB_IMAGE:-quay.io/centos-bootc/bootc-image-builder:latest}"
PULL_TIMEOUT="${MCNF_PULL_TIMEOUT:-120}"

# ── The E12-13 typed base-image gate ──────────────────────────────────────────
# Resolve an image ref before we need it: already-in-local-storage wins (THE
# airgap side-load path — podman build's default `missing` pull policy then
# never touches the network), otherwise ONE bounded pull. A network-shaped
# failure exits 3 with a GATED[...] block instead of a raw podman error halfway
# through the build; a non-network pull failure (bad ref/tag) is an author
# error and refuses with rc 2.
resolve_image() { # $1 = image ref, $2 = what it is (for the message)
    local ref="$1" label="$2" err rc=0
    if podman image exists "$ref"; then
        echo "==> $label image already in local storage (offline OK): $ref"
        return 0
    fi
    local -a pull=(podman pull "$ref")
    command -v timeout >/dev/null 2>&1 && pull=(timeout "$PULL_TIMEOUT" "${pull[@]}")
    echo "==> pulling $label image: $ref (bounded: ${PULL_TIMEOUT}s)"
    err=$("${pull[@]}" 2>&1 >/dev/null) || rc=$?
    [ "$rc" -eq 0 ] && return 0
    if [ "$rc" -eq 124 ] || grep -Eqi \
        'no such host|dial tcp|i/o timeout|timed out|connection refused|network is unreachable|no route to host|tls handshake|proxyconnect|temporary failure in name resolution' \
        <<<"$err"; then
        {
            echo "GATED[E12-13/base-image]: registry unreachable for the $label image"
            echo "  ref: $ref"
            printf '%s\n' "$err" | tail -n 3 | sed 's/^/  podman: /'
            echo "This farm is airgap-ish — an unreachable registry is the EXPECTED gated"
            echo "outcome here, not a build bug. Unblock either way:"
            echo "  1. run on a host with container-registry egress, or"
            echo "  2. side-load the $label image once: podman load -i <image.tar>"
            echo "     (an image already in local storage skips this probe entirely)"
        } >&2
        exit 3
    fi
    echo "FATAL: podman pull failed for the $label image $ref (not network-shaped — check the ref/tag):" >&2
    printf '%s\n' "$err" | tail -n 5 | sed 's/^/  podman: /' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --rpm)  RPM_PATHS+=("${2:?--rpm needs a path}"); LANE="local"; shift 2 ;;
        --tag)  TAG="${2:?--tag needs an image:tag}"; shift 2 ;;
        --base) BASE="${2:?--base needs an image ref}"; shift 2 ;;
        --disk) DISK_TYPE="${2:?--disk needs qcow2|raw|anaconda-iso}"; shift 2 ;;
        --out)  OUT_DIR="${2:?--out needs a dir}"; OUT_DIR_SET=1; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "FATAL: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# Surface admission is deliberately first. The committed blocked manifest must
# return the typed provenance gate before checking, pulling, or invoking Podman.
[ -x "$SURFACE_VERIFY" ] || { echo "REFUSED[WL-UX-011/surface-provenance]: verifier missing/not executable: $SURFACE_VERIFY" >&2; exit 2; }
[ -f "$SURFACE_MANIFEST" ] || { echo "REFUSED[WL-UX-011/surface-provenance]: manifest missing: $SURFACE_MANIFEST" >&2; exit 2; }
surface_rc=0
rm -f "$SURFACE_LOCK"
trap 'rm -f "$SURFACE_LOCK"' EXIT
"$SURFACE_VERIFY" --manifest "$SURFACE_MANIFEST" --artifact-dir "$SURFACE_ARTIFACTS" --emit-lock "$SURFACE_LOCK" || surface_rc=$?
case "$surface_rc" in
    0) ;;
    3)
        echo "GATED[WL-UX-011/surface-provenance]: Fedora 44 Surface stack provenance is unavailable" >&2
        exit 3
        ;;
    *)
        echo "REFUSED[WL-UX-011/surface-provenance]: invalid Surface stack provenance (verifier rc=$surface_rc)" >&2
        exit 2
        ;;
esac

# ── Pre-flight: collect EVERY missing input, then refuse in one shot ─────────
missing=()
command -v podman >/dev/null 2>&1 || missing+=("podman is not installed / not on PATH")
[ -f "$CONTAINERFILE" ]           || missing+=("Containerfile missing: $CONTAINERFILE")
[ -d "$RPMS_DIR" ]                || missing+=("staging dir missing: $RPMS_DIR")
[ -x "$SURFACE_VERIFY" ]          || missing+=("Surface provenance verifier missing/not executable: $SURFACE_VERIFY")
[ -f "$SURFACE_MANIFEST" ]        || missing+=("Surface provenance manifest missing: $SURFACE_MANIFEST")
[ -d "$SURFACE_ARTIFACTS" ]       || missing+=("Surface artifact directory missing: $SURFACE_ARTIFACTS")
[ -f "$REPO/packaging/repo/magic-mesh.repo" ] || missing+=("channel repo file missing: $REPO/packaging/repo/magic-mesh.repo")

if [ "$LANE" = "local" ]; then
    for _rpm in "${RPM_PATHS[@]}"; do
        if [ ! -f "$_rpm" ]; then
            missing+=("--rpm path does not exist: $_rpm")
        else
            case "$(basename "$_rpm")" in
                magic-mesh-*.rpm) : ;;
                *) missing+=("--rpm must be a magic-mesh-*.rpm (got: $(basename "$_rpm"))") ;;
            esac
        fi
    done
fi

[ -n "$OUT_DIR_SET" ] && [ -z "$DISK_TYPE" ] && \
    missing+=("--out only applies to the --disk lane (an image build writes no files there) — add --disk or drop --out")

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
    cp -v "${RPM_PATHS[@]}" "$RPMS_DIR/"
fi

# ── The base-image gate, THEN the build ──────────────────────────────────────
# Effective base = --base, else the Containerfile's ARG default (single source
# of truth — never restate the quay ref here).
LOCKED_BASE="$(awk -F '\t' '$1 == "BASE" { print $2 }' "$SURFACE_LOCK")"
EFFECTIVE_BASE="${BASE:-$LOCKED_BASE}"
if [ -z "$LOCKED_BASE" ] || [ "$EFFECTIVE_BASE" != "$LOCKED_BASE" ]; then
    echo "REFUSED[WL-UX-011/surface-base]: --base must exactly equal the manifest-authorized digest-pinned Fedora 44 base" >&2
    echo "  authorized: $LOCKED_BASE" >&2
    echo "  requested:  $EFFECTIVE_BASE" >&2
    exit 2
fi
resolve_image "$EFFECTIVE_BASE" "bootc base"

MANIFEST_SHA256="$(awk -F '\t' '$1 == "MANIFEST" { print $2 }' "$SURFACE_LOCK")"
LOCK_SHA256="$(sha256sum "$SURFACE_LOCK" | awk '{print $1}')"
build_args=(--build-arg "MCNF_RPM_LANE=$LANE" --build-arg "BOOTC_BASE=$EFFECTIVE_BASE"
    --build-arg "SURFACE_MANIFEST_SHA256=$MANIFEST_SHA256" --build-arg "SURFACE_LOCK_SHA256=$LOCK_SHA256")

echo "==> bootc image build: lane=$LANE tag=$TAG base=$EFFECTIVE_BASE"
# --ignorefile: context is the repo root but only packaging/ is COPYied —
# the allowlist keeps crates/.git/docs out of the context upload.
podman build "${build_args[@]}" -t "$TAG" \
    --ignorefile "$BOOTC_DIR/context.containerignore" \
    -f "$CONTAINERFILE" "$REPO"
echo "==> built: $TAG"

# ── Optional: a bootable disk image via bootc-image-builder ──────────────────
if [ -n "$DISK_TYPE" ]; then
    resolve_image "$BIB_IMAGE" "bootc-image-builder"
    case "$OUT_DIR" in
        /*) OUT_DIR_ABS="$OUT_DIR" ;;
        *) OUT_DIR_ABS="$PWD/$OUT_DIR" ;;
    esac
    mkdir -p "$OUT_DIR_ABS"
    echo "==> bootc-image-builder: type=$DISK_TYPE out=$OUT_DIR_ABS"
    podman run --rm --privileged \
        --security-opt label=type:unconfined_t \
        -v "$OUT_DIR_ABS:/output" \
        -v /var/lib/containers/storage:/var/lib/containers/storage \
        "$BIB_IMAGE" \
        --type "$DISK_TYPE" --local "$TAG"
    echo "==> disk image(s) under: $OUT_DIR_ABS"
fi
