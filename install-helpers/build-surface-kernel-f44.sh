#!/usr/bin/bash
# Build the locked Fedora 44 linux-surface kernel without persisting its signer.
set -euo pipefail
PATH=/usr/bin:/bin
export PATH

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/packaging/surface/surface-build-inputs.f44.json"
INPUTS=""
OUTPUT=""
PRIVATE_KEY=""
CERTIFICATE=""
CHECK_ONLY=0
MIN_SCRATCH_KIB=$((45 * 1024 * 1024))
MIN_OUTPUT_KIB=$((12 * 1024 * 1024))
MIN_MEMORY_KIB=$((8 * 1024 * 1024))

usage() {
    cat <<'USAGE'
Usage: build-surface-kernel-f44.sh --inputs ABS-DIR --output ABS-NEW-DIR \
       --private-key ABS-FILE --certificate ABS-FILE [--check]
       build-surface-kernel-f44.sh --self-test

The certificate must be byte-for-byte identical to the certificate in the
governed input lock and the private key must match it. --check performs all
readiness checks without creating the output or starting a build.
USAGE
}

self_test() {
    local work="$1" script="$2" output
    mkdir -p "$work/inputs" "$work/existing"
    printf 'key\n' >"$work/key"
    printf 'certificate\n' >"$work/cert"
    chmod 0600 "$work/key" "$work/cert"
    ln -s "$work/key" "$work/key-link"

    reject() {
        local label="$1" expected="$2"
        shift 2
        if output="$($script "$@" 2>&1)"; then
            echo "self-test accepted hostile fixture: $label" >&2
            exit 1
        fi
        [[ "$output" == *"$expected"* ]] || {
            echo "self-test returned the wrong failure for $label: $output" >&2
            exit 1
        }
    }

    reject "unknown option" "Usage:" --surprise
    reject "missing arguments" "four required absolute paths" --check
    reject "relative input" "must be absolute" \
        --inputs relative --output "$work/new" --private-key "$work/key" \
        --certificate "$work/cert" --check
    reject "existing output" "refusing to overwrite" \
        --inputs "$work/inputs" --output "$work/existing" \
        --private-key "$work/key" --certificate "$work/cert" --check
    reject "symlink private key" "non-symlink regular file" \
        --inputs "$work/inputs" --output "$work/new" \
        --private-key "$work/key-link" --certificate "$work/cert" --check
    chmod 0644 "$work/key"
    reject "permissive private key" "private key permissions" \
        --inputs "$work/inputs" --output "$work/new" \
        --private-key "$work/key" --certificate "$work/cert" --check
    chmod 0600 "$work/key"
    reject "colon-bearing credential path" "must not contain a colon" \
        --inputs "$work/inputs" --output "$work/new" \
        --private-key "$work/key" --certificate "$work/cert:bad" --check
    mkdir "$work/fakebin"
    cat >"$work/fakebin/stat" <<FAKE
#!/usr/bin/bash
printf invoked >'$work/fake-command-invoked'
exit 99
FAKE
    chmod 0700 "$work/fakebin/stat"
    if output="$(PATH="$work/fakebin:/usr/bin:/bin" "$script" \
        --inputs "$work/inputs" --output "$work/new" \
        --private-key "$work/key" --certificate "$work/cert" --check 2>&1)"; then
        echo "self-test accepted incomplete bundle" >&2
        exit 1
    fi
    [[ "$output" == *"input bundle lock does not match"* && ! -e "$work/fake-command-invoked" ]] \
        || { echo "self-test did not reject inherited PATH: $output" >&2; exit 1; }
    python3 - "$script" <<'PY'
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text(encoding="utf-8")
phase_one_marker = "# Phase 1:" + " networked dependency preparation with no source or credential mounts."
phase_two_marker = "# Phase 2:" + " locked source build with credentials and the network disabled."
end_marker = "mapfile -t " + "rpms < <(find"
if any(source.count(marker) != 1 for marker in (phase_one_marker, phase_two_marker, end_marker)):
    raise SystemExit("self-test could not locate the unique container phase boundaries")
phase_one_start = source.index(phase_one_marker)
phase_two_start = source.index(phase_two_marker, phase_one_start)
phase_end = source.index(end_marker, phase_two_start)
phase_one = source[phase_one_start:phase_two_start]
phase_two = source[phase_two_start:phase_end]
for forbidden in ("$PRIVATE_KEY", "$CERTIFICATE", "$scratch:/work", "/credentials/", "--volume"):
    if forbidden in phase_one:
        raise SystemExit(f"networked dependency phase exposes forbidden material: {forbidden}")
if 'podman commit --pause=false "$deps_container" "$deps_image"' not in phase_one:
    raise SystemExit("networked dependency phase does not commit its ephemeral image")
header_end = phase_two.find('"$deps_image" bash -ceu')
if header_end < 0:
    raise SystemExit("offline build phase does not consume the ephemeral dependency image")
header = phase_two[:header_end]
required = (
    "--network=none",
    '"LOCKED_ARK_BRANCH=$ark_branch"',
    '"LOCKED_ARK_TAG=$ark_tag"',
    '"LOCKED_ARK_UPSTREAM_TAG=$ark_upstream_tag"',
    '"LOCKED_ARK_VERSION=$ark_version"',
    '"$scratch:/work"',
    '"$PRIVATE_KEY:/credentials/MOK.key:ro"',
    '"$CERTIFICATE:/credentials/MOK.crt:ro"',
)
if any(value not in header for value in required):
    raise SystemExit("key-bearing build invocation is not offline with exact read-only mounts")
if 'git init --quiet --initial-branch=build/locked' not in phase_two:
    raise SystemExit("offline kernel-ark worktree does not isolate the patched build branch")
if 'git branch "$LOCKED_ARK_BRANCH" HEAD' not in phase_two:
    raise SystemExit("offline kernel-ark worktree does not restore the locked upstream branch")
if 'git tag --annotate --message "locked Fedora ${LOCKED_ARK_TAG}" "$LOCKED_ARK_TAG"' not in phase_two:
    raise SystemExit("offline kernel-ark worktree does not restore the locked upstream tag")
if 'git tag --annotate --message "locked upstream ${LOCKED_ARK_UPSTREAM_TAG}" "$LOCKED_ARK_UPSTREAM_TAG"' not in phase_two:
    raise SystemExit("offline kernel-ark worktree does not restore the upstream source tag")
if 'rpm -qp --queryformat "%{VERSION}\\n" "${srpms[0]}"' not in phase_two:
    raise SystemExit("offline kernel build does not verify SRPM version against the locked tag")
if 'chr(34) + "MOK.key"' not in phase_two:
    raise SystemExit("offline kernel packaging does not remove the signing key from kernel-devel")
if 'artifact="$(realpath -e "$artifact")"' not in phase_two:
    raise SystemExit("offline kernel artifact scan does not canonicalize paths before changing directories")
print("Surface kernel container phase-boundary assertions passed")
PY
    echo "Surface kernel builder self-test passed (9 hostile/structural fixtures rejected)"
}

