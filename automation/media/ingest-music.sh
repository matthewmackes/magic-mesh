#!/usr/bin/env bash
# MEDIA-9 — upload music to the shared Spaces bucket and trigger Navidrome scans.
#
# Secrets are materialized into root-only temp files and passed to rclone/curl via
# config files, not command-line arguments. The source directory/file is copied to
# the shared bucket path; then every resolved music endpoint is asked to rescan.
set -euo pipefail

REPO="${MCNF_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
SECRET_SCRIPT="${MCNF_SECRET_SCRIPT:-$REPO/automation/secrets/mcnf-secret.sh}"
SECRET_NAME="${MCNF_MEDIA_SECRET_NAME:-media-spaces}"
DEFAULT_REMOTE_NAME="${MCNF_MEDIA_REMOTE_NAME:-spaces}"
DEFAULT_DEST_PREFIX="${MCNF_MEDIA_DEST_PREFIX:-music}"
DEFAULT_MUSIC_URL="${MCNF_MUSIC_URL:-http://music.mesh:4533}"

usage() {
  cat <<USAGE
usage: $0 [--dest-prefix PREFIX] [--skip-rescan] [--rescan-url URL ...] <source-path>

Uploads <source-path> to the shared DO Spaces music bucket from the media-spaces
mesh secret, then triggers Navidrome startScan on each endpoint.

Options:
  --dest-prefix PREFIX  Bucket prefix under DO_SPACES_BUCKET (default: music)
  --skip-rescan         Upload only; do not call Navidrome startScan
  --rescan-url URL      Explicit Navidrome base URL. Repeatable. Default is
                        http://music.mesh:4533 plus any resolved music.mesh IPs.
  --self-test           Run script self-tests and exit
USAGE
}

log() { echo "==> media-ingest: $*"; }
die() { echo "media-ingest: $*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

media_secret_to_env() {
  local out="$1"
  [ -x "$SECRET_SCRIPT" ] || die "secret helper not executable: $SECRET_SCRIPT"
  umask 077
  "$SECRET_SCRIPT" get "$SECRET_NAME" >"$out"
  chmod 600 "$out"
}

require_env_keys() {
  local missing=0 key
  for key in DO_SPACES_KEY DO_SPACES_SECRET DO_SPACES_ENDPOINT DO_SPACES_REGION \
             DO_SPACES_BUCKET ND_ADMIN_USER ND_ADMIN_PASS; do
    if [ -z "${!key:-}" ]; then
      echo "media-ingest: secret missing $key" >&2
      missing=1
    fi
  done
  [ "$missing" -eq 0 ] || exit 1
}

write_rclone_config() {
  local out="$1"
  umask 077
  cat >"$out" <<EOF
[$DEFAULT_REMOTE_NAME]
type = s3
provider = DigitalOcean
access_key_id = $DO_SPACES_KEY
secret_access_key = $DO_SPACES_SECRET
endpoint = $DO_SPACES_ENDPOINT
region = $DO_SPACES_REGION
acl = private
EOF
  chmod 600 "$out"
}

object_dest() {
  local source="$1" prefix="$2" base
  base="$(basename "$source")"
  prefix="${prefix#/}"
  prefix="${prefix%/}"
  if [ -n "$prefix" ]; then
    printf '%s:%s/%s/%s\n' "$DEFAULT_REMOTE_NAME" "$DO_SPACES_BUCKET" "$prefix" "$base"
  else
    printf '%s:%s/%s\n' "$DEFAULT_REMOTE_NAME" "$DO_SPACES_BUCKET" "$base"
  fi
}

append_unique_url() {
  local url="$1" file="$2"
  [ -n "$url" ] || return 0
  grep -Fx -- "$url" "$file" >/dev/null 2>&1 || printf '%s\n' "$url" >>"$file"
}

discover_rescan_urls() {
  local out="$1" explicit_count="$2"
  if [ "$explicit_count" -eq 0 ]; then
    append_unique_url "$DEFAULT_MUSIC_URL" "$out"
  fi
  if command -v getent >/dev/null 2>&1; then
    getent ahostsv4 music.mesh 2>/dev/null \
      | awk '{print $1}' \
      | sort -u \
      | while read -r ip; do
          [ -n "$ip" ] && append_unique_url "http://$ip:4533" "$out"
        done
  fi
}

write_curl_config() {
  local out="$1" url="$2"
  umask 077
  cat >"$out" <<EOF
url = "$url/rest/startScan.view"
fail
silent
show-error
request = "POST"
data-urlencode = "u=$ND_ADMIN_USER"
data-urlencode = "p=$ND_ADMIN_PASS"
data-urlencode = "v=1.16.1"
data-urlencode = "c=mcnf-media-ingest"
data-urlencode = "f=json"
connect-timeout = 5
max-time = 20
EOF
  chmod 600 "$out"
}

trigger_rescan() {
  local url="$1" cfg="$2"
  write_curl_config "$cfg" "$url"
  curl --config "$cfg" >/dev/null
}

upload_source() {
  local source="$1" dest="$2" config="$3"
  if [ -d "$source" ]; then
    rclone copy "$source" "$dest" --config "$config" --progress
  else
    rclone copyto "$source" "$dest" --config "$config" --progress
  fi
}

self_test() {
  local tmp envfile rclone_cfg urls curl_cfg
  tmp="$(mktemp -d)"
  envfile="$tmp/media.env"
  cat >"$envfile" <<'EOF'
DO_SPACES_KEY=k
DO_SPACES_SECRET=s
DO_SPACES_ENDPOINT=nyc3.digitaloceanspaces.com
DO_SPACES_REGION=nyc3
DO_SPACES_BUCKET=mcnf-mesh-media
ND_ADMIN_USER=mesh
ND_ADMIN_PASS=secret
EOF
  set -a
  # shellcheck disable=SC1090
  . "$envfile"
  set +a
  require_env_keys
  rclone_cfg="$tmp/rclone.conf"
  write_rclone_config "$rclone_cfg"
  grep -q 'provider = DigitalOcean' "$rclone_cfg"
  [ "$(object_dest "/tmp/Album One" "music")" = "spaces:mcnf-mesh-media/music/Album One" ]
  urls="$tmp/urls"
  : >"$urls"
  append_unique_url "http://music.mesh:4533" "$urls"
  append_unique_url "http://music.mesh:4533" "$urls"
  [ "$(wc -l <"$urls")" -eq 1 ]
  curl_cfg="$tmp/curl.conf"
  write_curl_config "$curl_cfg" "http://10.42.0.20:4533"
  grep -q 'startScan.view' "$curl_cfg"
  grep -q 'data-urlencode = "u=mesh"' "$curl_cfg"
  rm -rf "$tmp"
  log "self-test passed"
}

DEST_PREFIX="$DEFAULT_DEST_PREFIX"
SKIP_RESCAN=0
SOURCE=""
EXPLICIT_URLS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --dest-prefix) DEST_PREFIX="$2"; shift 2;;
    --skip-rescan) SKIP_RESCAN=1; shift;;
    --rescan-url) EXPLICIT_URLS+=("$2"); shift 2;;
    --self-test) self_test; exit 0;;
    -h|--help) usage; exit 0;;
    --*) die "unknown option: $1";;
    *) SOURCE="$1"; shift;;
  esac
