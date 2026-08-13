#!/usr/bin/env bash
# WL-CRIT-006 — admit every mandatory first-release input before build mutation.
set -euo pipefail
umask 077

ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
KIRON_VERIFY="${MCNF_KIRON_VERIFY:-$ROOT/packaging/kiron/verify-package.sh}"
APP_TRUST_VERIFY="${MCNF_APP_TRUST_VERIFY:-$ROOT/install-helpers/verify-app-vm-catalog-trust.py}"
CUTTLEFISH_VERIFY="${MCNF_CUTTLEFISH_VERIFY:-$ROOT/packaging/android/verify-guest-payload.sh}"
CUTTLEFISH_DEB_VERIFY="${MCNF_CUTTLEFISH_DEB_VERIFY:-$ROOT/packaging/android/verify-guest-debs.sh}"
SOURCE_VERIFY="${MCNF_SOURCE_VERIFY:-$ROOT/install-helpers/source-revision-receipt.sh}"
RPM_SIGNING_RECEIPT="${MCNF_RPM_SIGNING_RECEIPT_INSPECTOR:-$ROOT/install-helpers/produce-rpm-signing-identity-receipt.py}"
BOOTC_DIGEST_RECEIPT="${MCNF_BOOTC_DIGEST_RECEIPT_INSPECTOR:-$ROOT/install-helpers/produce-bootc-digest-receipt.py}"
CUTTLEFISH_IMAGE_RECEIPT="${MCNF_CUTTLEFISH_IMAGE_RECEIPT_INSPECTOR:-$ROOT/packaging/android/produce-image-receipt.py}"
CUTTLEFISH_IMAGE_REPO="${MCNF_CUTTLEFISH_IMAGE_REPO:-$ROOT}"
MAPS_PRODUCER="${MCNF_MAPS_PRODUCER:-$ROOT/packaging/maps/produce-offline-catalog.py}"
MAPS_MATERIALIZER="${MCNF_MAPS_MATERIALIZER:-$ROOT/packaging/maps/materialize-offline-catalog.py}"

die() { printf 'release-input-preflight: REFUSED: %s\n' "$*" >&2; exit 2; }
need() { [[ -n "${2:-}" ]] || die "missing mandatory input: $1"; }
digest() {
  [[ "$2" =~ ^sha256:[0-9a-f]{64}$ && "$2" != "sha256:$(printf '%064d' 0)" ]] \
    || die "$1 must be a non-null immutable sha256 digest"
}

source_revision='' source_epoch='' app_receipt='' app_key=''
maps_approval='' maps_source_root='' maps_quota='' maps_verifier=''
cuttlefish_declaration='' cuttlefish_signature='' cuttlefish_relay='' cuttlefish_agent=''
rpm_signing_receipt='' bootc_receipt='' bootc_reference='' bootc_architecture='' bootc_role=''
app_vm_base_digest='' cuttlefish_image_receipt='' cuttlefish_image_source_kind=''
cuttlefish_image_original_source='' cuttlefish_image_architecture=''
cuttlefish_provider_identity='' cuttlefish_android_release_id=''
cuttlefish_image_compatibility_id='' cuttlefish_image_media_type='application/octet-stream'
cuttlefish_image_artifact_format='android-cuttlefish-host-package'
cuttlefish_packages=()
while (($#)); do
  case "$1" in
    --source-revision) source_revision=${2:-}; shift 2 ;;
    --source-epoch) source_epoch=${2:-}; shift 2 ;;
    --maps-approval) maps_approval=${2:-}; shift 2 ;;
    --maps-tile-source-root) maps_source_root=${2:-}; shift 2 ;;
    --maps-quota-bytes) maps_quota=${2:-}; shift 2 ;;
    --maps-verifier) maps_verifier=${2:-}; shift 2 ;;
    --app-vm-catalog-trust-receipt) app_receipt=${2:-}; shift 2 ;;
    --app-vm-catalog-trust-key) app_key=${2:-}; shift 2 ;;
    --cuttlefish-declaration) cuttlefish_declaration=${2:-}; shift 2 ;;
    --cuttlefish-signature) cuttlefish_signature=${2:-}; shift 2 ;;
    --cuttlefish-readiness-relay) cuttlefish_relay=${2:-}; shift 2 ;;
    --cuttlefish-vdi-agent) cuttlefish_agent=${2:-}; shift 2 ;;
    --cuttlefish-guest-package) cuttlefish_packages+=("${2:-}"); shift 2 ;;
    --rpm-signing-identity-receipt) rpm_signing_receipt=${2:-}; shift 2 ;;
    --bootc-base-digest-receipt) bootc_receipt=${2:-}; shift 2 ;;
    --bootc-base-image-reference) bootc_reference=${2:-}; shift 2 ;;
    --bootc-base-architecture) bootc_architecture=${2:-}; shift 2 ;;
    --bootc-release-role) bootc_role=${2:-}; shift 2 ;;
    --app-vm-base-digest) app_vm_base_digest=${2:-}; shift 2 ;;
    --cuttlefish-image-receipt) cuttlefish_image_receipt=${2:-}; shift 2 ;;
    --cuttlefish-image-source-kind) cuttlefish_image_source_kind=${2:-}; shift 2 ;;
    --cuttlefish-image-original-source) cuttlefish_image_original_source=${2:-}; shift 2 ;;
    --cuttlefish-image-architecture) cuttlefish_image_architecture=${2:-}; shift 2 ;;
    --cuttlefish-provider-identity) cuttlefish_provider_identity=${2:-}; shift 2 ;;
    --cuttlefish-android-release-id) cuttlefish_android_release_id=${2:-}; shift 2 ;;
    --cuttlefish-image-compatibility-id) cuttlefish_image_compatibility_id=${2:-}; shift 2 ;;
    --cuttlefish-image-media-type) cuttlefish_image_media_type=${2:-}; shift 2 ;;
    --cuttlefish-image-artifact-format) cuttlefish_image_artifact_format=${2:-}; shift 2 ;;
    *) die "unknown or incomplete argument: $1" ;;
  esac
