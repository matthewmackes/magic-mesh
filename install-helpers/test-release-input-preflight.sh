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
for verifier in source kiron app cuttlefish; do
  cat >"$fixture/$verifier" <<'EOF'
#!/usr/bin/env bash
exit "${FAKE_VERIFIER_RC:-0}"
EOF
  chmod 0755 "$fixture/$verifier"
done
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
cat >"$fixture/bin/gpg" <<'EOF'
#!/usr/bin/env bash
printf 'sec:-:4096:1:DEADBEEF:0:0:::::::23::0:\n'
printf 'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:\n'
EOF
chmod 0755 "$fixture/bin/gpg"
touch "$fixture/receipt" "$fixture/key" "$fixture/signature" "$fixture/relay" "$fixture/agent" "$fixture/signing-receipt.json"
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

args=(--source-revision "$source_revision" --source-epoch "$source_epoch"
  --app-vm-catalog-trust-receipt "$fixture/receipt" --app-vm-catalog-trust-key "$fixture/key"
  --cuttlefish-declaration "$fixture/declaration" --cuttlefish-signature "$fixture/signature"
  --cuttlefish-readiness-relay "$fixture/relay" --cuttlefish-vdi-agent "$fixture/agent"
  --rpm-signing-identity-receipt "$fixture/signing-receipt.json"
  --bootc-base-digest-receipt "$fixture/bootc-receipt.json"
  --bootc-base-image-reference "$bootc_reference"
  --bootc-base-architecture "$bootc_architecture"
  --bootc-release-role "$bootc_role"
  --app-vm-base-digest "sha256:$(printf 'c%.0s' {1..64})"
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
  MCNF_APP_TRUST_VERIFY="$fixture/app" MCNF_CUTTLEFISH_VERIFY="$fixture/cuttlefish"
  MCNF_RPM_SIGNING_RECEIPT_INSPECTOR="$fixture/signing-receipt.py"
  MCNF_BOOTC_DIGEST_RECEIPT_INSPECTOR="$fixture/bootc-inspector"
  MCNF_CUTTLEFISH_IMAGE_RECEIPT_INSPECTOR="$ROOT/packaging/android/produce-image-receipt.py"
  MCNF_CUTTLEFISH_IMAGE_REPO="$fixture/source-repo"
  BOOTC_TEST_INSPECTOR="$ROOT/install-helpers/produce-bootc-digest-receipt.py"
  BOOTC_TEST_REPO="$fixture/source-repo")

run_release() { env "${envs[@]}" "$PRE" "$@" && : >"$marker"; }
run_release "${args[@]}"
[[ -e "$marker" ]] || { echo 'preflight self-test: valid fixture did not reach build command' >&2; exit 1; }
echo 'release-input-preflight: bootc receipt integration PASS (revision, epoch, architecture, role, and image reference matched)'
rm -f "$marker"

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

printf 'substituted image bytes\n' >"$fixture/cuttlefish-image.tar"
if run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: substituted Cuttlefish image reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: substituted Cuttlefish image mutated build state' >&2; exit 1; }

python3 - "$ENTRY" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8").read()
rpm = text.index("  rpm)\n")
preflight = text.index('"$RELEASE_INPUT_PREFLIGHT" "${preflight_args[@]}"', rpm)
sync = text.index('do_sync_revision "$MCNF_BUILD_SOURCE_REVISION"', rpm)
vendor = text.index('remote "./install-helpers/vendor-birthright-blobs.sh"', rpm)
build = text.index('remote "export MCNF_BUILD_SOURCE_REVISION=', rpm)
if not rpm < preflight < sync < vendor < build:
    raise SystemExit("preflight self-test: release entry can mutate before input admission")
rpm_body = text[rpm:sync]
required = (
    'MCNF_BOOTC_BASE_DIGEST_RECEIPT',
    'MCNF_BOOTC_BASE_IMAGE_REFERENCE',
    'MCNF_BOOTC_BASE_ARCHITECTURE',
    'MCNF_BOOTC_RELEASE_ROLE',
    'MCNF_CUTTLEFISH_IMAGE_RECEIPT',
    'MCNF_CUTTLEFISH_IMAGE_ORIGINAL_SOURCE',
    'MCNF_CUTTLEFISH_IMAGE_ARCHITECTURE',
    'MCNF_CUTTLEFISH_PROVIDER_IDENTITY',
    'MCNF_CUTTLEFISH_ANDROID_RELEASE_ID',
    'MCNF_CUTTLEFISH_IMAGE_COMPATIBILITY_ID',
)
if any(item not in rpm_body for item in required) or 'MCNF_BOOTC_BASE_DIGEST:-' in rpm_body or 'MCNF_CUTTLEFISH_IMAGE_DIGEST:-' in rpm_body:
    raise SystemExit("preflight self-test: canonical RPM entry does not exclusively consume governed image receipts")
PY
echo 'release-input-preflight: self-test PASS (missing or mismatched receipts stop before build command)'
