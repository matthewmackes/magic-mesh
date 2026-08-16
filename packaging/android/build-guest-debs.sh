#!/usr/bin/env bash
# WL-FUNC-020 — build deterministic Debian packages from an admitted guest stage.
set -euo pipefail
umask 077

ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
readonly ROOT
SOURCE_REPO=${MCNF_SOURCE_REPO:-$ROOT}
readonly STAGE_VERIFY="$ROOT/packaging/android/stage-guest-runtime-artifacts.sh"
readonly PACKAGE_VERIFY="$ROOT/packaging/android/verify-guest-debs.sh"
readonly UNIT_DIR="$ROOT/packaging/android/debian"

fail() { echo "Cuttlefish guest DEB builder: $*" >&2; exit 2; }
valid_revision() { [[ $1 =~ ^[0-9a-f]{40}$ ]]; }

[[ $# -eq 6 && $1 == --source-revision && $3 == --stage-dir && $5 == --output-dir ]] \
    || fail "usage: $0 --source-revision FULL_GIT_REVISION --stage-dir DIRECTORY --output-dir NEW_DIRECTORY"
revision=$2 stage=$4 output=$6
valid_revision "$revision" || fail "source revision must be a full lowercase Git revision"
[[ ! -e $output && ! -L $output ]] || fail "output directory already exists"
parent=$(dirname -- "$output")
[[ -d $parent && ! -L $parent ]] || fail "output parent is missing or substituted"
for tool in dpkg-deb python3 sha256sum; do command -v "$tool" >/dev/null || fail "required tool is unavailable: $tool"; done
"$STAGE_VERIFY" --verify-stage --source-revision "$revision" --directory "$stage" >/dev/null

readarray -t identity < <(python3 - "$stage/guest-runtime-artifacts.json" "$revision" <<'PY'
import json, re, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
if d.get("source_revision") != sys.argv[2]: raise SystemExit("stage revision mismatch")
version, release = d.get("version"), d.get("release")
if not isinstance(version, str) or not re.fullmatch(r"[0-9]+(?:\.[0-9]+){2}", version): raise SystemExit("invalid version")
if not isinstance(release, str) or not re.fullmatch(r"[1-9][0-9]*\.git[0-9a-f]{12}", release): raise SystemExit("invalid release")
print(version); print(release)
PY
)
[[ ${#identity[@]} -eq 2 ]] || fail "stage build identity is invalid"
version=${identity[0]}; release=${identity[1]}; deb_version="$version-$release"
epoch=$(git -C "$SOURCE_REPO" show -s --format=%ct "$revision")
[[ $epoch =~ ^[1-9][0-9]*$ ]] || fail "commit epoch is invalid"
export SOURCE_DATE_EPOCH=$epoch TZ=UTC LC_ALL=C

work=$(mktemp -d -- "$parent/.cuttlefish-guest-debs.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
build_package() {
    local package=$1 description=$2 binary=$3 unit=$4 depends=$5 root="$work/root-$1"
    install -d -m 0755 "$root/DEBIAN" "$root/usr/libexec" "$root/lib/systemd/system"
    install -m 0555 "$stage/$binary" "$root/usr/libexec/$binary"
    install -m 0444 "$UNIT_DIR/$unit" "$root/lib/systemd/system/$unit"
    cat >"$root/DEBIAN/control" <<EOF
Package: $package
Version: $deb_version
Architecture: amd64
Maintainer: Magic Mesh Release Engineering <release@magic-mesh.invalid>
Section: admin
Priority: optional
Depends: $depends
X-MCNF-Source-Revision: $revision
X-MCNF-Build-Identity: $package-$deb_version.amd64
Description: $description
EOF
    chmod 0444 "$root/DEBIAN/control"
    find "$root" -exec touch -h -d "@$epoch" -- {} +
    dpkg-deb --build --root-owner-group -Zxz -z9 -- "$root" "$work/$package.deb" >/dev/null
}

build_package mcnf-cuttlefish-vdi-agent \
    "Magic Mesh authenticated Cuttlefish VDI guest agent" \
    mcnf-cuttlefish-vdi-agent mcnf-cuttlefish-vdi-agent.service \
    "libc6, android-tools-adb, systemd"
build_package mcnf-cuttlefish-readiness-relay \
    "Magic Mesh authenticated Cuttlefish readiness relay" \
    mcnf-cuttlefish-readiness-relay mcnf-cuttlefish-readiness-relay.service \
    "libc6, systemd, mcnf-cuttlefish-vdi-agent (= $deb_version)"

install -d -m 0755 "$work/output"
install -m 0444 "$work/mcnf-cuttlefish-vdi-agent.deb" "$work/output/"
install -m 0444 "$work/mcnf-cuttlefish-readiness-relay.deb" "$work/output/"
python3 - "$work/output" "$revision" "$version" "$release" <<'PY'
import hashlib, json, os, sys
directory, revision, version, release = sys.argv[1:]
packages=[]
for name in sorted(n for n in os.listdir(directory) if n.endswith(".deb")):
    path=os.path.join(directory,name); data=open(path,"rb").read()
    packages.append({"name":name,"sha256":"sha256:"+hashlib.sha256(data).hexdigest(),"size":len(data)})
d={"schema_version":1,"kind":"mcnf-cuttlefish-guest-deb-set","source_revision":revision,
   "version":version,"release":release,"architecture":"amd64","packages":packages}
path=os.path.join(directory,"guest-deb-manifest.json")
with open(path,"x",encoding="utf-8") as f: json.dump(d,f,sort_keys=True,separators=(",",":")); f.write("\n")
os.chmod(path,0o444)
PY
"$PACKAGE_VERIFY" --source-revision "$revision" --stage-dir "$stage" --package-dir "$work/output" >/dev/null
mv -T -- "$work/output" "$output"
trap - EXIT
rm -rf -- "$work"
echo "Cuttlefish guest DEBs built: $output"
