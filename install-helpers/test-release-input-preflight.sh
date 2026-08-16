#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PRE="$ROOT/install-helpers/release-input-preflight.sh"
ENTRY="$ROOT/install-helpers/xcp-build.sh"
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin"
marker="$fixture/build-command-ran"
mkdir -p "$fixture/source-repo"
git -C "$fixture/source-repo" init -q
touch "$fixture/source-repo/release-input"
git -C "$fixture/source-repo" add release-input
GIT_AUTHOR_NAME='release preflight self-test' GIT_AUTHOR_EMAIL='preflight@example.invalid' \
  GIT_COMMITTER_NAME='release preflight self-test' GIT_COMMITTER_EMAIL='preflight@example.invalid' \
  git -C "$fixture/source-repo" commit -q -m 'fixture release identity'
source_revision=$(git -C "$fixture/source-repo" rev-parse HEAD)
source_epoch=$(git -C "$fixture/source-repo" show -s --format=%ct "$source_revision")
bootc_reference='registry.invalid/mcnf/bootc:release'
bootc_architecture='amd64'
bootc_role='unified-seat-server'
app_vm_reference='registry.invalid/fedora/app-vm-base:44'
app_vm_architecture='amd64'
for verifier in source; do
  cat >"$fixture/$verifier" <<'EOF'
#!/usr/bin/env bash
exit "${FAKE_VERIFIER_RC:-0}"
EOF
  chmod 0755 "$fixture/$verifier"
done
cat >"$fixture/kiron" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ ${FAKE_VERIFIER_RC:-0} -eq 0 ]] || exit "${FAKE_VERIFIER_RC}"
[[ $# -eq 3 && $1 == --source && $2 == --expected-source-revision \
  && $3 == "$PREFLIGHT_TEST_REVISION" ]]
touch "$PREFLIGHT_TEST_KIRON_MARKER"
EOF
chmod 0755 "$fixture/kiron"
cat >"$fixture/maps-verifier" <<'EOF'
#!/usr/bin/env bash
[[ ${FAKE_VERIFIER_RC:-0} -eq 0 ]] || exit "${FAKE_VERIFIER_RC}"
[[ -f ${1:?}/manifest.json && -f $1/catalog.json && -f $1/payload/index.json ]]
EOF
chmod 0755 "$fixture/maps-verifier"
cat >"$fixture/cuttlefish" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
declaration='' stage=''
while (($#)); do
  case "$1" in
    --declaration) declaration=$2; shift 2 ;;
    --stage-dir) stage=$2; shift 2 ;;
    *) shift 2 ;;
  esac
done
[[ ${FAKE_VERIFIER_RC:-0} -eq 0 ]] || exit "${FAKE_VERIFIER_RC}"
mkdir -p "$stage"
cp "$declaration" "$stage/release.json"
EOF
chmod 0755 "$fixture/cuttlefish"
cat >"$fixture/guest-debs" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
revision='' stage='' packages=''
while (($#)); do
  case "$1" in
    --source-revision) revision=$2; shift 2 ;;
    --stage-dir) stage=$2; shift 2 ;;
    --package-dir) packages=$2; shift 2 ;;
    *) exit 2 ;;
  esac
done
[[ ${FAKE_VERIFIER_RC:-0} -eq 0 ]] || exit "${FAKE_VERIFIER_RC}"
[[ $revision =~ ^[0-9a-f]{40}$ ]]
cmp -s "$stage/mcnf-cuttlefish-readiness-relay" "$PREFLIGHT_TEST_RELAY"
cmp -s "$stage/mcnf-cuttlefish-vdi-agent" "$PREFLIGHT_TEST_AGENT"
[[ -f $packages/guest-deb-manifest.json ]]
touch "$PREFLIGHT_TEST_DEB_MARKER"
EOF
chmod 0755 "$fixture/guest-debs"
cat >"$fixture/signing-receipt.py" <<'EOF'
#!/usr/bin/env python3
import os
raise SystemExit(int(os.environ.get("FAKE_VERIFIER_RC", "0")))
EOF
chmod 0755 "$fixture/signing-receipt.py"
cat >"$fixture/bootc-inspector" <<'EOF'
#!/usr/bin/env python3
import os
import sys

