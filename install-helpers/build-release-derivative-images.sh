#!/usr/bin/env bash
# WL-CRIT-006 — build and collect verified first-release derivative images.
#
# This helper deliberately starts after RPM signing. It neither signs nor
# publishes anything: both signed RPM candidates are admitted by their existing
# verifiers, both existing image builders own image construction, and one
# no-replace output directory is published only after every derivative passes.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP_BUILDER=${MCNF_DERIVATIVE_APP_BUILDER:-$ROOT/packaging/app-vm/build-image.sh}
BROWSER_BUILDER=${MCNF_DERIVATIVE_BROWSER_BUILDER:-$ROOT/packaging/browser-vm/build-image.sh}
APP_RPM_VERIFY=${MCNF_DERIVATIVE_APP_RPM_VERIFY:-$ROOT/packaging/app-vm/verify-rpm-supply.sh}
BROWSER_RPM_VERIFY=${MCNF_DERIVATIVE_BROWSER_RPM_VERIFY:-$ROOT/packaging/browser-vm/produce-lighthouse-rpm-candidate.py}
BROWSER_MANIFEST_VERIFY=${MCNF_DERIVATIVE_BROWSER_MANIFEST_VERIFY:-$ROOT/packaging/browser-vm/verify-image-manifest.py}
APP_MANIFEST_VERIFY=${MCNF_DERIVATIVE_APP_MANIFEST_VERIFY:-$ROOT/packaging/app-vm/verify-qcow2-manifest.py}
RELEASE_KEY=${MCNF_DERIVATIVE_RELEASE_KEY:-$ROOT/packaging/repo/RPM-GPG-KEY-magic-mesh}
PROFILE=${MCNF_DERIVATIVE_BROWSER_PROFILE:-$ROOT/packaging/browser-vm/profile.env}

refuse() { printf 'release-derivative-images: REFUSED: %s\n' "$*" >&2; exit 2; }
usage() {
    cat <<'EOF'
Usage: build-release-derivative-images.sh \
  --source-revision REVISION --signed-workstation-rpm PATH \
  --app-rpm-candidate-manifest PATH --app-base-receipt PATH \
  --app-base-image IMAGE --app-catalog-trust-receipt PATH \
  --app-catalog-trust-key PATH --signed-lighthouse-rpm PATH \
  --browser-rpm-candidate-manifest PATH --browser-base-receipt PATH \
  --browser-base-image IMAGE --output DIR

Builds and verifies App VM and Browser VM qcow2 derivatives. The output is an
atomic, immutable candidate collection; it is not publication or promotion.
EOF
}

source_revision='' workstation_rpm='' app_candidate='' app_base_receipt=''
app_base='' app_trust_receipt='' app_trust_key='' lighthouse_rpm=''
browser_candidate='' browser_base_receipt='' browser_base='' output=''
while (($#)); do
    case "$1" in
        --source-revision) source_revision=${2:-}; shift 2 ;;
        --signed-workstation-rpm) workstation_rpm=${2:-}; shift 2 ;;
        --app-rpm-candidate-manifest) app_candidate=${2:-}; shift 2 ;;
        --app-base-receipt) app_base_receipt=${2:-}; shift 2 ;;
        --app-base-image) app_base=${2:-}; shift 2 ;;
        --app-catalog-trust-receipt) app_trust_receipt=${2:-}; shift 2 ;;
        --app-catalog-trust-key) app_trust_key=${2:-}; shift 2 ;;
        --signed-lighthouse-rpm) lighthouse_rpm=${2:-}; shift 2 ;;
        --browser-rpm-candidate-manifest) browser_candidate=${2:-}; shift 2 ;;
        --browser-base-receipt) browser_base_receipt=${2:-}; shift 2 ;;
        --browser-base-image) browser_base=${2:-}; shift 2 ;;
        --output) output=${2:-}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) refuse "unknown or incomplete argument: $1" ;;
    esac
done

[[ "$source_revision" =~ ^[0-9a-f]{40}$ && "$source_revision" != 0000000000000000000000000000000000000000 ]] \
    || refuse 'source revision must be one non-null lowercase Git object ID'
