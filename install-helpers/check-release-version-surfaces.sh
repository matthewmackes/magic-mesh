#!/usr/bin/env bash
# Enforce the WL-REL-001 S2 version-surface contract.
#
# Root [workspace.package].version is the only numeric release identity.
# Workspace members must resolve to that value, except the documented
# internal 0.0.0 packaging/test boundaries. Isolated browser-helper
# workspaces must mirror the root value in both manifest and lockfile;
# the isolated Maps verifier stays 0.0.0. This helper does not freeze
# source or admit release inputs.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO="$(cd "$HERE/.." && pwd)"

die() {
  printf 'check-release-version-surfaces: %s\n' "$*" >&2
  exit 1
}

usage() {
  die "usage: $0 --repo PATH [--metadata-json PATH] [--emit-matrix] | --self-test"
}

read_workspace_version() {
  local cargo_toml="$1"
  [[ -f "$cargo_toml" && ! -L "$cargo_toml" ]] \
    || die "root Cargo.toml is not a regular file: $cargo_toml"
  awk '
    $0 ~ /^\[workspace\.package\][[:space:]]*$/ { in_workspace = 1; next }
    in_workspace && $0 ~ /^\[/ { exit }
    in_workspace && $0 ~ /^[[:space:]]*version[[:space:]]*=/ {
      value = $0
      sub(/^[^"]*"/, "", value)
      sub(/".*$/, "", value)
      if (value != "") { print value; found = 1; exit }
    }
    END { if (!found) exit 1 }
  ' "$cargo_toml" || die "could not read [workspace.package].version from $cargo_toml"
}

validate_release_version() {
  [[ "${1:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

run_python_check() {
  local repo="$1" metadata_json="$2" emit_matrix="$3" workspace_version="$4"
  python3 - "$repo" "$metadata_json" "$emit_matrix" "$workspace_version" <<'PY'
import json
import pathlib
import sys

repo = pathlib.Path(sys.argv[1])
metadata_path = pathlib.Path(sys.argv[2])
emit_matrix = sys.argv[3] == "1"
workspace_version = sys.argv[4]

WORKSPACE_BOUNDARIES = {
    "mackes-transport": "0.0.0",
    "magic-fleet": "0.0.0",
    "mde-kdc-host": "0.0.0",
    "mde-kdc-proto": "0.0.0",
}

ISOLATED = (
    (
        "install-helpers/browser-vm-production-control",
        "browser-vm-production-control",
        "workspace",
        "isolated-browser-helper",
    ),
    (
        "install-helpers/browser-vm-production-control/guest-controller",
        "browser-vm-production-control-guest",
        "workspace",
        "isolated-browser-helper",
    ),
    (
        "install-helpers/serve-browser-vm-performance-rdp",
        "serve-browser-vm-performance-rdp",
        "workspace",
        "isolated-browser-helper",
    ),
    (
        "packaging/maps/verifier",
        "verify-offline-map-catalog",
        "0.0.0",
        "isolated-maps-verifier",
    ),
)

DEFERRED_ROLE_SOURCE = "mcnf-cuttlefish-guest"


def die(message):
    print(f"check-release-version-surfaces: {message}", file=sys.stderr)
    raise SystemExit(1)


def parse_toml_package_field(text, field):
    in_package = False
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("[") and line.endswith("]"):
            in_package = line == "[package]"
            continue
        if not in_package or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key.strip() != field:
            continue
        value = value.strip()
        if value.startswith('"') and value.endswith('"'):
            return value[1:-1]
    return None


def lockfile_package_version(text, name):
    current_name = None
    for raw in text.splitlines():
        line = raw.strip()
        if line == "[[package]]":
            current_name = None
            continue
        if line.startswith("name = "):
            current_name = line.split("=", 1)[1].strip().strip('"')
            continue
        if current_name == name and line.startswith("version = "):
            return line.split("=", 1)[1].strip().strip('"')
    return None


def load_metadata(path):
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        die(f"metadata JSON is not readable: {exc}")
    if not isinstance(payload, dict):
        die("metadata JSON must be an object")
    packages = payload.get("packages")
    members = payload.get("workspace_members")
    if not isinstance(packages, list) or not isinstance(members, list):
        die("metadata JSON is missing packages or workspace_members")
    return packages, set(members)


packages, member_ids = load_metadata(metadata_path)
rows = []
errors = []
seen_workspace = set()

for package in packages:
    if not isinstance(package, dict):
        die("metadata package entry is not an object")
    package_id = package.get("id")
    name = package.get("name")
    version = package.get("version")
    if package_id not in member_ids:
        continue
    if not isinstance(name, str) or not isinstance(version, str):
        die(f"workspace package is missing name/version: {package_id}")
    seen_workspace.add(name)
    expected = WORKSPACE_BOUNDARIES.get(name, workspace_version)
    if name in WORKSPACE_BOUNDARIES:
        kind = "non-release-workspace"
        source = f"crates (documented 0.0.0 boundary {name})"
    elif name == DEFERRED_ROLE_SOURCE:
        kind = "deferred-role-source"
        source = "root workspace member; not a 13.0.0 release role"
    else:
        kind = "shipped-workspace"
        source = "Cargo.toml [workspace.package].version"
    rows.append((name, source, version, expected, kind))
    if version != expected:
        errors.append(
            f"{name} resolved to {version}, expected {expected}"
        )

missing_boundaries = sorted(set(WORKSPACE_BOUNDARIES) - seen_workspace)
if missing_boundaries:
    errors.append(
        "documented 0.0.0 boundary missing from workspace metadata: "
        + ", ".join(missing_boundaries)
    )

for rel, name, expected_kind, class_name in ISOLATED:
    manifest = repo / rel / "Cargo.toml"
    lockfile = repo / rel / "Cargo.lock"
    if not manifest.is_file() or manifest.is_symlink():
        errors.append(f"isolated manifest missing: {rel}/Cargo.toml")
        continue
    if not lockfile.is_file() or lockfile.is_symlink():
        errors.append(f"isolated lockfile missing: {rel}/Cargo.lock")
        continue
    text = manifest.read_text(encoding="utf-8")
    observed = parse_toml_package_field(text, "version")
    expected = workspace_version if expected_kind == "workspace" else expected_kind
    if observed is None:
        errors.append(f"{rel} Cargo.toml has no package version")
        continue
    lock_version = lockfile_package_version(
        lockfile.read_text(encoding="utf-8"), name
    )
    source = f"{rel}/Cargo.toml + Cargo.lock"
    rows.append((name, source, observed, expected, class_name))
    if observed != expected:
        errors.append(f"{name} manifest is {observed}, expected {expected}")
    if lock_version != expected:
        errors.append(
            f"{name} lockfile is {lock_version or 'missing'}, expected {expected}"
        )

rows.sort(key=lambda row: (row[4], row[0]))
if emit_matrix:
    print("package\tsource\tobserved\texpected\tclass")
    for row in rows:
        print("\t".join(row))

if errors:
    die("; ".join(errors))

print(
    f"check-release-version-surfaces: PASS ({len(seen_workspace)} workspace "
    f"members; workspace {workspace_version}; "
    f"{len(WORKSPACE_BOUNDARIES)} documented 0.0.0 boundaries; "
    f"{len(ISOLATED)} isolated surfaces)"
)
PY
}

collect_metadata() {
  local repo="$1" dest="$2"
  (
    cd "$repo" || exit 1
    cargo metadata --no-deps --format-version 1 --manifest-path "$repo/Cargo.toml"
  ) >"$dest" || die "cargo metadata --no-deps --format-version 1 failed"
}

check_repo() {
  local repo="$1" metadata_json="${2:-}" emit_matrix="$3"
  local workspace_version tmp
  repo="$(cd "$repo" && pwd)" || die "repo path is not a directory: $1"
  workspace_version="$(read_workspace_version "$repo/Cargo.toml")"
  validate_release_version "$workspace_version" \
    || die "workspace version is not a release identity: $workspace_version"
  if [ -n "$metadata_json" ]; then
    [[ -f "$metadata_json" && ! -L "$metadata_json" ]] \
      || die "metadata JSON is not a regular file: $metadata_json"
    run_python_check "$repo" "$metadata_json" "$emit_matrix" "$workspace_version"
    return
  fi
  tmp="$(mktemp)"
  trap 'rm -f -- "$tmp"' RETURN
  collect_metadata "$repo" "$tmp"
  run_python_check "$repo" "$tmp" "$emit_matrix" "$workspace_version"
}

write_fixture_lock() {
  local path="$1" name="$2" version="$3"
  cat >"$path" <<EOF
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "$name"
version = "$version"
EOF
}

write_isolated_manifest() {
  local path="$1" name="$2" version="$3"
  cat >"$path" <<EOF
[package]
name = "$name"
version = "$version"
edition = "2021"
publish = false
EOF
}

self_test() {
  local root metadata workspace_manifest
  root="$(mktemp -d)"
  trap 'rm -rf -- "$root"' EXIT
  mkdir -p \
    "$root/install-helpers/browser-vm-production-control/guest-controller" \
    "$root/install-helpers/serve-browser-vm-performance-rdp" \
    "$root/packaging/maps/verifier"
  workspace_manifest="$root/Cargo.toml"
  cat >"$workspace_manifest" <<'EOF'
[workspace]
members = ["crates/demo"]

[workspace.package]
version = "13.0.0"
EOF
  write_isolated_manifest \
    "$root/install-helpers/browser-vm-production-control/Cargo.toml" \
    browser-vm-production-control 13.0.0
  write_fixture_lock \
    "$root/install-helpers/browser-vm-production-control/Cargo.lock" \
    browser-vm-production-control 13.0.0
  write_isolated_manifest \
    "$root/install-helpers/browser-vm-production-control/guest-controller/Cargo.toml" \
    browser-vm-production-control-guest 13.0.0
  write_fixture_lock \
    "$root/install-helpers/browser-vm-production-control/guest-controller/Cargo.lock" \
    browser-vm-production-control-guest 13.0.0
  write_isolated_manifest \
    "$root/install-helpers/serve-browser-vm-performance-rdp/Cargo.toml" \
    serve-browser-vm-performance-rdp 13.0.0
  write_fixture_lock \
    "$root/install-helpers/serve-browser-vm-performance-rdp/Cargo.lock" \
    serve-browser-vm-performance-rdp 13.0.0
  write_isolated_manifest \
    "$root/packaging/maps/verifier/Cargo.toml" \
    verify-offline-map-catalog 0.0.0
  write_fixture_lock \
    "$root/packaging/maps/verifier/Cargo.lock" \
    verify-offline-map-catalog 0.0.0

  metadata="$root/good.json"
  python3 - "$metadata" <<'PY'
import json, sys
path = sys.argv[1]
packages = []
members = []
for name, version in (
    ("mackesd", "13.0.0"),
    ("mde-role-chooser", "13.0.0"),
    ("mcnf-cuttlefish-guest", "13.0.0"),
    ("mackes-transport", "0.0.0"),
    ("magic-fleet", "0.0.0"),
    ("mde-kdc-host", "0.0.0"),
    ("mde-kdc-proto", "0.0.0"),
):
    package_id = f"{name} {version} (path+{name})"
    packages.append({"name": name, "version": version, "id": package_id})
    members.append(package_id)
json.dump({"packages": packages, "workspace_members": members}, open(path, "w"))
PY
  check_repo "$root" "$metadata" 0 >/dev/null

  python3 - "$metadata" "$root/drift.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
for package in payload["packages"]:
    if package["name"] == "mackesd":
        package["version"] = "12.1.1"
json.dump(payload, open(sys.argv[2], "w"))
PY
  if check_repo "$root" "$root/drift.json" 0 >/dev/null 2>"$root/drift.err"; then
    die "self-test accepted a drifted shipped workspace version"
  fi
  grep -q 'mackesd resolved to 12.1.1' "$root/drift.err" \
    || die "self-test did not refuse a drifted shipped workspace version"

  python3 - "$metadata" "$root/boundary.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
payload["packages"] = [
    package for package in payload["packages"] if package["name"] != "magic-fleet"
]
payload["workspace_members"] = [
    member for member in payload["workspace_members"] if "magic-fleet" not in member
]
json.dump(payload, open(sys.argv[2], "w"))
PY
  if check_repo "$root" "$root/boundary.json" 0 >/dev/null 2>"$root/boundary.err"; then
    die "self-test accepted a missing 0.0.0 boundary"
  fi
  grep -q 'magic-fleet' "$root/boundary.err" \
    || die "self-test did not refuse a missing 0.0.0 boundary"

  write_isolated_manifest \
    "$root/install-helpers/browser-vm-production-control/Cargo.toml" \
    browser-vm-production-control 12.0.0
  if check_repo "$root" "$metadata" 0 >/dev/null 2>"$root/helper.err"; then
    die "self-test accepted a drifted isolated helper version"
  fi
  grep -q 'browser-vm-production-control manifest is 12.0.0' "$root/helper.err" \
    || die "self-test did not refuse a drifted isolated helper version"

  rm -rf -- "$root"
  trap - EXIT
  printf 'check-release-version-surfaces: self-test passed (clean matrix; drifted shipped, missing boundary, and drifted isolated helper fail closed)\n'
}

REPO=""
METADATA_JSON=""
EMIT_MATRIX=0
while [ $# -gt 0 ]; do
  case "$1" in
    --repo)
      [ $# -ge 2 ] || usage
      REPO="$2"
      shift 2
      ;;
    --metadata-json)
      [ $# -ge 2 ] || usage
      METADATA_JSON="$2"
      shift 2
      ;;
    --emit-matrix)
      EMIT_MATRIX=1
      shift
      ;;
    --self-test)
      [ $# -eq 1 ] || usage
      self_test
      exit 0
      ;;
    *)
      usage
      ;;
  esac
done

[ -n "$REPO" ] || REPO="$DEFAULT_REPO"
check_repo "$REPO" "$METADATA_JSON" "$EMIT_MATRIX"