os.execv(
    sys.executable,
    [sys.executable, os.environ["BOOTC_TEST_INSPECTOR"], "--repo", os.environ["BOOTC_TEST_REPO"], *sys.argv[1:]],
)
EOF
chmod 0755 "$fixture/bootc-inspector"
cat >"$fixture/app-vm-base-inspector" <<'EOF'
#!/usr/bin/env python3
import os
import sys

arguments = sys.argv[1:]
if "--repo" in arguments:
    index = arguments.index("--repo")
    del arguments[index:index + 2]
os.execv(
    sys.executable,
    [sys.executable, os.environ["APP_VM_TEST_INSPECTOR"], "--repo", os.environ["APP_VM_TEST_REPO"], "--skopeo", os.environ["APP_VM_TEST_SKOPEO"], *arguments],
)
EOF
chmod 0755 "$fixture/app-vm-base-inspector"
cat >"$fixture/app-vm-skopeo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ ${1:-} == inspect && ${2:-} == --raw && ${3:-} == "docker://${APP_VM_TEST_REFERENCE:?}" ]]
cat "$APP_VM_TEST_MANIFEST"
EOF
chmod 0755 "$fixture/app-vm-skopeo"
cat >"$fixture/bin/gpg" <<'EOF'
#!/usr/bin/env bash
printf 'sec:-:4096:1:DEADBEEF:0:0:::::::23::0:\n'
printf 'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:\n'
EOF
chmod 0755 "$fixture/bin/gpg"
touch "$fixture/relay" "$fixture/agent" "$fixture/signing-receipt.json"
mkdir "$fixture/maps-tiles"
printf 'governed release tile\n' >"$fixture/maps-tiles/tile.bin"
chmod 0444 "$fixture/maps-tiles/tile.bin"
tile_sha=$(sha256sum "$fixture/maps-tiles/tile.bin" | cut -d' ' -f1)
python3 - "$fixture/maps-approval.json" "$source_revision" "$source_epoch" "$tile_sha" <<'PY'
import json
import sys

