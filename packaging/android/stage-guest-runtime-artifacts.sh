#!/usr/bin/env bash
# WL-FUNC-020 — reproducibly build and stage the real guest runtime artifacts.
set -euo pipefail
umask 077

ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
readonly ROOT
SOURCE_REPO=${MCNF_SOURCE_REPO:-$ROOT}
readonly PACKAGE=mcnf-cuttlefish-guest
readonly TARGET=x86_64-unknown-linux-gnu
readonly RELAY=mcnf-cuttlefish-readiness-relay
readonly AGENT=mcnf-cuttlefish-vdi-agent

fail() { echo "Cuttlefish guest artifact staging: $*" >&2; exit 2; }
valid_revision() { [[ $1 =~ ^[0-9a-f]{40}$ ]]; }

verify_elf() {
    local file=$1 expected_name=$2 revision=$3 header machine kind identity
    [[ -f $file && ! -L $file && $(stat -c %h -- "$file") -eq 1 ]] || fail "$expected_name is not a single-link regular file"
    header=$(readelf -hW -- "$file") || fail "$expected_name is not ELF"
    machine=$(sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p' <<<"$header")
    kind=$(sed -n 's/^[[:space:]]*Type:[[:space:]]*\([^[:space:]]*\).*/\1/p' <<<"$header")
    [[ $machine == 'Advanced Micro Devices X86-64' && $kind =~ ^(DYN|EXEC)$ ]] || fail "$expected_name has the wrong ELF architecture or type"
    ! readelf -SW -- "$file" | grep -Eq '[[:space:]]\.debug(_|[[:space:]])' || fail "$expected_name retains debug sections instead of governed release stripping"
    # The executable identity probe is the governed source-revision ABI. Do
    # not infer provenance from an unreferenced string section: release linkers
    # are allowed to garbage-collect such data while preserving executable
    # behavior.
    identity=$($file --build-identity) || fail "$expected_name identity probe failed"
    [[ $identity == "$revision" ]] || fail "$expected_name reports a stale source revision"
}

verify_stage() {
    local directory=$1 revision=$2 manifest
    manifest=$directory/guest-runtime-artifacts.json
    verify_elf "$directory/$RELAY" "$RELAY" "$revision"
    verify_elf "$directory/$AGENT" "$AGENT" "$revision"
    [[ -f $manifest && ! -L $manifest ]] || fail "artifact manifest is missing"
    python3 - "$manifest" "$directory/$RELAY" "$directory/$AGENT" "$revision" <<'PY'
import hashlib, json, os, stat, sys
manifest, relay, agent, revision = sys.argv[1:]
document = json.load(open(manifest, encoding="utf-8"))
if document.get("schema_version") != 1 or document.get("source_revision") != revision:
    raise SystemExit("Cuttlefish guest artifact staging: manifest revision is stale")
if document.get("architecture") != "x86_64" or document.get("profile") != "release":
    raise SystemExit("Cuttlefish guest artifact staging: manifest build identity is invalid")
for path in (relay, agent):
    name = os.path.basename(path)
    entry = document.get("artifacts", {}).get(name)
    digest = "sha256:" + hashlib.sha256(open(path, "rb").read()).hexdigest()
    mode = stat.S_IMODE(os.stat(path, follow_symlinks=False).st_mode)
    if not entry or entry.get("sha256") != digest or entry.get("size") != os.path.getsize(path):
        raise SystemExit(f"Cuttlefish guest artifact staging: manifest does not bind {name}")
    if mode != 0o555:
        raise SystemExit(f"Cuttlefish guest artifact staging: {name} is not immutable mode 0555")
PY
}

if [[ ${1:-} == --verify-stage ]]; then
    [[ $# -eq 5 && $2 == --source-revision && $4 == --directory ]] || fail "invalid --verify-stage invocation"
    valid_revision "$3" || fail "source revision must be a full lowercase Git revision"
    verify_stage "$5" "$3"
    echo "Cuttlefish guest artifact stage verified: $5"
    exit 0
fi

[[ $# -eq 4 && $1 == --source-revision && $3 == --output-dir ]] || fail "usage: $0 --source-revision FULL_GIT_REVISION --output-dir NEW_DIRECTORY"
revision=$2
output=$4
valid_revision "$revision" || fail "source revision must be a full lowercase Git revision"
[[ $(uname -m) == x86_64 ]] || fail "canonical guest artifacts must be built on x86_64"
git -C "$SOURCE_REPO" cat-file -e "$revision^{commit}" 2>/dev/null || fail "source revision is not a local commit"
[[ ! -e $output && ! -L $output ]] || fail "output directory already exists"
parent=$(dirname -- "$output")
[[ -d $parent && ! -L $parent ]] || fail "output parent is missing or substituted"

for tool in cargo git readelf sha256sum python3; do command -v "$tool" >/dev/null || fail "required tool is unavailable: $tool"; done
source_tree=$(mktemp -d)
trap 'rm -rf -- "$source_tree"' EXIT
git -C "$SOURCE_REPO" archive "$revision" | tar -x -C "$source_tree"
export MCNF_BUILD_SOURCE_REVISION=$revision
export CARGO_TARGET_DIR=$source_tree/target
(cd "$source_tree" && cargo build --locked --release --target "$TARGET" -p "$PACKAGE" --bins)
target_dir=$CARGO_TARGET_DIR
relay_source=$target_dir/$TARGET/release/$RELAY
agent_source=$target_dir/$TARGET/release/$AGENT

work=$(mktemp -d -- "$parent/.cuttlefish-guest-stage.XXXXXX")
trap 'rm -rf -- "$source_tree" "$work"' EXIT
install -m 0555 -- "$relay_source" "$work/$RELAY"
install -m 0555 -- "$agent_source" "$work/$AGENT"
# Cargo may hard-link a top-level release binary to its internal artifact.
# Admission begins only after `install` creates private, single-link candidate
# inodes; no compiler-target alias can retain mutation authority over them.
verify_elf "$work/$RELAY" "$RELAY" "$revision"
verify_elf "$work/$AGENT" "$AGENT" "$revision"
version=$(cd "$source_tree" && cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(p["version"] for p in d["packages"] if p["name"]=="mcnf-cuttlefish-guest"))')
rustc_identity=$(rustc -Vv | tr '\n' ';' | sed 's/;$/\n/')
python3 - "$work" "$revision" "$version" "$rustc_identity" "$RELAY" "$AGENT" <<'PY'
import hashlib, json, os, sys
directory, revision, version, toolchain, *names = sys.argv[1:]
artifacts = {}
for name in names:
    path = os.path.join(directory, name)
    artifacts[name] = {
        "sha256": "sha256:" + hashlib.sha256(open(path, "rb").read()).hexdigest(),
        "size": os.path.getsize(path),
        "identity": f"{name}-{version}-1.git{revision[:12]}.x86_64",
    }
document = {"schema_version": 1, "package": "mcnf-cuttlefish-guest", "version": version,
            "release": f"1.git{revision[:12]}", "architecture": "x86_64", "profile": "release",
            "source_revision": revision, "toolchain": toolchain, "artifacts": artifacts}
with open(os.path.join(directory, "guest-runtime-artifacts.json"), "x", encoding="utf-8") as stream:
    json.dump(document, stream, sort_keys=True, separators=(",", ":")); stream.write("\n")
os.chmod(os.path.join(directory, "guest-runtime-artifacts.json"), 0o444)
PY
verify_stage "$work" "$revision"
python3 - "$work" "$output" <<'PY'
import ctypes, errno, os, sys
source, destination = map(os.fsencode, sys.argv[1:])
libc = ctypes.CDLL(None, use_errno=True)
renameat2 = libc.renameat2
renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
renameat2.restype = ctypes.c_int
if renameat2(-100, source, -100, destination, 1) != 0:
    error = ctypes.get_errno()
    raise SystemExit("Cuttlefish guest artifact staging: " + ("output exists" if error == errno.EEXIST else os.strerror(error)))
PY
rm -rf -- "$source_tree"
trap - EXIT
echo "Cuttlefish guest release artifacts staged: $output"
