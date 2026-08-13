#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HELPER=$ROOT/install-helpers/build-release-derivative-images.sh
REVISION=0123456789abcdef0123456789abcdef01234567
work=$(mktemp -d)
trap 'chmod -R u+rwX -- "$work" 2>/dev/null || true; rm -rf -- "$work"' EXIT
mkdir -m 0700 "$work/bin" "$work/out-parent"
APP_BASE_IMAGE="registry.invalid/app@sha256:$(printf 'a%.0s' {1..64})"
BROWSER_BASE_IMAGE="registry.invalid/browser@sha256:$(printf 'b%.0s' {1..64})"

for name in workstation.rpm lighthouse.rpm app-candidate.json browser-candidate.json app-base.json browser-base.json trust.json trust.key release.key; do
    printf 'admitted-%s\n' "$name" >"$work/$name"; chmod 0400 "$work/$name"
done
cat >"$work/profile.env" <<EOF
BROWSER_VM_SOURCE_COMMIT=$REVISION
EOF
chmod 0400 "$work/profile.env"

cat >"$work/bin/app-verify" <<'EOF'
#!/bin/sh
printf 'app-verify\n' >>"$CALLS"
exit "${FAIL_APP_VERIFY:-0}"
EOF
cat >"$work/bin/browser-verify" <<'EOF'
#!/usr/bin/env python3
import os
with open(os.environ["CALLS"], "a", encoding="utf-8") as stream:
    stream.write("browser-verify\n")
raise SystemExit(int(os.environ.get("FAIL_BROWSER_VERIFY", "0")))
EOF
cat >"$work/bin/app-builder" <<'EOF'
#!/bin/sh
set -eu
printf 'app-builder %s\n' "$*" >>"$CALLS"
[ "${FAIL_APP_BUILD:-0}" -eq 0 ] || exit 7
while [ "$#" -gt 0 ]; do [ "$1" = --out ] && { out=$2; break; }; shift; done
mkdir -p "$out/qcow2"; printf 'QFI\373app-disk\n' >"$out/qcow2/disk.qcow2"; chmod 0400 "$out/qcow2/disk.qcow2"
EOF
cat >"$work/bin/browser-builder" <<'EOF'
#!/bin/sh
set -eu
printf 'browser-builder %s\n' "$*" >>"$CALLS"
[ "${FAIL_BROWSER_BUILD:-0}" -eq 0 ] || exit 8
while [ "$#" -gt 0 ]; do [ "$1" = --out ] && { out=$2; break; }; shift; done
mkdir -p "$out/qcow2"; printf 'browser-disk\n' >"$out/qcow2/disk.qcow2"
printf '{"verified":true}\n' >"$out/qcow2/disk.qcow2.mcnf-manifest.json"
chmod 0400 "$out/qcow2/"*
EOF
cat >"$work/bin/manifest-verify" <<'EOF'
#!/usr/bin/env python3
import os
with open(os.environ["CALLS"], "a", encoding="utf-8") as stream:
    stream.write("manifest-verify\n")
raise SystemExit(int(os.environ.get("FAIL_MANIFEST_VERIFY", "0")))
EOF
chmod 0500 "$work/bin/"*

common=(
  --source-revision "$REVISION"
  --signed-workstation-rpm "$work/workstation.rpm"
  --app-rpm-candidate-manifest "$work/app-candidate.json"
  --app-base-receipt "$work/app-base.json" --app-base-image "$APP_BASE_IMAGE"
  --app-catalog-trust-receipt "$work/trust.json" --app-catalog-trust-key "$work/trust.key"
  --signed-lighthouse-rpm "$work/lighthouse.rpm"
  --browser-rpm-candidate-manifest "$work/browser-candidate.json"
  --browser-base-receipt "$work/browser-base.json" --browser-base-image "$BROWSER_BASE_IMAGE"
)
run() {
  CALLS=$work/calls MCNF_DERIVATIVE_APP_BUILDER=$work/bin/app-builder \
  MCNF_DERIVATIVE_BROWSER_BUILDER=$work/bin/browser-builder \
  MCNF_DERIVATIVE_APP_RPM_VERIFY=$work/bin/app-verify \
  MCNF_DERIVATIVE_BROWSER_RPM_VERIFY=$work/bin/browser-verify \
  MCNF_DERIVATIVE_BROWSER_MANIFEST_VERIFY=$work/bin/manifest-verify \
  MCNF_DERIVATIVE_RELEASE_KEY=$work/release.key \
  MCNF_DERIVATIVE_BROWSER_PROFILE=$work/profile.env "$HELPER" "${common[@]}" "$@"
}

before=$(sha256sum "$work/workstation.rpm" "$work/lighthouse.rpm")
run --output "$work/out-parent/good"
[ -f "$work/out-parent/good/derivative-images.json" ]
[ -f "$work/out-parent/good/app-vm-wayland-standard.mcnf-manifest.json" ]
grep -Fq '"promotion":"forbidden"' "$work/out-parent/good/derivative-images.json"
grep -Fq 'app-builder --rpm ' "$work/calls"
grep -Fq 'browser-builder --rpm ' "$work/calls"
grep -Fq 'manifest-verify' "$work/calls"
[ "$before" = "$(sha256sum "$work/workstation.rpm" "$work/lighthouse.rpm")" ]

: >"$work/calls"
if FAIL_APP_BUILD=1 run --output "$work/out-parent/app-failed" >/dev/null 2>&1; then
  echo 'hostile test: App derivative failure was accepted' >&2; exit 1
fi
[ ! -e "$work/out-parent/app-failed" ]
if grep -Fq 'browser-builder' "$work/calls"; then
  echo 'hostile test: Browser builder ran after App derivative failure' >&2; exit 1
fi
[ "$before" = "$(sha256sum "$work/workstation.rpm" "$work/lighthouse.rpm")" ]

: >"$work/calls"
if FAIL_MANIFEST_VERIFY=1 run --output "$work/out-parent/manifest-failed" >/dev/null 2>&1; then
  echo 'hostile test: invalid Browser manifest was accepted' >&2; exit 1
fi
[ ! -e "$work/out-parent/manifest-failed" ]
[ "$before" = "$(sha256sum "$work/workstation.rpm" "$work/lighthouse.rpm")" ]

sed -i "s/$REVISION/1123456789abcdef0123456789abcdef01234567/" "$work/profile.env"
chmod 0400 "$work/profile.env"
: >"$work/calls"
if run --output "$work/out-parent/stale-profile" >/dev/null 2>&1; then
  echo 'hostile test: stale Browser profile was accepted' >&2; exit 1
fi
[ ! -s "$work/calls" ] && [ ! -e "$work/out-parent/stale-profile" ]

echo 'test-build-release-derivative-images: hostile orchestration suite passed'
