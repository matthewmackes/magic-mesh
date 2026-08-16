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
RPM_SUPPLY_VERIFY="$APP_VM_DIR/verify-rpm-supply.sh"
RPM_KEY="$REPO/packaging/repo/RPM-GPG-KEY-magic-mesh"
BASE_RECEIPT_VERIFY="$APP_VM_DIR/produce-base-image-receipt.py"
IMAGE="localhost/magic-mesh-app-vm-wayland:latest"
BASE=""
BASE_RECEIPT=""
LANE="repo"
RPMS=()
CANDIDATE_MANIFEST=""
DISK=""
OUT="$APP_VM_DIR/out"
REUSE_IMAGE=""
BIB_IMAGE="${MCNF_BIB_IMAGE:-quay.io/centos-bootc/bootc-image-builder:latest}"
APP_VM_ROOTFS="${MCNF_APP_VM_ROOTFS:-ext4}"
PULL_TIMEOUT="${MCNF_PULL_TIMEOUT:-120}"

canonical_image_id() { # $1 = raw Podman image ID
    local raw="$1"
    if [[ "$raw" == sha256:* ]]; then
        [[ "$raw" =~ ^sha256:[0-9a-fA-F]{64}$ ]] || return 1
        printf '%s\n' "$raw"
    elif [[ "$raw" =~ ^[0-9a-fA-F]{64}$ ]]; then
        printf 'sha256:%s\n' "$raw"
    else
        return 1
    fi
}

append_pinned_base_arg() { # $1 = selected ref, $2 = digest, $3 = optional pinned registry ref
    local selected_ref="$1" immutable_id="$2" pinned_ref="${3:-$2}"
    [[ -n "$selected_ref" ]] || {
        echo "FATAL: cannot pin an empty App VM base reference" >&2
        return 2
    }
    canonical_image_id "$immutable_id" >/dev/null || {
        echo "FATAL: cannot pin App VM build to invalid base image ID: $immutable_id" >&2
        return 2
    }
    if [[ "$pinned_ref" != "$immutable_id" && "$pinned_ref" != *"@$immutable_id" ]]; then
        echo "FATAL: pinned App VM base reference does not carry admitted digest" >&2
        return 2
    fi
    args+=(--build-arg "APP_VM_BASE=$pinned_ref")
}

self_test() {
    # Hostile case: a mutable selected tag is assumed to be retargeted after
    # inspection. It must never cross the Containerfile FROM boundary.
    local selected_ref='registry.example.invalid/app-vm-base:latest'
    local captured_id='sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    local rendered
    args=()
    append_pinned_base_arg "$selected_ref" "$captured_id"
    rendered=" ${args[*]} "
    [[ "$rendered" == *" APP_VM_BASE=$captured_id "* ]] || {
        echo "self-test: immutable base ID was not passed to the build" >&2
        return 1
    }
    [[ "$rendered" != *"$selected_ref"* ]] || {
        echo "self-test: mutable base tag escaped into the build" >&2
        return 1
    }
    echo "build-image.sh: self-test passed"
}

if [[ "${1:-}" == --self-test ]]; then
    [[ "$#" -eq 1 ]] || {
        echo "FATAL: --self-test takes no other arguments" >&2
        exit 2
    }
    self_test
    exit
fi

SOURCE_COMMIT="${MCNF_APP_VM_SOURCE_COMMIT:-$(git -C "$REPO" rev-parse --verify HEAD 2>/dev/null || true)}"

if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]+$ ]] || [ "${#SOURCE_COMMIT}" -ne 40 ] || \
   [[ "$SOURCE_COMMIT" == 0000000000000000000000000000000000000000 ]]; then
    echo "FATAL: App VM source provenance is not a non-null 40-character Git revision" >&2
    exit 2
fi

