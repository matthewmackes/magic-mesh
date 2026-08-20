#!/usr/bin/env bash
# WL-CRIT-006 — admit every mandatory first-release input before build mutation.
set -euo pipefail
umask 077

ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
KIRON_VERIFY="${MCNF_KIRON_VERIFY:-$ROOT/packaging/kiron/verify-package.sh}"
APP_VM_BASE_RECEIPT="${MCNF_APP_VM_BASE_RECEIPT_INSPECTOR:-$ROOT/packaging/app-vm/produce-base-image-receipt.py}"
SOURCE_VERIFY="${MCNF_SOURCE_VERIFY:-$ROOT/install-helpers/source-revision-receipt.sh}"
RPM_SIGNING_RECEIPT="${MCNF_RPM_SIGNING_RECEIPT_INSPECTOR:-$ROOT/install-helpers/produce-rpm-signing-identity-receipt.py}"
BOOTC_DIGEST_RECEIPT="${MCNF_BOOTC_DIGEST_RECEIPT_INSPECTOR:-$ROOT/install-helpers/produce-bootc-digest-receipt.py}"
MAPS_PRODUCER="${MCNF_MAPS_PRODUCER:-$ROOT/packaging/maps/produce-offline-catalog.py}"
MAPS_MATERIALIZER="${MCNF_MAPS_MATERIALIZER:-$ROOT/packaging/maps/materialize-offline-catalog.py}"
APP_CATALOG_RECEIPT="${MCNF_APP_CATALOG_RECEIPT:-$ROOT/packaging/app-vm/produce-catalog-receipt.py}"

die() { printf 'release-input-preflight: REFUSED: %s\n' "$*" >&2; exit 2; }
need() { [[ -n "${2:-}" ]] || die "missing mandatory input: $1"; }
regular_file() {
  local label=$1 path=$2
  [[ -n "$path" && -f "$path" && ! -L "$path" ]] ||
    die "$label must be a regular non-symlink file"
}
directory() {
  local label=$1 path=$2
  [[ -n "$path" && -d "$path" && ! -L "$path" ]] ||
    die "$label must be a real non-symlink directory"
}
source_revision='' source_epoch=''
maps_approval='' maps_source_root='' maps_quota='' maps_verifier='' maps_mbtiles=''
rpm_signing_receipt='' bootc_receipt='' bootc_reference='' bootc_architecture='' bootc_role=''
app_vm_base_receipt='' app_vm_base_reference='' app_vm_base_architecture='' app_vm_catalog_receipt=''
while (($#)); do
  case "$1" in
    --source-revision) source_revision=${2:-}; shift 2 ;;
    --source-epoch) source_epoch=${2:-}; shift 2 ;;
    --maps-approval) maps_approval=${2:-}; shift 2 ;;
    --maps-tile-source-root) maps_source_root=${2:-}; shift 2 ;;
    --maps-quota-bytes) maps_quota=${2:-}; shift 2 ;;
    --maps-verifier) maps_verifier=${2:-}; shift 2 ;;
    --maps-mbtiles) maps_mbtiles=${2:-}; shift 2 ;;
    --rpm-signing-identity-receipt) rpm_signing_receipt=${2:-}; shift 2 ;;
    --bootc-base-digest-receipt) bootc_receipt=${2:-}; shift 2 ;;
    --bootc-base-image-reference) bootc_reference=${2:-}; shift 2 ;;
    --bootc-base-architecture) bootc_architecture=${2:-}; shift 2 ;;
    --bootc-release-role) bootc_role=${2:-}; shift 2 ;;
    --app-vm-base-image-receipt) app_vm_base_receipt=${2:-}; shift 2 ;;
    --app-vm-base-image-reference) app_vm_base_reference=${2:-}; shift 2 ;;
    --app-vm-base-architecture) app_vm_base_architecture=${2:-}; shift 2 ;;
    --app-vm-catalog-receipt) app_vm_catalog_receipt=${2:-}; shift 2 ;;
    *) die "unknown or incomplete argument: $1" ;;
  esac
