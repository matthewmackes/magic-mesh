#!/bin/bash
# setup-media-navidrome.sh — legacy non-lighthouse media host helper: stand up
# a boot-durable Navidrome music service. DigitalOcean lighthouses are thin and
# are rejected below.
# + idempotent. Two units:
#
#   mcnf-music-store.service  — rclone mount of the shared DO Spaces bucket at
#                               <state>/library, read-mostly, with a bounded VFS
#                               cache (lock #2: S3 object store presented POSIX).
#   mcnf-navidrome.service    — rootless-podman Navidrome reading that path,
#                               per-instance local SQLite scan (lock #3), Subsonic
#                               API on the overlay :4533 (lock #1), hard
#                               MemoryMax/CPUQuota caps so it never starves the
#                               host (lock #9 / netdata-thrash protection).
#
# Active-active: every explicitly provisioned non-lighthouse media host may run this; clients reach any instance via
# `music.mesh` mesh-DNS (MEDIA-5). Per-instance scan of the one shared bucket =
# stateless readers, no shared-DB-over-network footgun (lock #3).
#
# Secrets are NEVER on argv (they'd show in `ps` — design-doc security lock,
# mirrors EFF-21 / XCP-7). The S3 keys + the shared-account password come from a
# root-only env file the leader-managed-secret path (MEDIA-2/MEDIA-6) writes.
#
# Options:
#   --listen <ip>      overlay ip to bind :4533 (default: auto-detect nebula;
#                      falls back to 127.0.0.1 so a not-yet-enrolled node comes up)
#   --creds <file>     S3 + account env file (default /etc/mackesd/media-spaces.env)
#   --state <dir>      host state root (default /var/lib/mcnf-music)
#   --port <p>         Subsonic API port (default 4533)
#   --image <ref>      Navidrome image (default docker.io/deluan/navidrome:0.53.3)
#   --memory-max <m>   container MemoryMax (default 768M)
#   --cpu-quota <q>    container CPUQuota (default 75%)
#   --vfs-cache <sz>   rclone VFS cache hard cap (default 4G)
#   --playlists <dir>  MEDIA-6 mesh-synced flat-file playlist dir (default
#                      /mnt/mesh-storage/music-playlists — the Syncthing plane)
#
# Required keys in the creds env file (KEY=value, root-only 0600):
#   DO_SPACES_KEY, DO_SPACES_SECRET, DO_SPACES_ENDPOINT (e.g. nyc3.digitaloceanspaces.com),
#   DO_SPACES_REGION (e.g. nyc3), DO_SPACES_BUCKET, ND_ADMIN_USER, ND_ADMIN_PASS
#
# Rollback: `systemctl disable --now mcnf-navidrome.service mcnf-music-store.service`
# then `podman rm -f navidrome` + `fusermount -uz <state>/library`.
set -euo pipefail

LISTEN=""; CREDS=/etc/mackesd/media-spaces.env; STATE=/var/lib/mcnf-music
PORT=4533; IMAGE=docker.io/deluan/navidrome:0.53.3
MEMORY_MAX=768M; CPU_QUOTA=75%; VFS_CACHE=4G
# MEDIA-6 — mesh-wide playlists via FLAT FILES: a Syncthing-replicated dir of
# .m3u playlists on the workgroup root (/mnt/mesh-storage, the SUBSTRATE-5
# Syncthing plane), bind-mounted into every Navidrome. Both instances import the
# same playlists; edits to a file-backed playlist sync back to its .m3u, which
# Syncthing then replicates. NO shared SQLite — lock #3 stands; flat files dodge
# the concurrent-DB-write corruption risk a shared scan DB would carry.
PLAYLISTS=/mnt/mesh-storage/music-playlists

while [ $# -gt 0 ]; do case "$1" in
  --listen) LISTEN="$2"; shift 2;;
  --creds) CREDS="$2"; shift 2;;
  --state) STATE="$2"; shift 2;;
  --port) PORT="$2"; shift 2;;
  --image) IMAGE="$2"; shift 2;;
  --memory-max) MEMORY_MAX="$2"; shift 2;;
  --cpu-quota) CPU_QUOTA="$2"; shift 2;;
  --vfs-cache) VFS_CACHE="$2"; shift 2;;
  --playlists) PLAYLISTS="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

log() { echo "==> media: $*"; }

# Thin-lighthouse policy: this legacy helper is retained for non-lighthouse
# media hosts, but it must never be used to promote or populate a DO lighthouse.
# Check the durable role pin before touching credentials, packages, or services.
if [ -r /var/lib/mde/role.toml ] && grep -Eq '^[[:space:]]*role[[:space:]]*=[[:space:]]*"?lighthouse"?[[:space:]]*$' /var/lib/mde/role.toml; then
  echo "setup-media-navidrome: media/file-sharing lighthouse support is retired; use a non-lighthouse media host" >&2
  exit 1