for pair in \
    'signed Workstation RPM' "$workstation_rpm" \
    'App VM RPM candidate manifest' "$app_candidate" \
    'App VM base receipt' "$app_base_receipt" \
    'App VM base image' "$app_base" \
    'App VM catalog trust receipt' "$app_trust_receipt" \
    'App VM catalog trust key' "$app_trust_key" \
    'signed Lighthouse RPM' "$lighthouse_rpm" \
    'Browser RPM candidate manifest' "$browser_candidate" \
    'Browser base receipt' "$browser_base_receipt" \
    'Browser base image' "$browser_base" \
    'output directory' "$output"; do
    if [[ -z ${label+x} ]]; then label=$pair; else [[ -n "$pair" ]] || refuse "missing $label"; unset label; fi
done

regular_input() { # label path maximum-bytes
    local label=$1 path=$2 maximum=$3 mode size
    [[ -f "$path" && ! -L "$path" ]] || refuse "$label must be a regular non-symlink file"
    read -r mode size < <(stat -Lc '%a %s' -- "$path") || refuse "$label metadata is unavailable"
    [[ "$mode" =~ ^[0-7]{3,4}$ ]] || refuse "$label mode is malformed"
    (( (8#$mode & 0022) == 0 )) || refuse "$label must not be group/other writable"
    if [[ ! "$size" =~ ^[0-9]+$ ]] || ((size <= 0 || size > maximum)); then
        refuse "$label exceeds its bounded size contract"
    fi
}
regular_input 'signed Workstation RPM' "$workstation_rpm" 1073741824
regular_input 'App VM RPM candidate manifest' "$app_candidate" 1048576
regular_input 'App VM base receipt' "$app_base_receipt" 1048576
regular_input 'App VM catalog trust receipt' "$app_trust_receipt" 1048576
regular_input 'App VM catalog trust key' "$app_trust_key" 1048576
regular_input 'signed Lighthouse RPM' "$lighthouse_rpm" 1073741824
regular_input 'Browser RPM candidate manifest' "$browser_candidate" 1048576
regular_input 'Browser base receipt' "$browser_base_receipt" 1048576
regular_input 'governed release key' "$RELEASE_KEY" 1048576
regular_input 'Browser profile' "$PROFILE" 65536

profile_revision=$(sed -n 's/^BROWSER_VM_SOURCE_COMMIT=//p' "$PROFILE")
[[ "$profile_revision" == "$source_revision" ]] \
    || refuse 'Browser profile is not bound to the requested release revision'
[[ ! -e "$output" && ! -L "$output" ]] || refuse 'output path already exists or is substituted'
output_parent=$(dirname -- "$output")
[[ -d "$output_parent" && ! -L "$output_parent" ]] || refuse 'output parent must be an existing real directory'
parent_mode=$(stat -Lc '%a' -- "$output_parent")
(( (8#$parent_mode & 0022) == 0 )) || refuse 'output parent must not be group/other writable'

work=$(mktemp -d --tmpdir="$output_parent" .release-derivatives.XXXXXX)
chmod 0700 "$work"
cleanup() { chmod -R u+rwX -- "$work" 2>/dev/null || true; rm -rf -- "$work"; }
trap cleanup EXIT
inputs=$work/inputs
browser_out=$work/browser-build
app_out=$work/app-build
collection=$work/collection
mkdir -m 0700 "$inputs" "$browser_out" "$app_out" "$collection"

# Snapshot the admitted bytes. Builders never receive caller-owned RPM paths,
# so failure cannot partially sign, rewrite, or consume the signed originals.
install -m 0400 -- "$workstation_rpm" "$inputs/workstation.rpm"
install -m 0400 -- "$lighthouse_rpm" "$inputs/lighthouse.rpm"
install -m 0400 -- "$app_candidate" "$inputs/app-candidate.json"
install -m 0400 -- "$browser_candidate" "$inputs/browser-candidate.json"

"$APP_RPM_VERIFY" --key "$RELEASE_KEY" --source-commit "$source_revision" \
    --candidate-manifest "$inputs/app-candidate.json" "$inputs/workstation.rpm" \
    || refuse 'signed Workstation RPM admission failed'
python3 "$BROWSER_RPM_VERIFY" verify --rpm "$inputs/lighthouse.rpm" \
    --source-revision "$source_revision" --release-key "$RELEASE_KEY" \
    --manifest "$inputs/browser-candidate.json" >/dev/null \
    || refuse 'signed Lighthouse RPM admission failed'

MCNF_APP_VM_SOURCE_COMMIT="$source_revision" "$APP_BUILDER" \
    --rpm "$inputs/workstation.rpm" --candidate-manifest "$inputs/app-candidate.json" \
    --catalog-trust-receipt "$app_trust_receipt" --catalog-trust-key "$app_trust_key" \
    --base-receipt "$app_base_receipt" --base "$app_base" \
    --disk qcow2 --out "$app_out" \
    || refuse 'App VM derivative build or verification failed'

"$BROWSER_BUILDER" --rpm "$inputs/lighthouse.rpm" \
    --base-receipt "$browser_base_receipt" --base "$browser_base" \
    --disk qcow2 --out "$browser_out" \
    || refuse 'Browser VM derivative build or verification failed'

app_disk=$app_out/qcow2/disk.qcow2
browser_disk=$browser_out/qcow2/disk.qcow2
browser_manifest=${browser_disk}.mcnf-manifest.json
for pair in 'App VM disk' "$app_disk" 'Browser VM disk' "$browser_disk" 'Browser VM manifest' "$browser_manifest"; do
    if [[ -z ${label+x} ]]; then label=$pair; else regular_input "$label" "$pair" 137438953472; unset label; fi
done
python3 "$BROWSER_MANIFEST_VERIFY" verify --repo-root "$ROOT" --profile "$PROFILE" \
    --image "$browser_disk" --manifest "$browser_manifest" >/dev/null \
    || refuse 'Browser VM emitted manifest re-verification failed'

install -m 0400 -- "$app_disk" "$collection/app-vm-wayland-standard.qcow2"
install -m 0400 -- "$browser_disk" "$collection/browser-vm-chromium.qcow2"
install -m 0400 -- "$browser_manifest" "$collection/browser-vm-chromium.mcnf-manifest.json"
python3 - "$collection" "$source_revision" <<'PY'
import hashlib, json, os, pathlib, sys
root = pathlib.Path(sys.argv[1])
revision = sys.argv[2]
def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024): value.update(chunk)
    return "sha256:" + value.hexdigest()
app_path = root / "app-vm-wayland-standard.qcow2"
app_manifest = {
    "artifact": {"filename": app_path.name, "sha256": digest(app_path), "size": app_path.stat().st_size},
    "image_profile": "mcnf-app-vm/wayland-standard-v1",
    "kind": "mcnf-app-vm-image-manifest",
    "schema_version": 1,
    "source_revision": revision,
}
app_manifest_path = root / "app-vm-wayland-standard.mcnf-manifest.json"
fd = os.open(app_manifest_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
with os.fdopen(fd, "wb") as stream:
    stream.write((json.dumps(app_manifest, sort_keys=True, separators=(",", ":")) + "\n").encode())
    stream.flush(); os.fsync(stream.fileno())
artifacts = {}
for name in ("app-vm-wayland-standard.qcow2", "app-vm-wayland-standard.mcnf-manifest.json", "browser-vm-chromium.qcow2", "browser-vm-chromium.mcnf-manifest.json"):
    path = root / name
    artifacts[name] = {"sha256": digest(path), "size": path.stat().st_size}
document = {"artifacts": artifacts, "kind": "mcnf-first-release-derivative-image-collection", "promotion": "forbidden", "schema_version": 1, "source_revision": revision}
target = root / "derivative-images.json"
fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
with os.fdopen(fd, "wb") as stream:
    stream.write((json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode())
    stream.flush(); os.fsync(stream.fileno())
PY

python3 "$APP_MANIFEST_VERIFY" --image "$collection/app-vm-wayland-standard.qcow2" \
    --manifest "$collection/app-vm-wayland-standard.mcnf-manifest.json" \
    --source-revision "$source_revision" >/dev/null \
    || refuse 'App VM emitted manifest re-verification failed'

mv -T -- "$collection" "$output" || refuse 'atomic derivative collection publication failed'
chmod 0500 "$output"
printf 'release-derivative-images: PASS: verified non-promoted collection %s\n' "$output"
