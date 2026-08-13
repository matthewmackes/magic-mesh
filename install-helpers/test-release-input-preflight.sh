#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PRE="$ROOT/install-helpers/release-input-preflight.sh"
ENTRY="$ROOT/install-helpers/xcp-build.sh"
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin"
marker="$fixture/build-command-ran"
for verifier in source kiron app cuttlefish; do
  cat >"$fixture/$verifier" <<'EOF'
#!/usr/bin/env bash
exit "${FAKE_VERIFIER_RC:-0}"
EOF
  chmod 0755 "$fixture/$verifier"
done
cat >"$fixture/bin/gpg" <<'EOF'
#!/usr/bin/env bash
printf 'sec:-:4096:1:DEADBEEF:0:0:::::::23::0:\n'
printf 'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:\n'
EOF
chmod 0755 "$fixture/bin/gpg"
touch "$fixture/receipt" "$fixture/key" "$fixture/declaration" "$fixture/signature" "$fixture/relay" "$fixture/agent"

args=(--source-revision aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --source-epoch 1700000000
  --app-vm-catalog-trust-receipt "$fixture/receipt" --app-vm-catalog-trust-key "$fixture/key"
  --cuttlefish-declaration "$fixture/declaration" --cuttlefish-signature "$fixture/signature"
  --cuttlefish-readiness-relay "$fixture/relay" --cuttlefish-vdi-agent "$fixture/agent"
  --rpm-signing-fingerprint AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
  --bootc-base-digest "sha256:$(printf 'b%.0s' {1..64})"
  --app-vm-base-digest "sha256:$(printf 'c%.0s' {1..64})"
  --cuttlefish-image-digest "sha256:$(printf 'd%.0s' {1..64})")
envs=(PATH="$fixture/bin:$PATH" MCNF_SOURCE_VERIFY="$fixture/source" MCNF_KIRON_VERIFY="$fixture/kiron"
  MCNF_APP_TRUST_VERIFY="$fixture/app" MCNF_CUTTLEFISH_VERIFY="$fixture/cuttlefish")

run_release() { env "${envs[@]}" "$PRE" "$@" && : >"$marker"; }
run_release "${args[@]}"
[[ -e "$marker" ]] || { echo 'preflight self-test: valid fixture did not reach build command' >&2; exit 1; }
rm -f "$marker"

if run_release "${args[@]:0:${#args[@]}-2}" >/dev/null 2>&1; then
  echo 'preflight self-test: missing image digest reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: missing input mutated build state' >&2; exit 1; }

if FAKE_VERIFIER_RC=7 run_release "${args[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: owning-verifier refusal reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: verifier mismatch mutated build state' >&2; exit 1; }

bad=("${args[@]}")
bad[${#bad[@]}-1]="sha256:$(printf '0%.0s' {1..64})"
if run_release "${bad[@]}" >/dev/null 2>&1; then
  echo 'preflight self-test: null immutable digest reached build command' >&2; exit 1
fi
[[ ! -e "$marker" ]] || { echo 'preflight self-test: bad digest mutated build state' >&2; exit 1; }

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
PY
echo 'release-input-preflight: self-test PASS (missing, mismatched, and null inputs stop before build command)'
