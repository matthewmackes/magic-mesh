#!/usr/bin/env bash
# verify-media-lighthouse.sh — MEDIA-LIGHTHOUSE live smoke.
#
# Non-mutating by default:
#   * materializes the `media-spaces` secret into a root-only temp env file,
#     or reads MCNF_MEDIA_ENV_FILE when verifying from a provisioned media node,
#   * verifies `music.mesh` and `music-writer.mesh` resolution,
#   * pings both Navidrome/Subsonic endpoints with the shared account.
#
# With `--mutate-playlist`, it also creates, lists, fetches, and deletes a
# temporary playlist through `music-writer.mesh`, proving playlist reads/writes
# hit the same deterministic writer endpoint. Passwords stay in root-only temp
# files and curl config files, not argv.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SECRET_SCRIPT="${MCNF_SECRET_SCRIPT:-$ROOT/automation/secrets/mcnf-secret.sh}"
SECRET_NAME="${MCNF_MEDIA_SECRET_NAME:-media-spaces}"
MEDIA_ENV_FILE="${MCNF_MEDIA_ENV_FILE:-}"
MUSIC_URL="${MCNF_MUSIC_URL:-http://music.mesh:4533}"
WRITER_URL="${MDE_MUSIC_WRITER_URL:-${MCNF_MUSIC_WRITER_URL:-http://music-writer.mesh:4533}}"
MUTATE=0

log() { printf '==> media-verify: %s\n' "$*" >&2; }
die() { printf 'media-verify: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<USAGE
Usage: $0 [--mutate-playlist] [--self-test]

Verifies MEDIA-LIGHTHOUSE DNS + shared-account reachability. The playlist
mutation proof is opt-in because it writes a temporary Navidrome playlist.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --mutate-playlist) MUTATE=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown arg: $1" ;;
  esac
done

require_key() {
  local file="$1" key="$2"
  if ! grep -Eq "^${key}=" "$file"; then
    die "secret '$SECRET_NAME' missing $key"
  fi
}

materialize_secret() {
  local out="$1"
  if [ -n "$MEDIA_ENV_FILE" ]; then
    [ -r "$MEDIA_ENV_FILE" ] || die "MCNF_MEDIA_ENV_FILE is not readable: $MEDIA_ENV_FILE"
    cp "$MEDIA_ENV_FILE" "$out"
    chmod 600 "$out"
    require_key "$out" ND_ADMIN_USER
    require_key "$out" ND_ADMIN_PASS
    return 0
  fi
  [ -x "$SECRET_SCRIPT" ] || die "secret helper not executable: $SECRET_SCRIPT"
  "$SECRET_SCRIPT" get "$SECRET_NAME" >"$out"
  chmod 600 "$out"
  require_key "$out" ND_ADMIN_USER
  require_key "$out" ND_ADMIN_PASS
}

resolved_ipv4_count() {
  local host="$1"
  (getent ahostsv4 "$host" 2>/dev/null || true) \
    | awk '{print $1}' \
    | sort -u \
    | sed '/^$/d' \
    | wc -l
}

write_curl_cfg() {
  local cfg="$1" base="$2" view="$3" payload="$4"
  cat >"$cfg" <<EOF
url = "${base%/}/rest/${view}.view"
get
silent
show-error
fail
data-urlencode = "u=$ND_ADMIN_USER"
data-urlencode = "p=$ND_ADMIN_PASS"
data-urlencode = "v=1.16.1"
data-urlencode = "c=mcnf-media-verify"
data-urlencode = "f=json"
EOF
  if [ -n "$payload" ]; then
    cat "$payload" >>"$cfg"
  fi
}

curl_json() {
  local base="$1" view="$2" payload="${3:-}" cfg out
  cfg="$(mktemp "$TMPDIR/curl.XXXXXX")"
  out="$(mktemp "$TMPDIR/resp.XXXXXX")"
  chmod 600 "$cfg" "$out"
  write_curl_cfg "$cfg" "$base" "$view" "$payload"
  curl --config "$cfg" >"$out"
  printf '%s\n' "$out"
}

assert_subsonic_ok() {
  local file="$1" label="$2"
  python3 - "$file" "$label" <<'PY'
import json
import sys

path, label = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
resp = data.get("subsonic-response", {})
if resp.get("status") != "ok":
    raise SystemExit(f"{label}: Subsonic status is not ok: {resp!r}")
PY
}

