#!/usr/bin/env bash
# Build locked, unsigned Fedora 44 Surface userspace RPMs in the pinned builder.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/packaging/surface/surface-build-inputs.f44.json"
INPUTS=""
OUTPUT=""
PACKAGE="iptsd"

usage() {
    echo "Usage: $0 --inputs DIR --output NEW-DIR [--package PACKAGE]"
    echo "The input directory must be produced by fetch-surface-build-inputs.sh."
    echo "PACKAGE: iptsd, libwacom-surface, surface-control, or surface-secureboot"
}

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
        --package)
            (($# >= 2)) || { usage >&2; exit 2; }
            PACKAGE=$2
            shift 2
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

[[ -n "$INPUTS" && -d "$INPUTS" ]] || { echo "--inputs must name a directory" >&2; exit 2; }
[[ -n "$OUTPUT" ]] || { echo "--output is required" >&2; exit 2; }
case "$PACKAGE" in
    iptsd|libwacom-surface|surface-control|surface-secureboot) ;;
    *) echo "unsupported Surface userspace package: $PACKAGE" >&2; exit 2 ;;
esac
[[ ! -e "$OUTPUT" ]] || { echo "refusing to overwrite existing output: $OUTPUT" >&2; exit 2; }
output_parent="$(dirname "$OUTPUT")"
[[ -d "$output_parent" ]] || { echo "output parent does not exist: $output_parent" >&2; exit 2; }

for command in podman python3 sha256sum tar; do
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

with open(sys.argv[1], encoding="utf-8") as stream:
    lock = json.load(stream)
root = Path(sys.argv[2])
expected = {row["filename"]: row["sha256"] for row in lock["inputs"]}
expected.update({
    "build-input-lock.json": hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest(),
})
entries = {path.name: path for path in root.iterdir()}
if set(entries) != set(expected) | {"SHA256SUMS"}:
    raise SystemExit("input bundle has missing or extra directory entries")
for filename in set(expected) | {"SHA256SUMS"}:
    path = entries[filename]
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"input bundle entry is not a regular non-symlink file: {filename}")
for filename, wanted in expected.items():
    hasher = hashlib.sha256()
    with entries[filename].open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    if digest != wanted:
        raise SystemExit(f"input bundle SHA-256 mismatch: {filename}")
PY
(
    cd "$INPUTS"
    sha256sum --strict --check SHA256SUMS
) >/dev/null

readarray -t build_metadata < <(python3 - "$LOCK" "$PACKAGE" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    lock = json.load(stream)
package = sys.argv[2]
mapping = next(row for row in lock["packages"] if row["name"] == package)
expected = {
    "iptsd": ["iptsd"],
    "libwacom-surface": ["libwacom-surface", "libwacom-upstream"],
    "surface-control": ["surface-control"],
    "surface-secureboot": ["secureboot-mok", "surface-certificate"],
}
if mapping["input_ids"] != expected[package]:
    raise SystemExit(f"unexpected {package} source mapping")
items = {row["id"]: row for row in lock["inputs"]}
print(lock["builder_image"])
for input_id in mapping["input_ids"]:
    item = items[input_id]
    print("\t".join((input_id, item["filename"], item["commit"])))
PY
)
builder_image=${build_metadata[0]}
source_rows=("${build_metadata[@]:1}")

primary_source=""
primary_commit=""
declare -A source_filenames=()
for row in "${source_rows[@]}"; do
    IFS=$'\t' read -r input_id filename commit <<< "$row"
    [[ -n "$input_id" && -n "$filename" && -n "$commit" ]] \
        || { echo "incomplete locked source metadata for $PACKAGE" >&2; exit 1; }
    source_filenames["$input_id"]=$filename
    if [[ -z "$primary_source" ]]; then
        primary_source=$input_id
        primary_commit=$commit
    fi
done

stage="$(mktemp -d "$output_parent/.surface-userspace-build.XXXXXX")"
chmod 0700 "$stage"
cleanup_stage() {
    if [[ -n "${stage:-}" && -d "$stage" ]]; then
        find "$stage" -depth -delete
    fi
}
trap cleanup_stage EXIT