usage() {
    cat <<'EOF'
Usage: packaging/app-vm/build-image.sh --base-receipt PATH
       [--rpm PATH --candidate-manifest PATH]
       [--base IMAGE]
       [--tag IMAGE] [--reuse-image IMAGE_ID]
       [--disk qcow2|raw|anaconda-iso] [--out DIR]
       packaging/app-vm/build-image.sh --self-test

Builds the fixed wayland-standard App VM guest image. A disk output uses
bootc-image-builder and is suitable for the app_base_image_source OpenTofu
variable. The curated Flatpak remote is provisioned separately and is never
selected from catalog data.

The build is fail-closed: a missing base image is pulled once with a bounded
timeout, and an unreachable registry exits 3 before podman build starts. A
successful image build is always passed through verify-image.sh before a disk
artifact is emitted. The source revision is recorded in both the image label
and the guest-readable provenance file. The local lane checks the candidate
manifest against authoritative RPM NEVRA/payload metadata, then authenticates
the requested revision against the signed binaries' compile-time build identity
before and after staging and inside the Containerfile.
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
        --candidate-manifest) CANDIDATE_MANIFEST="${2:?--candidate-manifest needs a path}"; shift 2 ;;
        --base-receipt) BASE_RECEIPT="${2:?--base-receipt needs a path}"; shift 2 ;;
        --base) BASE="${2:?--base needs an image}"; shift 2 ;;
        --tag) IMAGE="${2:?--tag needs an image}"; shift 2 ;;
        --reuse-image) REUSE_IMAGE="${2:?--reuse-image needs an image ID}"; shift 2 ;;
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
[ -x "$RPM_SUPPLY_VERIFY" ] || missing+=("RPM supply verifier missing or not executable: $RPM_SUPPLY_VERIFY")
[ -f "$RPM_KEY" ] || missing+=("governed RPM key missing: $RPM_KEY")
[ -x "$BASE_RECEIPT_VERIFY" ] || missing+=("base-image receipt verifier missing or not executable: $BASE_RECEIPT_VERIFY")
[ -n "$BASE_RECEIPT" ] || missing+=("--base-receipt is required")
[ -z "$BASE_RECEIPT" ] || { [ -f "$BASE_RECEIPT" ] && [ ! -L "$BASE_RECEIPT" ]; } \
    || missing+=("base-image receipt is not a regular non-symlink file")

if [ "$LANE" = "local" ]; then
    [ "${#RPMS[@]}" -eq 1 ] || missing+=("local lane requires exactly one magic-mesh RPM")
    [ -n "$CANDIDATE_MANIFEST" ] || missing+=("local lane requires --candidate-manifest")
    [ -z "$CANDIDATE_MANIFEST" ] || [ -f "$CANDIDATE_MANIFEST" ] \
        || missing+=("--candidate-manifest path does not exist: $CANDIDATE_MANIFEST")
    for rpm in "${RPMS[@]}"; do
        [ -f "$rpm" ] || missing+=("--rpm path does not exist: $rpm")
    done
elif [ -n "$CANDIDATE_MANIFEST" ]; then
    missing+=("--candidate-manifest is only valid with the local --rpm lane")
fi

[ -n "$OUT" ] && [ -z "$DISK" ] && [ "$OUT" != "$APP_VM_DIR/out" ] && \
    missing+=("--out only applies to --disk; add --disk or omit --out")

if [ -n "$REUSE_IMAGE" ]; then
    canonical_image_id "$REUSE_IMAGE" >/dev/null \
        || missing+=("--reuse-image must be one complete sha256 image ID")
    IMAGE="$REUSE_IMAGE"
fi

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
if [ -n "$DISK" ]; then
    case "$APP_VM_ROOTFS" in
        ext4|xfs|btrfs) ;;
        *) echo "FATAL: invalid App VM rootfs: $APP_VM_ROOTFS" >&2; exit 2 ;;
    esac
fi