playlist_id_by_name() {
  local file="$1" name="$2"
  python3 - "$file" "$name" <<'PY'
import json
import sys

path, needle = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
playlists = data.get("subsonic-response", {}).get("playlists", {}).get("playlist", [])
if isinstance(playlists, dict):
    playlists = [playlists]
for playlist in playlists:
    if playlist.get("name") == needle:
        print(playlist.get("id", ""))
        break
PY
}

self_test() {
  local tmp cfg payload
  tmp="$(mktemp -d)"
  TMPDIR="$tmp"
  ND_ADMIN_USER="admin"
  ND_ADMIN_PASS="secret with spaces"
  export ND_ADMIN_USER ND_ADMIN_PASS TMPDIR
  payload="$tmp/payload"
  cat >"$payload" <<'EOF'
data-urlencode = "name=mcnf verify"
EOF
  cfg="$tmp/curl.cfg"
  write_curl_cfg "$cfg" "http://music-writer.mesh:4533/" "createPlaylist" "$payload"
  grep -q 'url = "http://music-writer.mesh:4533/rest/createPlaylist.view"' "$cfg"
  grep -q 'data-urlencode = "name=mcnf verify"' "$cfg"
  grep -q 'data-urlencode = "p=secret with spaces"' "$cfg"
  rm -rf "$tmp"
  echo "verify-media-lighthouse: self-test passed"
}

if [ "${SELF_TEST:-0}" = 1 ]; then
  self_test
  exit 0
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT
ENV_FILE="$TMPDIR/media.env"
materialize_secret "$ENV_FILE"
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

for key in ND_ADMIN_USER ND_ADMIN_PASS; do
  [ -n "${!key:-}" ] || die "secret '$SECRET_NAME' has empty $key"
done

read_count="$(resolved_ipv4_count music.mesh)"
writer_count="$(resolved_ipv4_count music-writer.mesh)"
[ "$read_count" -ge 1 ] || die "music.mesh has no IPv4 answers"
[ "$writer_count" -eq 1 ] || die "music-writer.mesh must resolve to exactly one IPv4 answer (got $writer_count)"
log "DNS ok: music.mesh=$read_count answer(s), music-writer.mesh=$writer_count answer"

read_ping="$(curl_json "$MUSIC_URL" ping)"
assert_subsonic_ok "$read_ping" "$MUSIC_URL ping"
writer_ping="$(curl_json "$WRITER_URL" ping)"
assert_subsonic_ok "$writer_ping" "$WRITER_URL ping"
log "Subsonic ping ok on read and writer endpoints"

if [ "$MUTATE" -eq 1 ]; then
  name="mcnf-verify-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  payload="$TMPDIR/create-playlist.payload"
  printf 'data-urlencode = "name=%s"\n' "$name" >"$payload"
  created="$(curl_json "$WRITER_URL" createPlaylist "$payload")"
  assert_subsonic_ok "$created" "createPlaylist"
  listed="$(curl_json "$WRITER_URL" getPlaylists)"
  assert_subsonic_ok "$listed" "getPlaylists"
  playlist_id="$(playlist_id_by_name "$listed" "$name")"
  [ -n "$playlist_id" ] || die "created playlist '$name' not visible on writer"
  get_payload="$TMPDIR/get-playlist.payload"
  printf 'data-urlencode = "id=%s"\n' "$playlist_id" >"$get_payload"
  fetched="$(curl_json "$WRITER_URL" getPlaylist "$get_payload")"
  assert_subsonic_ok "$fetched" "getPlaylist"
  delete_payload="$TMPDIR/delete-playlist.payload"
  printf 'data-urlencode = "id=%s"\n' "$playlist_id" >"$delete_payload"
  deleted="$(curl_json "$WRITER_URL" deletePlaylist "$delete_payload")"
  assert_subsonic_ok "$deleted" "deletePlaylist"
  log "playlist mutation proof ok: created/read/deleted temporary playlist"
else
  log "playlist mutation proof skipped (pass --mutate-playlist to arm)"
fi

log "MEDIA-LIGHTHOUSE verify complete"
