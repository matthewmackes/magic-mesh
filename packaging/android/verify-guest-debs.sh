#!/usr/bin/env bash
# WL-FUNC-020 — attest DEB metadata and payload bytes against the admitted stage.
set -euo pipefail
umask 077
fail() { echo "Cuttlefish guest DEB verifier: $*" >&2; exit 2; }
[[ $# -eq 6 && $1 == --source-revision && $3 == --stage-dir && $5 == --package-dir ]] \
    || fail "usage: $0 --source-revision FULL_GIT_REVISION --stage-dir DIRECTORY --package-dir DIRECTORY"
revision=$2 stage=$4 directory=$6
[[ $revision =~ ^[0-9a-f]{40}$ ]] || fail "invalid source revision"
[[ -d $directory && ! -L $directory ]] || fail "package directory is missing or substituted"
for tool in dpkg-deb python3 sha256sum; do command -v "$tool" >/dev/null || fail "required tool unavailable: $tool"; done
manifest=$directory/guest-deb-manifest.json
[[ -f $manifest && ! -L $manifest && $(stat -c %h -- "$manifest") -eq 1 && $(stat -c %a -- "$manifest") == 444 ]] \
    || fail "package manifest identity or mode is invalid"

python3 - "$manifest" "$directory" "$revision" <<'PY'
import hashlib,json,os,stat,sys
path,directory,revision=sys.argv[1:]
d=json.load(open(path,encoding="utf-8"))
if set(d)!={"schema_version","kind","source_revision","version","release","architecture","packages"}: raise SystemExit("manifest shape is invalid")
if d["schema_version"]!=1 or d["kind"]!="mcnf-cuttlefish-guest-deb-set" or d["source_revision"]!=revision or d["architecture"]!="amd64": raise SystemExit("manifest identity is invalid")
expected=["mcnf-cuttlefish-readiness-relay.deb","mcnf-cuttlefish-vdi-agent.deb"]
if [x.get("name") for x in d["packages"]]!=expected: raise SystemExit("package set/order is invalid")
if sorted(n for n in os.listdir(directory) if n.endswith(".deb")) != expected: raise SystemExit("package directory contains an undeclared DEB")
for x in d["packages"]:
 p=os.path.join(directory,x["name"]); s=os.stat(p,follow_symlinks=False)
 if not stat.S_ISREG(s.st_mode) or s.st_nlink!=1 or stat.S_IMODE(s.st_mode)!=0o444: raise SystemExit("package inode or mode is invalid")
 data=open(p,"rb").read()
 if len(data)!=x.get("size") or "sha256:"+hashlib.sha256(data).hexdigest()!=x.get("sha256"): raise SystemExit("package digest is invalid")
PY

work=$(mktemp -d); trap 'rm -rf -- "$work"' EXIT
verify_one() {
    local package=$1 binary=$2 unit=$3 expected_dep=$4 value
    local deb="$directory/$package.deb" root="$work/$package"
    [[ $(dpkg-deb -f "$deb" Package) == "$package" ]] || fail "$package metadata name mismatch"
    [[ $(dpkg-deb -f "$deb" Architecture) == amd64 ]] || fail "$package architecture mismatch"
    [[ $(dpkg-deb -f "$deb" X-MCNF-Source-Revision) == "$revision" ]] || fail "$package source revision mismatch"
    value=$(dpkg-deb -f "$deb" X-MCNF-Build-Identity)
    [[ $value == "$package-"*.*.amd64 ]] || fail "$package build identity mismatch"
    dpkg-deb -f "$deb" Depends | grep -Fq "$expected_dep" || fail "$package dependency missing: $expected_dep"
    dpkg-deb -c "$deb" | awk -v path="./usr/libexec/$binary" \
        '$6 == path && $1 == "-r-xr-xr-x" && $2 == "root/root" { found=1 } END { exit !found }' \
        || fail "$binary archive ownership/mode invalid"
    dpkg-deb -c "$deb" | awk -v path="./lib/systemd/system/$unit" \
        '$6 == path && $1 == "-r--r--r--" && $2 == "root/root" { found=1 } END { exit !found }' \
        || fail "$unit archive ownership/mode invalid"
    mkdir "$root"; dpkg-deb -x "$deb" "$root"
    cmp -s -- "$stage/$binary" "$root/usr/libexec/$binary" || fail "$package binary differs from admitted stage: $binary"
    [[ $(stat -c %a "$root/usr/libexec/$binary") == 555 ]] || fail "$binary extracted mode invalid"
    [[ $(stat -c %a "$root/lib/systemd/system/$unit") == 444 ]] || fail "$unit extracted mode invalid"
    find "$root" -type f -printf '%P\n' | sort >"$root.files"
    printf 'lib/systemd/system/%s\nusr/libexec/%s\n' "$unit" "$binary" | sort >"$root.expected"
    cmp -s "$root.files" "$root.expected" || fail "$package payload contains undeclared files"
}
verify_one mcnf-cuttlefish-vdi-agent mcnf-cuttlefish-vdi-agent mcnf-cuttlefish-vdi-agent.service android-tools-adb
version=$(dpkg-deb -f "$directory/mcnf-cuttlefish-vdi-agent.deb" Version)
verify_one mcnf-cuttlefish-readiness-relay mcnf-cuttlefish-readiness-relay mcnf-cuttlefish-readiness-relay.service "mcnf-cuttlefish-vdi-agent (= $version)"
echo "Cuttlefish guest DEB set verified: $directory"