# Re-attest the registry bytes and all release bindings before RPM staging,
# Podman storage, or the image build context can be mutated.  The receipt
# inspector performs a bounded manifest-only registry read; it never pulls a
# layer or handles registry credentials.
EFFECTIVE_BASE="${BASE:-$(sed -n 's/^ARG APP_VM_BASE=//p' "$CONTAINERFILE" | head -n 1)}"
[ -n "$EFFECTIVE_BASE" ] || {
    echo "FATAL: cannot determine App VM base (no --base and no ARG APP_VM_BASE=)" >&2
    exit 2
}
case "$(uname -m)" in
    x86_64) REGISTRY_ARCH=amd64 ;;
    aarch64) REGISTRY_ARCH=arm64 ;;
    *) echo "FATAL: unsupported App VM build architecture" >&2; exit 2 ;;
esac
COMMIT_EPOCH="$(git -C "$REPO" show -s --format=%ct "$SOURCE_COMMIT")"
if [ -n "$REUSE_IMAGE" ]; then
    # A verified local image checkpoint must be restartable while the registry
    # is unavailable. Revalidate the already-admitted receipt offline; fresh
    # builds still perform the live manifest re-attestation below.
    BASE_RECEIPT_JSON="$(python3 - "$BASE_RECEIPT" "$EFFECTIVE_BASE" "$REGISTRY_ARCH" "$SOURCE_COMMIT" "$COMMIT_EPOCH" <<'PY'
import json, pathlib, re, sys
path, reference, architecture, revision, epoch = sys.argv[1:]
value = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
expected = {
    "kind": "mcnf-app-vm-base-image-receipt",
    "schema_version": 1,
    "image_reference": reference,
    "architecture": architecture,
    "source_revision": revision,
    "commit_epoch": int(epoch),
}
if any(value.get(key) != wanted for key, wanted in expected.items()):
    raise SystemExit("offline base receipt binding mismatch")
for key in ("resolved_digest", "platform_digest"):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", str(value.get(key, ""))):
        raise SystemExit(f"offline base receipt {key} is malformed")
print(json.dumps(value, sort_keys=True, separators=(",", ":")))
PY
)" || {
        echo "FATAL: reusable App VM base-image receipt admission failed" >&2
        exit 2
    }
else
    BASE_RECEIPT_JSON="$($BASE_RECEIPT_VERIFY --repo "$REPO" inspect \
        --image-reference "$EFFECTIVE_BASE" --architecture "$REGISTRY_ARCH" \
        --source-revision "$SOURCE_COMMIT" --commit-epoch "$COMMIT_EPOCH" \
        --receipt "$BASE_RECEIPT")" || {
        echo "FATAL: App VM base-image receipt admission failed" >&2
        exit 2
    }
fi
BASE_MANIFEST_DIGEST="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["resolved_digest"])' <<<"$BASE_RECEIPT_JSON")"
BASE_PLATFORM_DIGEST="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["platform_digest"] or "")' <<<"$BASE_RECEIPT_JSON")"
BASE_ID="${BASE_PLATFORM_DIGEST:-$BASE_MANIFEST_DIGEST}"
BASE_REPOSITORY="${EFFECTIVE_BASE%%@*}"
BASE_LAST_COMPONENT="${BASE_REPOSITORY##*/}"
if [[ "$BASE_LAST_COMPONENT" == *:* ]]; then
    BASE_REPOSITORY="${BASE_REPOSITORY%:*}"
fi
PINNED_BASE="$BASE_REPOSITORY@$BASE_ID"

if [ "$LANE" = "local" ]; then
    # The caller-supplied manifest is a consistency record, not revision trust.
    # The verifier authenticates SOURCE_COMMIT from the signed RPM binaries'
    # compile-time BuildInfo before touching the build context. Repeat against
    # the read-only copies; the Containerfile performs the same gate on the
    # bytes crossing the actual image-build boundary.
    "$RPM_SUPPLY_VERIFY" --key "$RPM_KEY" --source-commit "$SOURCE_COMMIT" \
        --candidate-manifest "$CANDIDATE_MANIFEST" "${RPMS[0]}"