if [[ "${1:-}" == "--self-test" ]]; then
    (($# == 1)) || { usage >&2; exit 2; }
    test_root="$(mktemp -d /tmp/surface-kernel-selftest.XXXXXX)"
    trap 'find "$test_root" -depth -delete' EXIT
    self_test "$test_root" "$(realpath -e "$0")"
    exit 0
fi

while (($#)); do
    case "$1" in
        --inputs)
            (($# >= 2)) || { usage >&2; exit 2; }
            INPUTS=$2
            shift 2
            ;;
        --output)
            (($# >= 2)) || { usage >&2; exit 2; }
            OUTPUT=$2
            shift 2
            ;;
        --private-key)
            (($# >= 2)) || { usage >&2; exit 2; }
            PRIVATE_KEY=$2
            shift 2
            ;;
        --certificate)
            (($# >= 2)) || { usage >&2; exit 2; }
            CERTIFICATE=$2
            shift 2
            ;;
        --check)
            CHECK_ONLY=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

[[ -n "$INPUTS" && -n "$OUTPUT" && -n "$PRIVATE_KEY" && -n "$CERTIFICATE" ]] \
    || { echo "four required absolute paths were not provided" >&2; exit 2; }
for path in "$INPUTS" "$OUTPUT" "$PRIVATE_KEY" "$CERTIFICATE"; do
    [[ "$path" == /* ]] || { echo "all supplied paths must be absolute" >&2; exit 2; }
    [[ "$path" != *:* ]] || { echo "supplied paths must not contain a colon" >&2; exit 2; }
    [[ "$path" != *$'\n'* && "$path" != *$'\r'* ]] \
        || { echo "supplied paths must not contain control characters" >&2; exit 2; }
done
[[ -d "$INPUTS" && ! -L "$INPUTS" ]] \
    || { echo "--inputs must be a non-symlink directory" >&2; exit 2; }
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] \
    || { echo "refusing to overwrite existing output: $OUTPUT" >&2; exit 2; }
output_parent="$(dirname "$OUTPUT")"
[[ -d "$output_parent" && ! -L "$output_parent" ]] \
    || { echo "output parent must be a non-symlink directory" >&2; exit 2; }
[[ -f "$PRIVATE_KEY" && ! -L "$PRIVATE_KEY" && -r "$PRIVATE_KEY" ]] \
    || { echo "private key must be a readable non-symlink regular file" >&2; exit 2; }
[[ -f "$CERTIFICATE" && ! -L "$CERTIFICATE" && -r "$CERTIFICATE" ]] \
    || { echo "certificate must be a readable non-symlink regular file" >&2; exit 2; }
[[ "$(stat -c '%u' "$PRIVATE_KEY")" == "$(id -u)" ]] \
    || { echo "private key must be owned by the invoking user" >&2; exit 2; }
[[ "$(stat -c '%h' "$PRIVATE_KEY")" == 1 ]] \
    || { echo "private key must not have additional hard links" >&2; exit 2; }
resolved_private_key="$(realpath -e "$PRIVATE_KEY")"
[[ "$resolved_private_key" != "$ROOT" && "$resolved_private_key" != "$ROOT/"* ]] \
    || { echo "private key must be stored outside the repository" >&2; exit 2; }
key_mode=$((8#$(stat -c '%a' "$PRIVATE_KEY")))
(( (key_mode & 077) == 0 )) \
    || { echo "private key permissions must deny group and other access" >&2; exit 2; }
key_size="$(stat -c '%s' "$PRIVATE_KEY")"
cert_size="$(stat -c '%s' "$CERTIFICATE")"
((key_size > 0 && key_size <= 65536)) \
    || { echo "private key size is outside the 1..65536-byte bound" >&2; exit 2; }
((cert_size > 0 && cert_size <= 65536)) \
    || { echo "certificate size is outside the 1..65536-byte bound" >&2; exit 2; }

for command in cpio df find openssl podman python3 realpath rpm rpm2cpio sha256sum stat tar; do
    command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }
done

"$ROOT/install-helpers/fetch-surface-build-inputs.sh" --lock "$LOCK" >/dev/null
cmp -s "$LOCK" "$INPUTS/build-input-lock.json" \
    || { echo "input bundle lock does not match the governed repository lock" >&2; exit 1; }
python3 - "$LOCK" "$INPUTS" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

lock = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
root = Path(sys.argv[2])
expected = {row["filename"]: row["sha256"] for row in lock["inputs"]}
expected["build-input-lock.json"] = hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest()
entries = {path.name: path for path in root.iterdir()}
if set(entries) != set(expected) | {"SHA256SUMS"}:
    raise SystemExit("input bundle has missing or extra directory entries")
for name in set(expected) | {"SHA256SUMS"}:
    if entries[name].is_symlink() or not entries[name].is_file():
        raise SystemExit(f"input bundle entry is not a regular non-symlink file: {name}")
for name, wanted in expected.items():
    hasher = hashlib.sha256()
    with entries[name].open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    if digest != wanted:
        raise SystemExit(f"input bundle SHA-256 mismatch: {name}")
PY
(
    cd "$INPUTS"
    sha256sum --strict --check SHA256SUMS
) >/dev/null

readarray -t build_metadata < <(python3 - "$LOCK" <<'PY'
import json
import sys

lock = json.load(open(sys.argv[1], encoding="utf-8"))
mapping = next(row for row in lock["packages"] if row["name"] == "kernel-surface")
if mapping["input_ids"] != ["linux-surface", "kernel-ark", "surface-certificate"]:
    raise SystemExit("unexpected kernel-surface source mapping")
items = {row["id"]: row for row in lock["inputs"]}
print(lock["builder_image"])
for ident in mapping["input_ids"]:
    item = items[ident]
    print("\t".join((ident, item["filename"], item["ref"], item["commit"], item["sha256"])))
PY
)
builder_image=${build_metadata[0]}
IFS=$'\t' read -r _ linux_filename linux_ref linux_commit linux_sha <<<"${build_metadata[1]}"
IFS=$'\t' read -r _ ark_filename ark_ref ark_commit ark_sha <<<"${build_metadata[2]}"
IFS=$'\t' read -r _ cert_filename cert_ref cert_commit cert_sha <<<"${build_metadata[3]}"
[[ "$ark_ref" =~ ^refs/tags/kernel-([0-9]+)\.([0-9]+)\.([0-9]+)-[0-9]+$ ]] \
    || { echo "locked kernel-ark ref cannot derive the Fedora release branch" >&2; exit 1; }
ark_branch="linux-${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.y"
ark_version="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}"
ark_tag="${ark_ref#refs/tags/}"
ark_upstream_tag="v$ark_version"

actual_cert_sha="$(sha256sum "$CERTIFICATE" | awk '{print $1}')"
[[ "$actual_cert_sha" == "$cert_sha" ]] \
    || { echo "provided certificate does not match the locked Surface certificate" >&2; exit 1; }
cmp -s "$CERTIFICATE" "$INPUTS/$cert_filename" \
    || { echo "provided certificate is not byte-identical to the locked Surface certificate" >&2; exit 1; }
cert_public_sha="$(openssl x509 -in "$CERTIFICATE" -pubkey -noout 2>/dev/null \
    | openssl pkey -pubin -outform DER 2>/dev/null | sha256sum | awk '{print $1}')" \
    || { echo "locked Surface certificate could not be parsed" >&2; exit 1; }
key_public_sha="$(openssl pkey -in "$PRIVATE_KEY" -passin pass: -pubout -outform DER 2>/dev/null \
    | sha256sum | awk '{print $1}')" \
    || { echo "private key could not be parsed non-interactively" >&2; exit 1; }
[[ -n "$cert_public_sha" && "$cert_public_sha" != "$(printf '' | sha256sum | awk '{print $1}')" ]] \
    || { echo "locked Surface certificate could not be parsed" >&2; exit 1; }
[[ "$key_public_sha" == "$cert_public_sha" ]] \
    || { echo "private key does not match the locked Surface certificate" >&2; exit 1; }

podman image exists "$builder_image" \
    || { echo "digest-pinned Fedora 44 builder image is not present; pre-pull the exact locked digest" >&2; exit 2; }
[[ -d /var/tmp && ! -L /var/tmp ]] \
    || { echo "/var/tmp must be a non-symlink scratch directory" >&2; exit 2; }
scratch_free="$(df -Pk /var/tmp | awk 'NR == 2 {print $4}')"
output_free="$(df -Pk "$output_parent" | awk 'NR == 2 {print $4}')"
memory_free="$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)"
((scratch_free >= MIN_SCRATCH_KIB)) \
    || { echo "kernel build requires at least 45 GiB free under /var/tmp (available: $((scratch_free / 1024 / 1024)) GiB)" >&2; exit 2; }
((output_free >= MIN_OUTPUT_KIB)) \
    || { echo "kernel output requires at least 12 GiB free (available: $((output_free / 1024 / 1024)) GiB)" >&2; exit 2; }
((memory_free >= MIN_MEMORY_KIB)) \
    || { echo "kernel build requires at least 8 GiB available memory" >&2; exit 2; }

if ((CHECK_ONLY)); then
    echo "Surface kernel build readiness passed: locked inputs, signer, image, memory, and storage"
    exit 0
fi

scratch="$(mktemp -d /var/tmp/mcnf-surface-kernel.XXXXXX)"
result_stage="$(mktemp -d "$output_parent/.surface-kernel-result.XXXXXX")"
deps_container=""
deps_image=""
deps_container_owned=0
deps_image_owned=0
chmod 0700 "$scratch" "$result_stage"
cleanup() {
    if (( ${deps_container_owned:-0} )); then podman rm --force "$deps_container" >/dev/null 2>&1 || true; fi
    if (( ${deps_image_owned:-0} )); then podman image rm --force "$deps_image" >/dev/null 2>&1 || true; fi
    if [[ -n "${scratch:-}" && -d "$scratch" ]]; then find "$scratch" -depth -delete; fi
    if [[ -n "${result_stage:-}" && -d "$result_stage" ]]; then find "$result_stage" -depth -delete; fi
}
trap cleanup EXIT
mkdir "$scratch/linux-surface" "$scratch/kernel-ark"

python3 "$ROOT/install-helpers/verify-surface-source-archive.py" \
    "$INPUTS/$linux_filename" "$INPUTS/$ark_filename"
tar --extract --gzip --file "$INPUTS/$linux_filename" --directory "$scratch/linux-surface" \
    --strip-components=1 --no-same-owner --no-same-permissions
tar --extract --gzip --file "$INPUTS/$ark_filename" --directory "$scratch/kernel-ark" \
    --strip-components=1 --no-same-owner --no-same-permissions

# The pinned upstream helper otherwise fetches and resets kernel-ark. Replace
# only that audited block in the ephemeral tree, then consume the verified
# archive worktree with no source-network operation.
python3 - "$scratch/linux-surface/pkg/fedora/kernel-surface/build-ark.py" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
source = path.read_text(encoding="utf-8")
start_marker = "# Clone the kernel-ark repository if it doesn't exist."
end_marker = "# Apply patches"
new = '''# MCNF producer: args.ark_dir is the already hash-verified locked archive.
if not os.path.isdir(os.path.join(args.ark_dir, ".git")):
    raise SystemExit("locked kernel-ark worktree is not initialized")
os.chdir(args.ark_dir)
system("git clean -dfx")
'''
if source.count(start_marker) != 1 or source.count(end_marker) != 1:
    raise SystemExit("pinned build-ark.py no longer matches the audited offline patch")
start = source.index(start_marker)
end = source.index(end_marker, start)
path.write_text(source[:start] + new + "\n" + source[end:], encoding="utf-8")
PY

build_token="$(printf '%s-%s-%s' "$scratch" "$$" "$RANDOM" \
    | sha256sum | awk '{print substr($1, 1, 24)}')"
deps_container="mcnf-surface-deps-$build_token"
deps_image="localhost/mcnf-surface-builddeps:$build_token"

# Phase 1: networked dependency preparation with no source or credential mounts.
podman container exists "$deps_container" \
    && { echo "refusing colliding ephemeral dependency container name" >&2; exit 1; }
podman image exists "$deps_image" \
    && { echo "refusing colliding ephemeral dependency image name" >&2; exit 1; }
podman create --name "$deps_container" --pull=never "$builder_image" bash -ceu '
    dnf -y distro-sync
    dnf -y install @rpm-development-tools git rpm-sign sbsigntools cpio
    dnf -y builddep kernel
' >/dev/null
deps_container_owned=1
podman start --attach "$deps_container"
[[ "$(podman inspect --format '{{.State.ExitCode}}' "$deps_container")" == 0 ]] \
    || { echo "Surface build-dependency preparation container failed" >&2; exit 1; }
podman commit --pause=false "$deps_container" "$deps_image" >/dev/null
deps_image_owned=1
podman rm "$deps_container" >/dev/null
deps_container=""
deps_container_owned=0
podman image exists "$deps_image" \
    || { echo "ephemeral Surface build-dependency image was not committed" >&2; exit 1; }

# Phase 2: locked source build with credentials and the network disabled.
podman run --rm --pull=never \
    --network=none \
    --security-opt label=disable \
    --env "LOCKED_ARK_COMMIT=$ark_commit" \
    --env "LOCKED_ARK_BRANCH=$ark_branch" \
    --env "LOCKED_ARK_TAG=$ark_tag" \
    --env "LOCKED_ARK_UPSTREAM_TAG=$ark_upstream_tag" \
    --env "LOCKED_ARK_VERSION=$ark_version" \
    --volume "$scratch:/work" \
    --volume "$PRIVATE_KEY:/credentials/MOK.key:ro" \
    --volume "$CERTIFICATE:/credentials/MOK.crt:ro" \
    "$deps_image" bash -ceu '
        cd /work/kernel-ark
        git init --quiet --initial-branch=build/locked
        git config user.name "MCNF Surface Builder"
        git config user.email "surface-builder@invalid"
        git add --all
        GIT_AUTHOR_DATE=2000-01-01T00:00:00Z \
        GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
            git commit --quiet --message "locked kernel-ark archive ${LOCKED_ARK_COMMIT}"
        git branch "$LOCKED_ARK_BRANCH" HEAD
        GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
            git tag --annotate --message "locked Fedora ${LOCKED_ARK_TAG}" "$LOCKED_ARK_TAG"
        GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
            git tag --annotate --message "locked upstream ${LOCKED_ARK_UPSTREAM_TAG}" "$LOCKED_ARK_UPSTREAM_TAG"

        secureboot=/work/linux-surface/pkg/fedora/kernel-surface/secureboot
        install -m 0600 /credentials/MOK.key "$secureboot/MOK.key"
        install -m 0600 /credentials/MOK.crt "$secureboot/MOK.crt"
        python3 - <<\PY
from pathlib import Path

build_ark = Path("/work/linux-surface/pkg/fedora/kernel-surface/build-ark.py")
source = build_ark.read_text(encoding="utf-8")
anchor = "# Copy files" + chr(10)
body = [
    "from pathlib import Path",
    "spec = Path(args.ark_dir) / " + chr(34) + "redhat/kernel.spec.template" + chr(34),
    "spec_source = spec.read_text(encoding=" + chr(34) + "utf-8" + chr(34) + ")",
    "needle = " + chr(34) + "    # prune junk from kernel-devel" + chr(34) + " + chr(10)",
    "cleanup = needle + " + chr(39) + "    find $RPM_BUILD_ROOT/usr/src/kernels -type f -name " + chr(34) + "MOK.key" + chr(34) + " -delete" + chr(39) + " + chr(10)",
    "if spec_source.count(needle) < 1:",
    "    raise SystemExit(" + chr(34) + "kernel spec cleanup anchor is missing" + chr(34) + ")",
    "spec.write_text(spec_source.replace(needle, cleanup), encoding=" + chr(34) + "utf-8" + chr(34) + ")",
]
injection = chr(10).join(body) + chr(10)
if source.count(anchor) != 1:
    raise SystemExit("build-ark copy-files anchor is not unique")
build_ark.write_text(source.replace(anchor, injection + anchor, 1), encoding="utf-8")
PY
        cd /work/linux-surface/pkg/fedora/kernel-surface
        python3 build-linux-surface.py --mode srpm --ark-dir /work/kernel-ark --outdir srpm
        find /work/kernel-ark -depth -delete
        mapfile -d "" srpms < <(find srpm -type f -name "*.src.rpm" -print0)
        test "${#srpms[@]}" -eq 1
        test "$(rpm -qp --queryformat "%{VERSION}\n" "${srpms[0]}")" = "$LOCKED_ARK_VERSION"
        rpmbuild -rb --define "_topdir ${PWD}/rpmbuild" \
            --define "_rpmdir ${PWD}/out" "${srpms[0]}"

        mapfile -d "" binaries < <(find out -type f -name "*.rpm" ! -name "*.src.rpm" -print0)
        test "${#binaries[@]}" -ge 1
        test "${#binaries[@]}" -le 64
        for artifact in "${binaries[@]}"; do
            artifact="$(realpath -e "$artifact")"
            rpm -qpl "$artifact" | grep -Eiq "(MOK[.]key|private[-_.]?key)" && {
                echo "binary RPM payload exposes private-key material" >&2
                exit 1
            }
            scan="$(mktemp -d /work/key-scan.XXXXXX)"
            (cd "$scan" && rpm2cpio "$artifact" | cpio -idm --quiet)
            while IFS= read -r -d "" payload; do
                if cmp -s /credentials/MOK.key "$payload"; then
                    echo "binary RPM payload contains the supplied private key" >&2
                    exit 1
                fi
            done < <(find "$scan" -type f -print0)
            find "$scan" -depth -delete
        done
        rpm -qa --qf "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n" \
            | LC_ALL=C sort -u > /work/build-environment-rpm-nevra.txt
        test "$(wc -l </work/build-environment-rpm-nevra.txt)" -le 4096
        test "$(wc -c </work/build-environment-rpm-nevra.txt)" -le 1048576
    '

mapfile -t rpms < <(find "$scratch/linux-surface/pkg/fedora/kernel-surface/out" \
    -type f -name '*.rpm' ! -name '*.src.rpm' -printf '%p\n' | LC_ALL=C sort)
(( ${#rpms[@]} >= 1 && ${#rpms[@]} <= 64 )) \
    || { echo "kernel build emitted an invalid binary RPM count" >&2; exit 1; }
for artifact in "${rpms[@]}"; do
    name="$(rpm -qp --qf '%{NAME}' "$artifact")"
    [[ "$name" == kernel-surface* ]] \
        || { echo "kernel build emitted an unexpected package name: $name" >&2; exit 1; }
    destination="$result_stage/$(basename "$artifact")"
    [[ ! -e "$destination" ]] || { echo "kernel build emitted duplicate RPM basenames" >&2; exit 1; }
    cp "$artifact" "$destination"
done
cp "$scratch/build-environment-rpm-nevra.txt" "$result_stage/"

cert_subject="$(openssl x509 -in "$CERTIFICATE" -noout -subject -nameopt RFC2253 | sed 's/^subject=//')"
cert_fingerprint="$(openssl x509 -in "$CERTIFICATE" -noout -fingerprint -sha256 | sed 's/^sha256 Fingerprint=//')"
python3 - "$result_stage" "$builder_image" "$linux_filename" "$linux_commit" "$linux_sha" \
    "$ark_filename" "$ark_commit" "$ark_sha" "$cert_filename" "$cert_commit" "$cert_sha" \
    "$cert_subject" "$cert_fingerprint" "$cert_public_sha" <<'PY'
import hashlib
import json
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
artifacts = []
for path in sorted(root.glob("*.rpm")):
    nevra = subprocess.run(
        ["rpm", "-qp", "--qf", "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}", str(path)],
        check=True, capture_output=True, text=True,
    ).stdout
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            hasher.update(chunk)
    artifacts.append({
        "filename": path.name,
        "nevra": nevra,
        "sha256": hasher.hexdigest(),
        "size_bytes": path.stat().st_size,
        "rpm_signature": "unsigned",
    })
manifest = {
    "schema_version": 1,
    "kind": "mcnf-surface-kernel-build",
    "target": {"os": "fedora", "release": 44, "arch": "x86_64"},
    "package": "kernel-surface",
    "builder_image": sys.argv[2],
    "source_inputs": [
        {"id": "linux-surface", "filename": sys.argv[3], "commit": sys.argv[4], "sha256": sys.argv[5]},
        {"id": "kernel-ark", "filename": sys.argv[6], "commit": sys.argv[7], "sha256": sys.argv[8]},
        {"id": "surface-certificate", "filename": sys.argv[9], "commit": sys.argv[10], "sha256": sys.argv[11]},
    ],
    "build_environment": {
        "dependency_resolution": "live Fedora 44 repositories at build time",
        "installed_rpm_inventory": "build-environment-rpm-nevra.txt",
        "source_fetch_policy": "locked archives only; audited upstream helper fetch disabled",
        "dependency_phase_network": "enabled without source or credential mounts",
        "key_bearing_build_phase_network": "disabled by podman --network=none",
    },
    "artifacts": artifacts,
}
(root / "build-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
signing = {
    "schema_version": 1,
    "kind": "mcnf-surface-kernel-signing",
    "certificate": {
        "source_input": sys.argv[9],
        "file_sha256": sys.argv[11],
        "public_key_sha256": sys.argv[14],
        "subject": sys.argv[12],
        "sha256_fingerprint": sys.argv[13],
    },
    "private_key": {
        "matching_public_key_verified": True,
        "persisted_in_output": False,
        "path_recorded": False,
    },
    "kernel_image": {
        "operation": "sbsign through pinned linux-surface Secure Boot spec patch",
        "certificate_bound": True,
    },
    "kernel_modules": {
        "signer_asserted": False,
        "required_follow_up": "verify module signer from every built binary RPM before promotion",
    },
    "source_rpm": {
        "published": False,
        "reason": "upstream transient SRPM embeds Source7001 MOK.key and must never leave scratch",
    },
    "rpm_packages": {"signed": False, "required_follow_up": "project release signing"},
}
(root / "signing-manifest.json").write_text(json.dumps(signing, indent=2, sort_keys=True) + "\n")
PY
(
    cd "$result_stage"
    mapfile -t files < <(find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\n' | LC_ALL=C sort)
    sha256sum "${files[@]}" >SHA256SUMS
)
chmod 0600 "$result_stage"/*
podman image rm "$deps_image" >/dev/null
deps_image=""
deps_image_owned=0
find "$scratch" -depth -delete
scratch=""
mv -T --no-clobber "$result_stage" "$OUTPUT"
[[ ! -d "$result_stage" ]] \
    || { echo "refusing output created concurrently: $OUTPUT" >&2; exit 2; }
result_stage=""
trap - EXIT
echo "Fedora 44 Surface kernel binary RPMs built; signing limits are explicit in $OUTPUT/signing-manifest.json"