done

for pair in \
  'source revision' "$source_revision" 'source epoch' "$source_epoch" \
  'Maps approval' "$maps_approval" 'Maps tile source root' "$maps_source_root" \
  'Maps quota bytes' "$maps_quota" 'Maps production verifier' "$maps_verifier" \
  'App VM catalog trust receipt' "$app_receipt" 'App VM catalog trust key' "$app_key" \
  'Cuttlefish declaration' "$cuttlefish_declaration" 'Cuttlefish signature' "$cuttlefish_signature" \
  'Cuttlefish readiness relay' "$cuttlefish_relay" 'Cuttlefish VDI agent' "$cuttlefish_agent" \
  'RPM signing identity receipt' "$rpm_signing_receipt" 'bootc base digest receipt' "$bootc_receipt" \
  'bootc base image reference' "$bootc_reference" 'bootc base architecture' "$bootc_architecture" \
  'bootc release role' "$bootc_role" \
  'App VM base digest' "$app_vm_base_digest" \
  'Cuttlefish image receipt' "$cuttlefish_image_receipt" \
  'Cuttlefish image source kind' "$cuttlefish_image_source_kind" \
  'Cuttlefish image original source' "$cuttlefish_image_original_source" \
  'Cuttlefish image architecture' "$cuttlefish_image_architecture" \
  'Cuttlefish provider identity' "$cuttlefish_provider_identity" \
  'Cuttlefish Android release ID' "$cuttlefish_android_release_id" \
  'Cuttlefish image compatibility ID' "$cuttlefish_image_compatibility_id"; do
  if [[ -z ${label+x} ]]; then label=$pair; else need "$label" "$pair"; unset label; fi
done

"$SOURCE_VERIFY" --verify "$source_revision" "$source_epoch" >/dev/null \
  || die 'source revision receipt is invalid'
"$KIRON_VERIFY" --source >/dev/null || die 'UX-014 A-F package admission failed'

trust_stage=$(mktemp -d)
payload_parent=$(mktemp -d)
deb_stage=$(mktemp -d)
maps_stage=$(mktemp -d)
image_identity=$(mktemp)
cleanup() {
  # The Maps producer intentionally seals its bundle directories 0555. This is
  # our private mktemp tree, so restore owner write permission solely to erase
  # the preflight copy; approved source bytes are never modified.
  chmod -R u+w -- "$maps_stage" 2>/dev/null || true
  rm -rf -- "$trust_stage" "$payload_parent" "$deb_stage" "$maps_stage"
  rm -f -- "$image_identity"
}
trap cleanup EXIT

# WL-FUNC-017 — release engineering supplies an approval document plus its
# immutable, content-addressed tile source outside Git. Reproduce the governed
# bundle, then make the production Maps APIs admit a no-replace materialization
# before any release build mutation. A caller-authored bundle cannot bypass the
# producer.
[[ "$maps_quota" =~ ^[1-9][0-9]*$ ]] || die 'Maps quota bytes must be a positive integer'
python3 "$MAPS_PRODUCER" --approval "$maps_approval" \
  --source-root "$maps_source_root" --output "$maps_stage/bundle" >/dev/null \
  || die 'offline Maps approved bundle production failed'
python3 "$MAPS_MATERIALIZER" --bundle "$maps_stage/bundle" \
  --cache-root "$maps_stage/cache" --verifier "$maps_verifier" \
  --source-revision "$source_revision" --source-epoch "$source_epoch" \
  --quota-bytes "$maps_quota" >/dev/null \
  || die 'offline Maps bundle admission/materialization failed'
