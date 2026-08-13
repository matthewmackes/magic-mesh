#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VERIFY=$ROOT/packaging/browser-vm/verify-profile.sh
FREEZE=$ROOT/packaging/browser-vm/release-profile.py
BUILDER=$ROOT/packaging/browser-vm/build-image.sh
IMAGE_VERIFY=$ROOT/packaging/browser-vm/verify-image.sh
REVISION=$(git -C "$ROOT" rev-parse HEAD)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

freeze_profile() {
    "$FREEZE" --repo "$ROOT" --source-revision "$REVISION" --output "$1" >/dev/null
}

expect_refused() {
    local label=$1
    shift
    if "$@" >/dev/null 2>&1; then
        echo "profile source freeze hostile test accepted $label" >&2
        exit 1
    fi
}

freeze_profile "$work/frozen.env"
"$VERIFY" --source --source-revision "$REVISION" "$work/frozen.env" >/dev/null
"$VERIFY" --template "$ROOT/packaging/browser-vm/profile.env" >/dev/null

expect_refused 'an existing output' "$FREEZE" --repo "$ROOT" \
    --source-revision "$REVISION" --output "$work/frozen.env"

mkdir "$work/writable-parent"
chmod 0770 "$work/writable-parent"
expect_refused 'a group-writable output parent' "$FREEZE" --repo "$ROOT" \
    --source-revision "$REVISION" --output "$work/writable-parent/profile.env"

stale=1123456789abcdef0123456789abcdef01234567
sed "s/$REVISION/$stale/" "$work/frozen.env" >"$work/stale.env"
chmod 0400 "$work/stale.env"
expect_refused 'a stale declared revision' "$VERIFY" --source \
    --source-revision "$REVISION" "$work/stale.env"

freeze_profile "$work/dirty.env"
sed -i '1s/$/ (modified)/' "$work/dirty.env"
chmod 0400 "$work/dirty.env"
expect_refused 'dirty profile bytes' "$VERIFY" --source \
    --source-revision "$REVISION" "$work/dirty.env"

freeze_profile "$work/substituted.env"
chmod 0600 "$work/substituted.env"
printf '# substituted release input\n' >>"$work/substituted.env"
chmod 0400 "$work/substituted.env"
expect_refused 'a substituted profile' "$VERIFY" --source \
    --source-revision "$REVISION" "$work/substituted.env"

other_revision=2123456789abcdef0123456789abcdef01234567
expect_refused 'a mismatched requested revision' "$VERIFY" --source \
    --source-revision "$other_revision" "$work/frozen.env"

dirty_repo="$work/dirty-repo"
git clone -q --no-local "$ROOT" "$dirty_repo"
printf '# mutation\n' >>"$dirty_repo/packaging/browser-vm/profile.env"
"$FREEZE" --repo "$dirty_repo" --source-revision "$REVISION" \
    --output "$work/from-dirty-repo.env" >/dev/null
cmp -s "$work/from-dirty-repo.env" "$work/frozen.env" \
    || { echo 'producer admitted dirty working-tree bytes as authority' >&2; exit 1; }

grep -Fq "MCNF_BROWSER_VM_RELEASE_PROFILE=\"\$PROFILE\"" "$BUILDER" \
    || { echo 'builder does not pass the frozen profile to static verification' >&2; exit 1; }
grep -Fq 'MCNF_BROWSER_VM_RELEASE_PROFILE:-' "$IMAGE_VERIFY" \
    || { echo 'static verifier cannot consume the frozen release profile' >&2; exit 1; }

echo 'Browser VM profile source-freeze hostile test passed'