mkdir "$stage/source" "$stage/result"
case "$PACKAGE" in
    iptsd|surface-control)
        tar --extract --gzip --file "$INPUTS/${source_filenames[$primary_source]}" \
            --directory "$stage/source" --strip-components=1 \
            --no-same-owner --no-same-permissions
        ;;
    libwacom-surface)
        tar --extract --gzip --file "$INPUTS/${source_filenames[libwacom-surface]}" \
            --directory "$stage/source" --strip-components=1 \
            --no-same-owner --no-same-permissions
        cp "$INPUTS/${source_filenames[libwacom-upstream]}" \
            "$stage/source/pkg/fedora/libwacom-2.17.0.tar.xz"
        ;;
    surface-secureboot)
        tar --extract --gzip --file "$INPUTS/${source_filenames[secureboot-mok]}" \
            --directory "$stage/source" --strip-components=1 \
            --no-same-owner --no-same-permissions
        cp "$INPUTS/${source_filenames[surface-certificate]}" \
            "$stage/source/fedora/surface.cer"
        ;;
esac

# The source is immutable, but rpkg creates its build tree and RPMs beside it.
# DNF needs normal rootless-container capabilities to install build dependencies.
podman run --rm --pull=never \
    --env "SURFACE_PACKAGE=$PACKAGE" \
    --volume "$stage/source:/src:Z" \
    "$builder_image" \
    bash -ceu '
        case "$SURFACE_PACKAGE" in
            iptsd)
                cd /src
                bash .github/scripts/pkg-fedora.sh install
                # GitHub source archives contain the locked tree but no .git
                # metadata; rpkg and the upstream helper require a repository.
                git init --quiet
                git config user.name "MCNF Surface Builder"
                git config user.email "surface-builder@invalid"
                git add --all
                GIT_AUTHOR_DATE=2000-01-01T00:00:00Z \
                    GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
                    git commit --quiet --message "locked source archive"
                bash .github/scripts/pkg-fedora.sh build
                ;;
            libwacom-surface)
                spec=/src/pkg/fedora/libwacom-surface.spec
                dnf -y install rpm-build rpmdevtools dnf5-plugins
                dnf -y builddep "$spec"
                mkdir -p /src/pkg/fedora/.build /src/pkg/fedora/out
                rpmbuild -ba \
                    --define "_sourcedir /src/pkg/fedora" \
                    --define "_builddir /src/pkg/fedora/.build" \
                    --define "_srcrpmdir /src/pkg/fedora/out" \
                    --define "_rpmdir /src/pkg/fedora/out" \
                    --define "_specdir /src/pkg/fedora" \
                    "$spec"
                ;;
            surface-control)
                spec=/src/pkg/fedora/surface-control.spec
                dnf -y install rpm-build rpmdevtools dnf5-plugins
                dnf -y builddep "$spec"
                mkdir -p /src/pkg/fedora/.build /src/pkg/fedora/out
                find /src -mindepth 1 -maxdepth 1 ! -name pkg \
                    -exec cp -a -t /src/pkg/fedora/.build -- {} +
                rpmbuild -ba \
                    --define "_sourcedir /src/pkg/fedora/.build" \
                    --define "_builddir /src/pkg/fedora/.build" \
                    --define "_srcrpmdir /src/pkg/fedora/out" \
                    --define "_rpmdir /src/pkg/fedora/out" \
                    --define "_specdir /src/pkg/fedora" \
                    "$spec"
                ;;
            surface-secureboot)
                spec=/src/fedora/surface-secureboot.spec
                dnf -y install rpm-build rpmdevtools dnf5-plugins
                dnf -y builddep "$spec"
                mkdir -p /src/fedora/.build /src/fedora/out
                rpmbuild -ba \
                    --define "_sourcedir /src/fedora" \
                    --define "_builddir /src/fedora/.build" \
                    --define "_srcrpmdir /src/fedora/out" \
                    --define "_rpmdir /src/fedora/out" \
                    --define "_specdir /src/fedora" \
                    "$spec"
                ;;
        esac
        rpm -qa --qf "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n" \
            | LC_ALL=C sort -u > /src/.mcnf-build-environment-nevra.txt
        test "$(wc -l < /src/.mcnf-build-environment-nevra.txt)" -ge 1
        test "$(wc -l < /src/.mcnf-build-environment-nevra.txt)" -le 4096
        test "$(wc -c < /src/.mcnf-build-environment-nevra.txt)" -le 1048576
    '