"$APP_TRUST_VERIFY" --receipt "$app_receipt" --key "$app_key" \
  --expected-source-revision "$source_revision" --stage-dir "$trust_stage" >/dev/null \
  || die 'App VM catalog trust admission failed'

cuttlefish_args=(--declaration "$cuttlefish_declaration" --signature "$cuttlefish_signature"
  --readiness-relay "$cuttlefish_relay" --vdi-agent "$cuttlefish_agent"
  --stage-dir "$payload_parent/admitted")
for package in "${cuttlefish_packages[@]}"; do
  [[ -n "$package" ]] || die 'empty Cuttlefish guest package input'
  cuttlefish_args+=(--guest-package "$package")
done
"$CUTTLEFISH_VERIFY" "${cuttlefish_args[@]}" >/dev/null \
  || die 'Cuttlefish signed guest payload admission failed'

# The signed declaration alone does not prove that release engineering supplied
# the deterministic package set. Require the exact two package names from one
# manifest-bearing directory, then bind their archived binaries and units to
# the same admitted relay/agent bytes through the owning package verifier.
[[ ${#cuttlefish_packages[@]} -eq 2 ]] \
  || die 'Cuttlefish release requires exactly two deterministic guest DEBs'
package_dir=''
declare -A cuttlefish_package_names=()
for package in "${cuttlefish_packages[@]}"; do
  [[ -f "$package" && ! -L "$package" ]] || die "Cuttlefish guest DEB is missing or substituted: $package"
  [[ -z "$package_dir" || $(dirname -- "$package") == "$package_dir" ]] \
    || die 'Cuttlefish guest DEBs must share one manifest-bearing directory'
  package_dir=$(dirname -- "$package")
  cuttlefish_package_names["$(basename -- "$package")"]=1
done
for name in mcnf-cuttlefish-readiness-relay.deb mcnf-cuttlefish-vdi-agent.deb; do
  [[ ${cuttlefish_package_names[$name]:-0} -eq 1 ]] \
    || die "Cuttlefish deterministic guest DEB is missing: $name"
done
install -m 0555 -- "$cuttlefish_relay" "$deb_stage/mcnf-cuttlefish-readiness-relay"
install -m 0555 -- "$cuttlefish_agent" "$deb_stage/mcnf-cuttlefish-vdi-agent"
"$CUTTLEFISH_DEB_VERIFY" --source-revision "$source_revision" \
  --stage-dir "$deb_stage" --package-dir "$package_dir" >/dev/null \
  || die 'Cuttlefish deterministic guest DEB admission failed'

python3 "$CUTTLEFISH_IMAGE_RECEIPT" --repo "$CUTTLEFISH_IMAGE_REPO" inspect \
  --source-kind "$cuttlefish_image_source_kind" \
  --original-source "$cuttlefish_image_original_source" \
  --architecture "$cuttlefish_image_architecture" \
  --provider-identity "$cuttlefish_provider_identity" \
  --android-release-id "$cuttlefish_android_release_id" \
  --compatibility-id "$cuttlefish_image_compatibility_id" \
  --source-revision "$source_revision" --commit-epoch "$source_epoch" \
  --media-type "$cuttlefish_image_media_type" \
  --artifact-format "$cuttlefish_image_artifact_format" \
  --receipt "$cuttlefish_image_receipt" >"$image_identity" \
  || die 'Cuttlefish image receipt admission failed'

python3 - "$payload_parent/admitted/release.json" "$image_identity" <<'PY' \
  || die 'signed Cuttlefish declaration does not bind the admitted image receipt'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    declaration = json.load(stream)
with open(sys.argv[2], encoding="utf-8") as stream:
    receipt = json.load(stream)
if declaration.get("image_identity") != receipt:
    raise SystemExit(1)
PY

python3 "$RPM_SIGNING_RECEIPT" inspect --receipt "$rpm_signing_receipt" \
  --expected-source-revision "$source_revision" --expected-release-epoch "$source_epoch" >/dev/null \
  || die 'RPM signing identity receipt admission failed'

python3 "$BOOTC_DIGEST_RECEIPT" inspect --receipt "$bootc_receipt" \
  --expected-image-reference "$bootc_reference" --expected-architecture "$bootc_architecture" \
  --expected-source-revision "$source_revision" --expected-commit-epoch "$source_epoch" \
  --expected-release-role "$bootc_role" >/dev/null \
  || die 'bootc base digest receipt admission failed'

digest 'App VM base digest' "$app_vm_base_digest"
printf 'release-input-preflight: PASS: all mandatory first-release inputs admitted for %s\n' "$source_revision"
