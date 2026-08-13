#!/usr/bin/env bash
set -euo pipefail
ROOT=$(git rev-parse --show-toplevel)
revision=$(git -C "$ROOT" rev-parse HEAD)
fixture=$(mktemp -d); trap 'rm -rf -- "$fixture"' EXIT
"$ROOT/packaging/android/stage-guest-runtime-artifacts.sh" --source-revision "$revision" --output-dir "$fixture/stage"
"$ROOT/packaging/android/build-guest-debs.sh" --source-revision "$revision" --stage-dir "$fixture/stage" --output-dir "$fixture/packages-a"
"$ROOT/packaging/android/build-guest-debs.sh" --source-revision "$revision" --stage-dir "$fixture/stage" --output-dir "$fixture/packages-b"
for name in mcnf-cuttlefish-readiness-relay.deb mcnf-cuttlefish-vdi-agent.deb guest-deb-manifest.json; do
    cmp -s "$fixture/packages-a/$name" "$fixture/packages-b/$name" || { echo "non-deterministic output: $name" >&2; exit 1; }
done
"$ROOT/packaging/android/verify-guest-debs.sh" --source-revision "$revision" --stage-dir "$fixture/stage" --package-dir "$fixture/packages-a"
cp -a "$fixture/packages-a" "$fixture/substituted"
chmod u+w "$fixture/substituted/mcnf-cuttlefish-vdi-agent.deb"
printf x >>"$fixture/substituted/mcnf-cuttlefish-vdi-agent.deb"
chmod 0444 "$fixture/substituted/mcnf-cuttlefish-vdi-agent.deb"
if "$ROOT/packaging/android/verify-guest-debs.sh" --source-revision "$revision" --stage-dir "$fixture/stage" --package-dir "$fixture/substituted" >/dev/null 2>&1; then
    echo "verifier admitted substituted package bytes" >&2; exit 1
fi
cp -a "$fixture/packages-a" "$fixture/wrong-stage"
chmod u+w "$fixture/wrong-stage/guest-deb-manifest.json"
sed -i "s/$revision/1111111111111111111111111111111111111111/" "$fixture/wrong-stage/guest-deb-manifest.json"
chmod 0444 "$fixture/wrong-stage/guest-deb-manifest.json"
if "$ROOT/packaging/android/verify-guest-debs.sh" --source-revision "$revision" --stage-dir "$fixture/stage" --package-dir "$fixture/wrong-stage" >/dev/null 2>&1; then
    echo "verifier admitted stale package identity" >&2; exit 1
fi
echo "Cuttlefish guest DEB hostile self-test passed"