done

[ -n "$SOURCE" ] || { usage >&2; exit 2; }
[ -e "$SOURCE" ] || die "source path not found: $SOURCE"

need_cmd rclone
need_cmd curl

TMPDIR="$(mktemp -d)"
cleanup() { rm -rf "$TMPDIR"; }
trap cleanup EXIT

ENV_FILE="$TMPDIR/media.env"
RCLONE_CONFIG="$TMPDIR/rclone.conf"
URLS_FILE="$TMPDIR/rescan-urls"

media_secret_to_env "$ENV_FILE"
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a
require_env_keys
write_rclone_config "$RCLONE_CONFIG"

DEST="$(object_dest "$SOURCE" "$DEST_PREFIX")"
log "uploading $(basename "$SOURCE") to $DEST"
upload_source "$SOURCE" "$DEST" "$RCLONE_CONFIG"

if [ "$SKIP_RESCAN" -eq 1 ]; then
  log "rescan skipped"
  exit 0
fi

: >"$URLS_FILE"
for url in "${EXPLICIT_URLS[@]}"; do
  append_unique_url "$url" "$URLS_FILE"
done
discover_rescan_urls "$URLS_FILE" "${#EXPLICIT_URLS[@]}"

if [ ! -s "$URLS_FILE" ]; then
  die "no rescan endpoints found"
fi

failures=0
while read -r url; do
  [ -n "$url" ] || continue
  cfg="$TMPDIR/curl-$(echo "$url" | tr -c 'A-Za-z0-9' '_').conf"
  if trigger_rescan "$url" "$cfg"; then
    log "rescan triggered at $url"
  else
    echo "media-ingest: rescan failed at $url" >&2
    failures=$((failures + 1))
  fi
done <"$URLS_FILE"

[ "$failures" -eq 0 ] || exit 1
log "done"