path, revision, epoch, tile_sha = sys.argv[1:]
approval = {
    "schema": 1,
    "provider": "openstreetmap-derived",
    "attribution": "© OpenStreetMap contributors",
    "license": "ODbL-1.0",
    "source_revision": revision,
    "source_epoch": int(epoch),
    "quota_bytes": 1024,
    "regions": [{
        "region_id": "release-fixture",
        "revision": "2026.08",
        "bounds": {"west": -96.0, "south": 29.0, "east": -93.0, "north": 34.0},
        "min_zoom": 1,
        "max_zoom": 1,
        "expires_at_ms": (int(epoch) + 31_536_000) * 1000,
        "tiles": [{"z": 1, "x": 0, "y": 0, "source": "tile.bin", "sha256": tile_sha}],
    }],
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(approval, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
chmod 0444 "$fixture/maps-approval.json"
mkdir "$fixture/guest-debs-input"
touch "$fixture/guest-debs-input/mcnf-cuttlefish-readiness-relay.deb"
touch "$fixture/guest-debs-input/mcnf-cuttlefish-vdi-agent.deb"
touch "$fixture/guest-debs-input/guest-deb-manifest.json"
printf 'immutable Cuttlefish image fixture\n' >"$fixture/cuttlefish-image.tar"
python3 "$ROOT/packaging/android/produce-image-receipt.py" --repo "$fixture/source-repo" produce \
  --source-kind artifact --original-source "$fixture/cuttlefish-image.tar" \
  --architecture amd64 --provider-identity mcnf-cuttlefish \
  --android-release-id android-15.0.0_r1 --compatibility-id mcnf-cuttlefish-v1 \
  --source-revision "$source_revision" --commit-epoch "$source_epoch" \
  --media-type application/vnd.mcnf.cuttlefish.image.v1+tar \
  --artifact-format android-cuttlefish-image-archive \
  --output "$fixture/cuttlefish-image-receipt.json" >/dev/null
python3 - "$fixture/cuttlefish-image-receipt.json" "$fixture/declaration" "$source_revision" <<'PY'
import json, sys
image = json.load(open(sys.argv[1], encoding="utf-8"))
json.dump({"schema_version":3,"kind":"cuttlefish_guest_payload_release",
           "release_id":"fixture-r1","compatibility_version":"2026.08.1",
           "source_revision":sys.argv[3],"provider_identity":"mcnf-cuttlefish",
           "image_identity":image,"artifacts":{}},
          open(sys.argv[2], "w", encoding="utf-8"), sort_keys=True, separators=(",", ":"))
PY
python3 - "$fixture/bootc-receipt.json" "$bootc_reference" "$bootc_architecture" \
  "$source_revision" "$source_epoch" "$bootc_role" <<'PY'
import json
import sys

path, reference, architecture, revision, epoch, role = sys.argv[1:]
receipt = {
    "architecture": architecture,
    "commit_epoch": int(epoch),
    "image_reference": reference,
    "kind": "mcnf-bootc-image-digest",
    "manifest_media_type": "application/vnd.oci.image.manifest.v1+json",
    "os": "linux",
    "release_role": role,
    "resolved_digest": "sha256:" + "b" * 64,
    "schema_version": 1,
    "source_revision": revision,
}
with open(path, "w", encoding="ascii") as stream:
    json.dump(receipt, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
app_vm_platform_digest="sha256:$(printf 'a%.0s' {1..64})"
python3 - "$fixture/app-vm-manifest.json" "$app_vm_platform_digest" <<'PY'
import json
import sys

path, platform_digest = sys.argv[1:]
manifest = {
    "schemaVersion": 2,
    "mediaType": "application/vnd.oci.image.index.v1+json",
    "manifests": [{
        "digest": platform_digest,
        "platform": {"os": "linux", "architecture": "amd64"},
    }],
}
with open(path, "w", encoding="ascii") as stream:
    json.dump(manifest, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
APP_VM_TEST_REFERENCE="$app_vm_reference" APP_VM_TEST_MANIFEST="$fixture/app-vm-manifest.json" \
  python3 "$ROOT/packaging/app-vm/produce-base-image-receipt.py" \
  --repo "$fixture/source-repo" --skopeo "$fixture/app-vm-skopeo" produce \
  --image-reference "$app_vm_reference" --architecture "$app_vm_architecture" \
  --source-revision "$source_revision" --commit-epoch "$source_epoch" \
  --output "$fixture/app-vm-base-receipt.json" >/dev/null

args=(--source-revision "$source_revision" --source-epoch "$source_epoch"
  --maps-approval "$fixture/maps-approval.json"
  --maps-tile-source-root "$fixture/maps-tiles"
  --maps-quota-bytes 1024
  --maps-verifier "$fixture/maps-verifier"
  --cuttlefish-declaration "$fixture/declaration"
  --cuttlefish-readiness-relay "$fixture/relay" --cuttlefish-vdi-agent "$fixture/agent"
  --cuttlefish-guest-package "$fixture/guest-debs-input/mcnf-cuttlefish-readiness-relay.deb"
  --cuttlefish-guest-package "$fixture/guest-debs-input/mcnf-cuttlefish-vdi-agent.deb"
  --rpm-signing-identity-receipt "$fixture/signing-receipt.json"
  --bootc-base-digest-receipt "$fixture/bootc-receipt.json"
  --bootc-base-image-reference "$bootc_reference"
  --bootc-base-architecture "$bootc_architecture"
  --bootc-release-role "$bootc_role"
  --app-vm-base-image-receipt "$fixture/app-vm-base-receipt.json"
  --app-vm-base-image-reference "$app_vm_reference"
  --app-vm-base-architecture "$app_vm_architecture"
  --cuttlefish-image-receipt "$fixture/cuttlefish-image-receipt.json"
  --cuttlefish-image-source-kind artifact
  --cuttlefish-image-original-source "$fixture/cuttlefish-image.tar"
  --cuttlefish-image-architecture amd64
  --cuttlefish-provider-identity mcnf-cuttlefish
  --cuttlefish-android-release-id android-15.0.0_r1
  --cuttlefish-image-compatibility-id mcnf-cuttlefish-v1
  --cuttlefish-image-media-type application/vnd.mcnf.cuttlefish.image.v1+tar
  --cuttlefish-image-artifact-format android-cuttlefish-image-archive)
envs=(PATH="$fixture/bin:$PATH" MCNF_SOURCE_VERIFY="$fixture/source" MCNF_KIRON_VERIFY="$fixture/kiron"
  MCNF_CUTTLEFISH_VERIFY="$fixture/cuttlefish"
  MCNF_CUTTLEFISH_DEB_VERIFY="$fixture/guest-debs"
  PREFLIGHT_TEST_REVISION="$source_revision"
  PREFLIGHT_TEST_KIRON_MARKER="$fixture/kiron-revision-verified"
  PREFLIGHT_TEST_RELAY="$fixture/relay" PREFLIGHT_TEST_AGENT="$fixture/agent"
  PREFLIGHT_TEST_DEB_MARKER="$fixture/guest-debs-verified"
  MCNF_RPM_SIGNING_RECEIPT_INSPECTOR="$fixture/signing-receipt.py"
  MCNF_BOOTC_DIGEST_RECEIPT_INSPECTOR="$fixture/bootc-inspector"
  MCNF_APP_VM_BASE_RECEIPT_INSPECTOR="$fixture/app-vm-base-inspector"
  MCNF_CUTTLEFISH_IMAGE_RECEIPT_INSPECTOR="$ROOT/packaging/android/produce-image-receipt.py"
  MCNF_CUTTLEFISH_IMAGE_REPO="$fixture/source-repo"
  BOOTC_TEST_INSPECTOR="$ROOT/install-helpers/produce-bootc-digest-receipt.py"
  BOOTC_TEST_REPO="$fixture/source-repo"
  APP_VM_TEST_INSPECTOR="$ROOT/packaging/app-vm/produce-base-image-receipt.py"
  APP_VM_TEST_REPO="$fixture/source-repo" APP_VM_TEST_SKOPEO="$fixture/app-vm-skopeo"
  APP_VM_TEST_REFERENCE="$app_vm_reference" APP_VM_TEST_MANIFEST="$fixture/app-vm-manifest.json")

run_release() { env "${envs[@]}" "$PRE" "$@" && : >"$marker"; }
run_release "${args[@]}"
[[ -e "$marker" ]] || { echo 'preflight self-test: valid fixture did not reach build command' >&2; exit 1; }
[[ -e "$fixture/kiron-revision-verified" ]] || { echo 'preflight self-test: Kiron verifier did not receive the release revision' >&2; exit 1; }
[[ -e "$fixture/guest-debs-verified" ]] || { echo 'preflight self-test: deterministic guest DEB verifier was not invoked' >&2; exit 1; }
echo 'release-input-preflight: bootc receipt integration PASS (revision, epoch, architecture, role, and image reference matched)'
echo 'release-input-preflight: App VM base receipt integration PASS (revision, epoch, architecture, reference, manifest, and platform digest matched)'
rm -f "$marker"

missing_maps=()
for ((index = 0; index < ${#args[@]}; index++)); do
  if [[ ${args[index]} == --maps-approval ]]; then
    index=$((index + 1))
    continue
  fi
  missing_maps+=("${args[index]}")
done
if run_release "${missing_maps[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: missing Maps approval reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: missing Maps input mutated build state' >&2; exit 1; }

chmod 0644 "$fixture/maps-approval.json"
python3 - "$fixture/maps-approval.json" <<'PY'
import json, sys
path = sys.argv[1]
value = json.load(open(path, encoding="utf-8"))
value["source_revision"] = "f" * 40
json.dump(value, open(path, "w", encoding="utf-8"), sort_keys=True, separators=(",", ":"))
PY
chmod 0444 "$fixture/maps-approval.json"
if run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: wrong-revision Maps approval reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: wrong-revision Maps approval mutated build state' >&2; exit 1; }
chmod 0644 "$fixture/maps-approval.json"
sed -i "s/ffffffffffffffffffffffffffffffffffffffff/$source_revision/" "$fixture/maps-approval.json"
chmod 0444 "$fixture/maps-approval.json"

chmod 0644 "$fixture/maps-tiles/tile.bin"
printf 'substituted release tile\n' >"$fixture/maps-tiles/tile.bin"
chmod 0444 "$fixture/maps-tiles/tile.bin"
if run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: substituted Maps tile reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: substituted Maps tile mutated build state' >&2; exit 1; }
chmod 0644 "$fixture/maps-tiles/tile.bin"
printf 'governed release tile\n' >"$fixture/maps-tiles/tile.bin"
chmod 0444 "$fixture/maps-tiles/tile.bin"

missing_deb=()
for ((index = 0; index < ${#args[@]}; index++)); do
  if [[ ${args[index]} == --cuttlefish-guest-package \
      && ${args[index + 1]} == *mcnf-cuttlefish-readiness-relay.deb ]]; then
    index=$((index + 1))
    continue
  fi
  missing_deb+=("${args[index]}")
done
if run_release "${missing_deb[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: incomplete deterministic guest DEB set reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: incomplete guest DEBs mutated build state' >&2; exit 1; }

if run_release "${args[@]:0:${#args[@]}-2}" >/dev/null 2>&1; then
  echo 'preflight self-test: incomplete image receipt interface reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: missing input mutated build state' >&2; exit 1; }

if FAKE_VERIFIER_RC=7 run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: owning-verifier refusal reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: verifier mismatch mutated build state' >&2; exit 1; }

bad=("${args[@]}")
for ((index = 0; index < ${#bad[@]}; index++)); do
  if [[ ${bad[index]} == --bootc-base-architecture ]]; then
    bad[index + 1]=arm64
    break
  fi
done
if run_release "${bad[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: mismatched bootc receipt reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: bad bootc receipt mutated build state' >&2; exit 1; }

bad=("${args[@]}")
for ((index = 0; index < ${#bad[@]}; index++)); do
  if [[ ${bad[index]} == --app-vm-base-architecture ]]; then
    bad[index + 1]=arm64
    break
  fi
done
if run_release "${bad[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: mismatched App VM base receipt reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: bad App VM base receipt mutated build state' >&2; exit 1; }

python3 - "$fixture/app-vm-manifest.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="ascii") as stream:
    value = json.load(stream)
value["manifests"][0]["digest"] = "sha256:" + "d" * 64
with open(path, "w", encoding="ascii") as stream:
    json.dump(value, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
if run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: substituted App VM registry manifest reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: substituted App VM manifest mutated build state' >&2; exit 1; }
python3 - "$fixture/app-vm-manifest.json" "$app_vm_platform_digest" <<'PY'
import json
import sys

path, platform_digest = sys.argv[1:]
with open(path, encoding="ascii") as stream:
    value = json.load(stream)
value["manifests"][0]["digest"] = platform_digest
with open(path, "w", encoding="ascii") as stream:
    json.dump(value, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY

printf 'substituted image bytes\n' >"$fixture/cuttlefish-image.tar"
if run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: substituted Cuttlefish image reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: substituted Cuttlefish image mutated build state' >&2; exit 1; }

python3 - "$ENTRY" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8").read()
rpm = text.index("  rpm)\n")
preflight = text.index('"$RELEASE_INPUT_ARGV_LOADER" "$MCNF_RELEASE_INPUT_ARGV_FILE"', rpm)
sync = text.index('do_sync_revision "$MCNF_BUILD_SOURCE_REVISION"', rpm)
vendor = text.index('remote "./install-helpers/vendor-birthright-blobs.sh"', rpm)
build = text.index('remote "export MCNF_BUILD_SOURCE_REVISION=', rpm)
if not rpm < preflight < sync < vendor < build:
    raise SystemExit("preflight self-test: release entry can mutate before input admission")
rpm_body = text[rpm:sync]
required = (
    'MCNF_RELEASE_INPUT_ARGV_FILE',
    '--expected-source-revision',
    '--expected-source-epoch',
)
forbidden = ('MCNF_BOOTC_BASE_', 'MCNF_APP_VM_BASE_', 'MCNF_CUTTLEFISH_IMAGE_', 'preflight_args=(')
if any(item not in rpm_body for item in required) or any(item in rpm_body for item in forbidden):
    raise SystemExit("preflight self-test: canonical RPM entry does not exclusively consume the private argv document")
PY
echo 'release-input-preflight: self-test PASS (missing or mismatched receipts stop before build command)'
