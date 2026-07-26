#!/usr/bin/env bash
# Install an offline Maps region bundle into the persistent seat data root.
#
# The Maps renderer reads `/var/lib/mde/maps/<region>/{<region>.mbtiles,gazetteer.sqlite}`
# by default.  `MDE_MAPS_DIR` or `--dest-root` can override that for tests or
# operator-managed external storage.  This helper intentionally installs data;
# it does not download map content or invent a public source/license.
set -euo pipefail

DEFAULT_DEST_ROOT="${MDE_MAPS_DIR:-/var/lib/mde/maps}"

die() {
  echo "install-offline-map-region: $*" >&2
  exit 1
}

usage() {
  cat >&2 <<'USAGE'
Usage:
  install-offline-map-region --region <name> --mbtiles <file> [--gazetteer <file>] [--dest-root <dir>] [--replace]
  install-offline-map-region --from-dir <dir> [--region <name>] [--dest-root <dir>] [--replace]
  install-offline-map-region --status [--dest-root <dir>]
  install-offline-map-region --self-test

Options:
  --region <name>          Region directory/name. If omitted with --from-dir, the
                           single *.mbtiles file stem is used.
  --from-dir <dir>         Directory containing exactly one *.mbtiles and,
                           optionally, gazetteer.sqlite.
  --mbtiles <file>         Explicit MBTiles file to install.
  --gazetteer <file>       Optional SQLite FTS gazetteer to install.
  --sha256 <hex>           Expected SHA-256 for the MBTiles file.
  --gazetteer-sha256 <hex> Expected SHA-256 for the gazetteer file.
  --dest-root <dir>        Destination root. Default: MDE_MAPS_DIR or /var/lib/mde/maps.
  --replace                Replace an existing different file atomically.
USAGE
}

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

validate_sha256() {
  local path="$1" expected="$2" label="$3"
  [ -z "$expected" ] && return 0
  local actual
  actual="$(sha256_file "$path")"
  [ "$actual" = "$expected" ] || die "$label SHA-256 mismatch: expected $expected got $actual"
}

validate_regular_source() {
  local path="$1" label="$2"
  [ -n "$path" ] || die "$label path is required"
  [ ! -L "$path" ] || die "$label must not be a symlink: $path"
  [ -f "$path" ] || die "$label must be a regular file: $path"
  [ -r "$path" ] || die "$label is not readable: $path"
}