fi

# Refuse early without the creds (a clear error beats a half-provisioned mount
# that silently fails to authenticate — mirrors sign-release.sh's key check).
[ -s "$CREDS" ] || {
  echo "setup-media-navidrome: creds env file '$CREDS' missing/empty — the" >&2
  echo "  leader-managed secret (MEDIA-2/6) must write DO_SPACES_* + ND_ADMIN_*" >&2
  echo "  there (root-only 0600) before provisioning. Refusing to continue." >&2
  exit 1; }
# shellcheck disable=SC1090
set -a; . "$CREDS"; set +a
for k in DO_SPACES_KEY DO_SPACES_SECRET DO_SPACES_ENDPOINT DO_SPACES_REGION \
         DO_SPACES_BUCKET ND_ADMIN_USER ND_ADMIN_PASS; do
  [ -n "${!k:-}" ] || { echo "setup-media-navidrome: creds file missing $k" >&2; exit 1; }
done

detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
LISTEN="${LISTEN:-$(detect_overlay)}"
LISTEN="${LISTEN:-127.0.0.1}"

LIBRARY="$STATE/library"   # rclone mount target → Navidrome /music (read-mostly)
DATA="$STATE/data"         # per-instance SQLite scan DB (local, NOT shared)
CACHE="$STATE/vfs-cache"   # rclone VFS cache
mkdir -p "$LIBRARY" "$DATA" "$CACHE"
# MEDIA-6 — the mesh-synced flat-file playlist dir (Syncthing-replicated when it
# lives under the workgroup root; a plain local dir otherwise — still valid).
mkdir -p "$PLAYLISTS"

# ---- tooling: rclone + podman (BIRTHRIGHT: both in the Fedora base repos) --
for pkg in rclone podman fuse3; do
  cmd="$pkg"; [ "$pkg" = "fuse3" ] && cmd="fusermount3"
  command -v "$cmd" >/dev/null 2>&1 && continue
  log "installing $pkg"
  dnf install -y "$pkg" >/dev/null 2>&1 || {
    echo "$pkg not installed and dnf install failed" >&2; exit 1; }
done

# ---- rclone remote config (idempotent; secrets written root-only 0600) ----
# A dedicated config file so we never touch a human's ~/.config/rclone.
RCLONE_CONF="$STATE/rclone.conf"
umask 077
cat > "$RCLONE_CONF" <<EOF
[spaces]
type = s3
provider = DigitalOcean
access_key_id = $DO_SPACES_KEY
secret_access_key = $DO_SPACES_SECRET
endpoint = $DO_SPACES_ENDPOINT
region = $DO_SPACES_REGION
acl = private
# A bucket-scoped (readwrite) key cannot CreateBucket; without this, rclone's
# default bucket-existence precheck 403s on any write (uploads/MEDIA-9). The
# bucket is provisioned out-of-band (MEDIA-2), so skip the check.
no_check_bucket = true
EOF
chmod 600 "$RCLONE_CONF"
umask 022

# ---- the shared-bucket mount unit (rclone → /music source) ----------------
# read-mostly: --read-only-mount keeps Navidrome's scan from mutating the shared
# bucket; VFS full cache bounds first-scan + cover-art S3 latency (design risk).
cat > /etc/systemd/system/mcnf-music-store.service <<UNIT
[Unit]
Description=MCNF Media — shared music bucket (rclone S3 mount of DO Spaces)
After=network-online.target nebula.service
Wants=network-online.target
[Service]
Type=notify
Environment=RCLONE_CONFIG=$RCLONE_CONF
ExecStartPre=/usr/bin/mkdir -p $LIBRARY $CACHE
ExecStart=/usr/bin/rclone mount spaces:$DO_SPACES_BUCKET $LIBRARY \\
  --config $RCLONE_CONF --allow-other --read-only \\
  --dir-cache-time 1m --poll-interval 30s \\
  --vfs-cache-mode full --vfs-cache-max-size $VFS_CACHE --cache-dir $CACHE
ExecStop=/bin/fusermount3 -uz $LIBRARY
Restart=always
RestartSec=10
[Install]
WantedBy=multi-user.target
UNIT

