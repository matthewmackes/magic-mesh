#!/usr/bin/env bash
set -euo pipefail
ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
STAGER=$ROOT/packaging/android/stage-guest-runtime-artifacts.sh
revision=${MCNF_BUILD_SOURCE_REVISION:-}
if [[ -z "$revision" ]]; then
    revision=$(git -C "$ROOT" rev-parse HEAD)
fi
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

"$STAGER" --source-revision "$revision" --output-dir "$fixture/good"
"$STAGER" --verify-stage --source-revision "$revision" --directory "$fixture/good"

if "$STAGER" --verify-stage --source-revision "1111111111111111111111111111111111111111" --directory "$fixture/good" >/dev/null 2>&1; then
    echo "staging self-test accepted stale source identity" >&2; exit 1
fi
cp -a "$fixture/good" "$fixture/wrong-arch"
chmod u+w "$fixture/wrong-arch/mcnf-cuttlefish-vdi-agent"
python3 - "$fixture/wrong-arch/mcnf-cuttlefish-vdi-agent" <<'PY'
import sys
with open(sys.argv[1], "r+b") as stream:
    stream.seek(18); stream.write((183).to_bytes(2, "little"))  # EM_AARCH64
PY
chmod 0555 "$fixture/wrong-arch/mcnf-cuttlefish-vdi-agent"
if "$STAGER" --verify-stage --source-revision "$revision" --directory "$fixture/wrong-arch" >/dev/null 2>&1; then
    echo "staging self-test accepted wrong-architecture ELF" >&2; exit 1
fi
cp -a "$fixture/good" "$fixture/mutable"
chmod 0755 "$fixture/mutable/mcnf-cuttlefish-readiness-relay"
if "$STAGER" --verify-stage --source-revision "$revision" --directory "$fixture/mutable" >/dev/null 2>&1; then
    echo "staging self-test accepted mutable artifact" >&2; exit 1
fi
echo "Cuttlefish guest artifact staging hostile self-test passed"