validate_region() {
  local region="$1"
  [ -n "$region" ] || die "--region is required"
  [[ "$region" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
    || die "region must be a short name, not a path: $region"
}

validate_dest_root() {
  local root="$1"
  [ -n "$root" ] || die "destination root is empty"
  [ "$root" != "/" ] || die "refusing to install into /"
}

status() {
  local root="$1"
  if [ ! -d "$root" ]; then
    echo "No offline map root at $root"
    return 0
  fi
  local rows
  rows="$(find "$root" -mindepth 2 -maxdepth 2 \( -iname '*.mbtiles' -o -name 'gazetteer.sqlite' \) -printf '%P\n' 2>/dev/null | sort || true)"
  if [ -z "$rows" ]; then
    echo "No offline map regions installed under $root"
  else
    printf '%s\n' "$rows"
  fi
}

copy_atomically() {
  local src="$1" dest="$2" label="$3" replace="$4"
  validate_regular_source "$src" "$label"
  if [ -e "$dest" ] && ! cmp -s "$src" "$dest"; then
    [ "$replace" = "1" ] || die "$dest already exists with different content; pass --replace"
  fi
  if [ -e "$dest" ] && cmp -s "$src" "$dest"; then
    echo "$label already installed: $dest"
    return 0
  fi

  local tmp
  tmp="$(mktemp "${dest}.tmp.XXXXXX")"
  cp --reflink=auto -- "$src" "$tmp"
  chmod 0644 "$tmp"
  mv -f -- "$tmp" "$dest"
  echo "installed $label: $dest"
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  self_test_tmp="$tmp"
  trap 'rm -rf "${self_test_tmp:-}"' EXIT
  mkdir -p "$tmp/src"
  printf 'mbtiles fixture\n' >"$tmp/src/east-texas.mbtiles"
  printf 'gazetteer fixture\n' >"$tmp/src/gazetteer.sqlite"
  "$0" --from-dir "$tmp/src" --dest-root "$tmp/maps"
  [ -f "$tmp/maps/east-texas/east-texas.mbtiles" ] || die "self-test missing installed mbtiles"
  [ -f "$tmp/maps/east-texas/gazetteer.sqlite" ] || die "self-test missing installed gazetteer"
  "$0" --status --dest-root "$tmp/maps" | grep -q '^east-texas/east-texas\.mbtiles$' \
    || die "self-test status did not report installed mbtiles"
  echo "install-offline-map-region self-test passed"
}

mode=install
region=""
from_dir=""
mbtiles=""
gazetteer=""
dest_root="$DEFAULT_DEST_ROOT"
mbtiles_sha256=""
gazetteer_sha256=""
replace=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --region) region="${2:-}"; shift 2 ;;
    --from-dir) from_dir="${2:-}"; shift 2 ;;
    --mbtiles) mbtiles="${2:-}"; shift 2 ;;
    --gazetteer) gazetteer="${2:-}"; shift 2 ;;
    --sha256) mbtiles_sha256="${2:-}"; shift 2 ;;
    --gazetteer-sha256) gazetteer_sha256="${2:-}"; shift 2 ;;
    --dest-root) dest_root="${2:-}"; shift 2 ;;
    --replace) replace=1; shift ;;
    --status) mode=status; shift ;;
    --self-test) self_test; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown argument: $1" ;;
  esac
done

validate_dest_root "$dest_root"

if [ "$mode" = "status" ]; then
  status "$dest_root"
  exit 0
fi

if [ -n "$from_dir" ]; then
  [ ! -L "$from_dir" ] || die "--from-dir must not be a symlink: $from_dir"
  [ -d "$from_dir" ] || die "--from-dir must be a directory: $from_dir"
  if [ -z "$mbtiles" ]; then
    mapfile -t hits < <(find "$from_dir" -maxdepth 1 -type f -iname '*.mbtiles' | sort)
    [ "${#hits[@]}" -eq 1 ] || die "--from-dir must contain exactly one *.mbtiles file"
    mbtiles="${hits[0]}"
  fi
  if [ -z "$gazetteer" ] && [ -f "$from_dir/gazetteer.sqlite" ]; then
    gazetteer="$from_dir/gazetteer.sqlite"
  fi
  if [ -z "$region" ]; then
    region="$(basename "$mbtiles")"
    region="${region%.*}"
  fi
fi

validate_region "$region"
validate_regular_source "$mbtiles" "MBTiles"
[ "${mbtiles##*.}" = "mbtiles" ] || die "MBTiles file must end in .mbtiles: $mbtiles"
validate_sha256 "$mbtiles" "$mbtiles_sha256" "MBTiles"
if [ -n "$gazetteer" ]; then
  validate_regular_source "$gazetteer" "gazetteer"
  [ "$(basename "$gazetteer")" = "gazetteer.sqlite" ] || die "gazetteer file must be named gazetteer.sqlite"
  validate_sha256 "$gazetteer" "$gazetteer_sha256" "gazetteer"
fi

dest_dir="$dest_root/$region"
install -d -m 0755 "$dest_dir"
copy_atomically "$mbtiles" "$dest_dir/$region.mbtiles" "MBTiles" "$replace"
if [ -n "$gazetteer" ]; then
  copy_atomically "$gazetteer" "$dest_dir/gazetteer.sqlite" "gazetteer" "$replace"
fi
status "$dest_root"