# ---- the Navidrome container unit (capped, per-instance scan) -------------
# Rootless-podman --rm run under systemd; the breaker is Restart=always. Local
# SQLite under $DATA = per-instance scan (lock #3). MemoryMax/CPUQuota are the
# hard host-protection caps (lock #9). Bound to the overlay only (no public).
cat > /etc/systemd/system/mcnf-navidrome.service <<UNIT
[Unit]
Description=MCNF Media — Navidrome (Subsonic) music server
Requires=mcnf-music-store.service
After=mcnf-music-store.service network-online.target nebula.service
Wants=network-online.target
[Service]
Type=simple
# Hard host-protection caps so the container never starves the lighthouse.
MemoryMax=$MEMORY_MAX
CPUQuota=$CPU_QUOTA
Environment=ND_MUSICFOLDER=/music
Environment=ND_DATAFOLDER=/data
Environment=ND_SCANSCHEDULE=1h
Environment=ND_SESSIONTIMEOUT=24h
Environment=ND_LOGLEVEL=info
# MEDIA-6 — import the Syncthing-synced flat .m3u playlists (mesh-wide); a
# file-backed playlist's UI edits sync back to its .m3u, which Syncthing then
# replicates to the peer non-lighthouse media host.
Environment=ND_PLAYLISTSPATH=/playlists
Environment=ND_AUTOIMPORTPLAYLISTS=true
# First-start bootstrap of the single shared service account (idempotent —
# Navidrome creates it only if no users exist; MEDIA-6).
Environment=ND_DEFAULTADMINPASSWORD=$ND_ADMIN_PASS
ExecStartPre=-/usr/bin/podman rm -f navidrome
ExecStart=/usr/bin/podman run --rm --name navidrome \\
  --network host \\
  -e ND_MUSICFOLDER -e ND_DATAFOLDER -e ND_SCANSCHEDULE \\
  -e ND_SESSIONTIMEOUT -e ND_LOGLEVEL -e ND_DEFAULTADMINPASSWORD \\
  -e ND_PLAYLISTSPATH -e ND_AUTOIMPORTPLAYLISTS \\
  -e ND_ADDRESS=$LISTEN -e ND_PORT=$PORT \\
  -v $LIBRARY:/music:ro -v $DATA:/data:Z -v $PLAYLISTS:/playlists:rw \\
  $IMAGE
ExecStop=/usr/bin/podman stop -t 10 navidrome
Restart=always
RestartSec=15
[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload 2>/dev/null || true
# enable + RESTART (not `enable --now`): on a RE-provision the unit file changed
# but an already-running unit keeps its old config under `enable --now` (this bit
# the MEDIA-6 playlists mount — the live container ran stale until an explicit
# restart). `restart` applies the rewritten unit and starts it if it was down.
systemctl enable mcnf-music-store.service >/dev/null 2>&1 || true
systemctl restart mcnf-music-store.service >/dev/null 2>&1 || true
# Mount established (Requires= orders navidrome after it); restart navidrome too
# so a config change (e.g. the playlists mount) actually reaches the container.
systemctl enable mcnf-navidrome.service >/dev/null 2>&1 || true
systemctl restart mcnf-navidrome.service >/dev/null 2>&1 || true

# MEDIA-6 — auto-provision the single shared service account. Navidrome 0.53's
# ND_DEFAULTADMINPASSWORD does NOT auto-create the user (verified live 2026-06-27 —
# Subsonic auth returned "data not found"); the first-run POST /auth/createAdmin
# is what actually seeds it. Idempotent: once an admin exists Navidrome refuses a
# second, so a re-run is a harmless no-op. Without this the published shared
# account (media_registry → music_autoconfig) never authenticates.
for _ in $(seq 1 20); do ss -tlnp 2>/dev/null | grep -q ":$PORT\b" && break; sleep 2; done
curl -fsS -m8 "http://$LISTEN:$PORT/" >/dev/null 2>&1 || true   # touch the web app (first-run init)
if curl -fsS -m10 -X POST "http://$LISTEN:$PORT/auth/createAdmin" \
     -H 'Content-Type: application/json' \
     -d "{\"username\":\"$ND_ADMIN_USER\",\"password\":\"$ND_ADMIN_PASS\"}" >/dev/null 2>&1; then
  echo "==> media: shared account '$ND_ADMIN_USER' provisioned (createAdmin)"
else
  echo "==> media: admin account already present (createAdmin no-op)"
fi

log "done — Navidrome on http://$LISTEN:$PORT (Subsonic API), bucket=$DO_SPACES_BUCKET, playlists=$PLAYLISTS (mesh-synced flat .m3u)"
echo "  verify: curl -fsS \"http://$LISTEN:$PORT/rest/ping.view?u=$ND_ADMIN_USER&p=…&v=1.16.1&c=mcnf\""
echo "  store:  systemctl status mcnf-music-store.service   # rclone S3 mount"
echo "  server: systemctl status mcnf-navidrome.service     # the container"
echo "  Legacy media helper: use only on an explicitly provisioned non-lighthouse host"
echo "  load-balances + fails over across instances reading this one bucket."
