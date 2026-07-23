#!/bin/bash
# mcnf-music-ingest.sh — MEDIA-9 (MEDIA-LIGHTHOUSE): the operator's content path.
#
# The shared DO Spaces bucket is the SINGLE source of truth for the mesh music
# library (lock #10). This helper is the two operator verbs around it:
#
#   upload <local-path> [<dest-subpath>]
#       rclone-copy a file or directory of music INTO the bucket (the same
#       `spaces:` remote setup-media-navidrome.sh configures, reusing the
#       root-only creds — never on argv). Idempotent (`rclone copy` skips
#       unchanged objects).
#
#   rescan
#       trigger a library re-scan on EVERY explicitly provisioned non-lighthouse
#       media host so the
#       new tracks appear everywhere, via Navidrome's Subsonic `startScan.view`
#       API. Instances are discovered by resolving the `music.mesh` A-record set
#       (MEDIA-5) — one HTTP call per overlay IP — so adding a media node needs
#       no config here.
#
# Secrets (DO_SPACES_* + ND_ADMIN_*) come from the same root-only creds env file
# the leader-managed-secret path (MEDIA-2/6) writes; nothing sensitive is on
# argv (it would show in `ps` — the design security lock, mirrors EFF-21/XCP-7).
#
# Options:
#   --creds <file>   creds env file (default /etc/mackesd/media-spaces.env)
#   --port <p>       Subsonic API port (default 4533)
#   --host <h>       resolve this name for the instance set (default music.mesh)
#
# Exit non-zero on any hard failure (missing creds, no instances, a failed
# upload) so a cron/operator wrapper can see it; a single instance failing a
# rescan is reported but does not fail the whole sweep (active-active: the others
# still re-index).
set -euo pipefail

CREDS=/etc/mackesd/media-spaces.env
PORT=4533
HOST=music.mesh

usage() {
  cat >&2 <<EOF
usage: mcnf-music-ingest.sh [--creds <file>] [--port <p>] [--host <h>] <command>
  upload <local-path> [<dest-subpath>]   copy music into the shared bucket
  rescan                                 re-index every live media instance
EOF
  exit 2
}

# ---- arg parse (options before the subcommand) ----------------------------
while [ $# -gt 0 ]; do case "$1" in
  --creds) CREDS="$2"; shift 2;;
  --port)  PORT="$2";  shift 2;;
  --host)  HOST="$2";  shift 2;;
  -h|--help) usage;;
  upload|rescan) break;;
  *) echo "mcnf-music-ingest: unknown option/command: $1" >&2; usage;;
esac; done
[ $# -ge 1 ] || usage
CMD="$1"; shift

log() { echo "==> music-ingest: $*"; }

load_creds() {
  [ -s "$CREDS" ] || {
    echo "mcnf-music-ingest: creds env file '$CREDS' missing/empty — the" >&2
    echo "  leader-managed secret (MEDIA-2/6) must write DO_SPACES_* + ND_ADMIN_*" >&2
    echo "  there before ingesting. Refusing to continue." >&2
    exit 1; }
  # shellcheck disable=SC1090
  set -a; . "$CREDS"; set +a
}

# A dedicated rclone config so we never touch a human's ~/.config/rclone — the
# same shape setup-media-navidrome.sh writes (kept in sync by hand; both derive
# from the one creds env file).
rclone_conf() {
  local conf; conf="$(mktemp)"
  umask 077
  cat > "$conf" <<EOF
[spaces]
type = s3
provider = DigitalOcean
access_key_id = ${DO_SPACES_KEY}
secret_access_key = ${DO_SPACES_SECRET}
endpoint = ${DO_SPACES_ENDPOINT}
region = ${DO_SPACES_REGION}
acl = private
EOF
  umask 022
  echo "$conf"
}

do_upload() {
  local src="${1:-}" dest="${2:-}"
  [ -n "$src" ] || { echo "upload: need a <local-path>" >&2; usage; }
  [ -e "$src" ] || { echo "upload: '$src' does not exist" >&2; exit 1; }
  command -v rclone >/dev/null 2>&1 || { echo "upload: rclone not installed" >&2; exit 1; }
  load_creds
  for k in DO_SPACES_KEY DO_SPACES_SECRET DO_SPACES_ENDPOINT DO_SPACES_REGION DO_SPACES_BUCKET; do
    [ -n "${!k:-}" ] || { echo "upload: creds file missing $k" >&2; exit 1; }
  done
  local conf; conf="$(rclone_conf)"
  trap 'rm -f "$conf"' EXIT
  local target="spaces:${DO_SPACES_BUCKET}"
  [ -n "$dest" ] && target="${target}/${dest#/}"
  log "rclone copy '$src' -> '$target'"
  rclone copy --config "$conf" --progress "$src" "$target"
  log "upload complete — run 'mcnf-music-ingest.sh rescan' to re-index every instance"
}

# Resolve the music.mesh A-set to a list of overlay IPs (MEDIA-5). Falls back to
# the single host string if resolution yields nothing (a one-instance mesh).
instance_ips() {
  local ips
  ips="$(getent ahostsv4 "$HOST" 2>/dev/null | awk '{print $1}' | sort -u || true)"
  [ -n "$ips" ] && { echo "$ips"; return; }
  echo "$HOST"
}

do_rescan() {
  command -v curl >/dev/null 2>&1 || { echo "rescan: curl not installed" >&2; exit 1; }
  load_creds
  [ -n "${ND_ADMIN_USER:-}" ] && [ -n "${ND_ADMIN_PASS:-}" ] || {
    echo "rescan: creds file missing ND_ADMIN_USER / ND_ADMIN_PASS" >&2; exit 1; }
  local any=0 ok=0
  while read -r ip; do
    [ -n "$ip" ] || continue
    any=1
    log "startScan on $ip:$PORT"
    # Subsonic startScan.view — password on the query is over the trusted overlay
    # only (mesh-internal, never public); kept off `ps` by virtue of curl reading
    # it from this shell, not a child argv we exported.
    if curl -fsS --max-time 15 \
        "http://${ip}:${PORT}/rest/startScan.view?u=${ND_ADMIN_USER}&p=${ND_ADMIN_PASS}&v=1.16.1&c=mcnf&f=json" \
        >/dev/null 2>&1; then
      ok=$((ok + 1))
    else
      echo "rescan: instance $ip did not accept startScan (skipping; active-active)" >&2
    fi
  done <<< "$(instance_ips)"
  [ "$any" = 1 ] || { echo "rescan: no music instances resolved from '$HOST'" >&2; exit 1; }
  log "rescan triggered on $ok instance(s)"
}

case "$CMD" in
  upload) do_upload "$@";;
  rescan) do_rescan "$@";;
  *) usage;;
esac