mapfile -t rpms < <(find "$stage/source" -type f -name '*.rpm' -printf '%p\n' | LC_ALL=C sort)
mapfile -t rpm_names < <(printf '%s\n' "${rpms[@]}" | xargs -r -n1 basename | LC_ALL=C sort)
case "$PACKAGE" in
    iptsd)
        expected_rpms=(
            iptsd-3.1.0-1.fc44.src.rpm
            iptsd-3.1.0-1.fc44.x86_64.rpm
        )
        ;;
    libwacom-surface)
        expected_rpms=(
            libwacom-surface-2.17.0-1.fc44.src.rpm
            libwacom-surface-2.17.0-1.fc44.x86_64.rpm
            libwacom-surface-data-2.17.0-1.fc44.noarch.rpm
            libwacom-surface-devel-2.17.0-1.fc44.x86_64.rpm
            libwacom-surface-utils-2.17.0-1.fc44.x86_64.rpm
        )
        ;;
    surface-control)
        expected_rpms=(
            surface-control-0.5.0-1.fc44.src.rpm
            surface-control-0.5.0-1.fc44.x86_64.rpm
        )
        ;;
    surface-secureboot)
        expected_rpms=(
            surface-secureboot-20251230-1.fc44.noarch.rpm
            surface-secureboot-20251230-1.fc44.src.rpm
        )
        ;;
esac
if [[ "${rpm_names[*]}" != "${expected_rpms[*]}" ]]; then
    printf 'unexpected %s RPM artifact set\nexpected: %s\nactual:   %s\n' \
        "$PACKAGE" "${expected_rpms[*]}" "${rpm_names[*]}" >&2
    exit 1
fi
for rpm in "${rpms[@]}"; do
    cp "$rpm" "$stage/result/"
done
cp "$stage/source/.mcnf-build-environment-nevra.txt" \
    "$stage/result/build-environment-rpm-nevra.txt"

python3 - "$stage/result" "$PACKAGE" "$primary_commit" "$builder_image" \
    "${source_rows[@]}" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
rpms = sorted(path.name for path in root.glob("*.rpm"))
sources = []
for row in sys.argv[5:]:
    input_id, filename, commit = row.split("\t")
    sources.append({"id": input_id, "filename": filename, "commit": commit})
manifest = {
    "schema_version": 1,
    "kind": "mcnf-surface-userspace-build",
    "target": {"os": "fedora", "release": 44, "arch": "x86_64"},
    "package": sys.argv[2],
    "source_commit": sys.argv[3],
    "source_inputs": sources,
    "builder_image": sys.argv[4],
    "build_environment": {
        "dependency_resolution": "live Fedora repositories at build time",
        "installed_rpm_inventory": "build-environment-rpm-nevra.txt",
    },
    "signed": False,
    "artifacts": rpms,
}
(root / "build-manifest.json").write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY
(
    cd "$stage/result"
    mapfile -t artifacts < <(find . -maxdepth 1 -type f ! -name SHA256SUMS \
        -printf '%f\n' | LC_ALL=C sort)
    sha256sum "${artifacts[@]}" > SHA256SUMS
)
chmod 0600 "$stage/result"/*
mv "$stage/result" "$OUTPUT"
find "$stage" -depth -delete
stage=""
trap - EXIT
echo "Unsigned Fedora 44 Surface RPMs built and verified: $OUTPUT"