fi
find "$RPMS_DIR" -maxdepth 1 -type f \
    \( -name '*.rpm' -o -name 'candidate-manifest.json' \) -delete
if [ "$LANE" = "local" ]; then
    install -v -m 0444 -- "${RPMS[0]}" "$RPMS_DIR/magic-mesh-local.rpm"
    install -v -m 0444 -- "$CANDIDATE_MANIFEST" "$RPMS_DIR/candidate-manifest.json"
    "$RPM_SUPPLY_VERIFY" --key "$RPM_KEY" --source-commit "$SOURCE_COMMIT" \
        --candidate-manifest "$RPMS_DIR/candidate-manifest.json" \
        "$RPMS_DIR/magic-mesh-local.rpm"
fi

resolve_image "$PINNED_BASE" "App VM base"

CONTRACT_ID="wayland-standard-v1"

args=(
    --build-arg "MCNF_RPM_LANE=$LANE"
    --build-arg "MCNF_APP_VM_SOURCE_COMMIT=$SOURCE_COMMIT"
    --build-arg "MCNF_APP_VM_BASE_IMAGE_ID=$BASE_ID"
)
# Never let the mutable selection ref cross the build boundary. A tag may be
# retargeted after resolve/inspect; FROM consumes the captured immutable ID.
# The registry receipt, not mutable local image storage, owns the FROM input.
append_pinned_base_arg "$EFFECTIVE_BASE" "$BASE_ID" "$PINNED_BASE"
cache_args=()
if [ "${MCNF_APP_VM_NO_CACHE:-0}" = 1 ]; then
    cache_args+=(--no-cache)
fi
if [ -n "$REUSE_IMAGE" ]; then
    podman image exists "$IMAGE" || {
        echo "FATAL: requested reusable App VM image is not in local storage: $IMAGE" >&2
        exit 1
    }
    REUSE_TAG="localhost/magic-mesh-app-vm-wayland:checkpoint-${REUSE_IMAGE#sha256:}"
    podman tag "$REUSE_IMAGE" "$REUSE_TAG"
    IMAGE="$REUSE_TAG"
    echo "==> reusing immutable App VM image checkpoint: $IMAGE"
else
    podman build "${cache_args[@]}" "${args[@]}" \
        --label "org.mcnf.app-vm.profile=$CONTRACT_ID" \
        --label "org.mcnf.app-vm.base-image=$EFFECTIVE_BASE" \
        --label "org.mcnf.app-vm.base-image-id=$BASE_ID" \
        --label "org.mcnf.app-vm.source-commit=$SOURCE_COMMIT" \
        -t "$IMAGE" \
        --ignorefile "$APP_VM_DIR/context.containerignore" \
        -f "$CONTAINERFILE" \
        "$REPO"
fi

# Inspect the built image before producing a disk artifact. This is a contents
# gate, not a boot claim: the image must contain the fixed guest contract and
# must not silently acquire a public Flatpak remote.
IMAGE_ID_RAW="$(podman image inspect --format '{{.Id}}' "$IMAGE" 2>/dev/null || true)"
if ! IMAGE_ID="$(canonical_image_id "$IMAGE_ID_RAW")"; then
    echo "FATAL: built App VM tag has no immutable image ID: $IMAGE" >&2
    exit 2
fi
"$REPO/packaging/app-vm/verify-image.sh" "$IMAGE_ID"

IMAGE_PROFILE="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.profile"}}' "$IMAGE_ID" 2>/dev/null || true)"
IMAGE_BASE_ID="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.base-image-id"}}' "$IMAGE_ID" 2>/dev/null || true)"
IMAGE_SOURCE_COMMIT="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.source-commit"}}' "$IMAGE_ID" 2>/dev/null || true)"
if [ "$IMAGE_PROFILE" != "$CONTRACT_ID" ] || \
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
        --rootfs "$APP_VM_ROOTFS" --type "$DISK" --local "$IMAGE"
fi
