#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VERIFY=$ROOT/packaging/browser-vm/verify-profile.sh
REVISION=$(git -C "$ROOT" rev-parse HEAD)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

freeze_profile() {
    sed "s/^BROWSER_VM_SOURCE_COMMIT=.*/BROWSER_VM_SOURCE_COMMIT=$REVISION/" \
        "$ROOT/packaging/browser-vm/profile.env" >"$1"
    chmod 0400 "$1"
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

echo 'Browser VM profile source-freeze hostile test passed'