done

for pair in \
  'source revision' "$source_revision" 'source epoch' "$source_epoch" \
  'Maps approval' "$maps_approval" 'Maps tile source root' "$maps_source_root" 'Maps MBTiles' "$maps_mbtiles" \
  'Maps quota bytes' "$maps_quota" 'Maps production verifier' "$maps_verifier" \
  'RPM signing identity receipt' "$rpm_signing_receipt" 'bootc base digest receipt' "$bootc_receipt" \
  'bootc base image reference' "$bootc_reference" 'bootc base architecture' "$bootc_architecture" \
  'bootc release role' "$bootc_role" \
  'App VM base image receipt' "$app_vm_base_receipt" \
  'App VM curated catalog receipt' "$app_vm_catalog_receipt" \
  'App VM base image reference' "$app_vm_base_reference" \
  'App VM base architecture' "$app_vm_base_architecture"; do
  if [[ -z ${label+x} ]]; then label=$pair; else need "$label" "$pair"; unset label; fi
done

regular_file 'Maps approval' "$maps_approval"
directory 'Maps tile source root' "$maps_source_root"
regular_file 'Maps production verifier' "$maps_verifier"
regular_file 'Maps MBTiles' "$maps_mbtiles"
regular_file 'RPM signing identity receipt' "$rpm_signing_receipt"
regular_file 'bootc base digest receipt' "$bootc_receipt"
regular_file 'App VM base image receipt' "$app_vm_base_receipt"
regular_file 'App VM curated catalog receipt' "$app_vm_catalog_receipt"

"$SOURCE_VERIFY" --verify "$source_revision" "$source_epoch" >/dev/null \
  || die 'source revision receipt is invalid'
"$KIRON_VERIFY" --source --expected-source-revision "$source_revision" >/dev/null \
  || die 'UX-014 A-F package admission failed'
python3 "$APP_VM_BASE_RECEIPT" --repo "$ROOT" inspect \
  --image-reference "$app_vm_base_reference" --architecture "$app_vm_base_architecture" \
  --source-revision "$source_revision" --commit-epoch "$source_epoch" \
  --receipt "$app_vm_base_receipt" >/dev/null \
  || die 'App VM base-image receipt admission failed'

maps_stage=$(mktemp -d)
cleanup() {
  # The Maps producer intentionally seals its bundle directories 0555. This is
  # our private mktemp tree, so restore owner write permission solely to erase
  # the preflight copy; approved source bytes are never modified.
  chmod -R u+w -- "$maps_stage" 2>/dev/null || true
  rm -rf -- "$maps_stage"
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
  --mbtiles "$maps_mbtiles" \
  --quota-bytes "$maps_quota" >/dev/null \
  || die 'offline Maps bundle admission/materialization failed'
python3 "$APP_CATALOG_RECEIPT" --catalog "$app_vm_catalog_receipt" \
  --source-revision "$source_revision" --source-epoch "$source_epoch" \
  --output "$maps_stage/app-catalog-receipt.json" >/dev/null \
  || die 'App VM curated catalog admission failed'
python3 "$RPM_SIGNING_RECEIPT" inspect --receipt "$rpm_signing_receipt" \
  --expected-source-revision "$source_revision" --expected-release-epoch "$source_epoch" >/dev/null \
  || die 'RPM signing identity receipt admission failed'

python3 "$BOOTC_DIGEST_RECEIPT" inspect --receipt "$bootc_receipt" \
  --expected-image-reference "$bootc_reference" --expected-architecture "$bootc_architecture" \
  --expected-source-revision "$source_revision" --expected-commit-epoch "$source_epoch" \
  --expected-release-role "$bootc_role" >/dev/null \
  || die 'bootc base digest receipt admission failed'

printf 'release-input-preflight: PASS: all mandatory first-release inputs admitted for %s\n' "$source_revision"
