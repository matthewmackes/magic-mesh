#!/bin/bash
# setup-syncthing.sh — SUBSTRATE-5 (SUBSTRATE-V2): stand up the Syncthing file
# plane that replaces the LizardFS QNM-Shared *file* mount. Boot-durable +
# idempotent.
#
# /mnt/mesh-storage is a dedicated bind or block mount (still no FUSE) that
# Syncthing replicates full-mesh. An empty directory on the root LV is not a
# healthy file plane — preflight uses `mountpoint -q` (same fail-closed check
# as XDG Downloads in node_grade) and refuses to claim the folder ready until
# a real mount is present. This helper does not invent dests or create that
# mount. Overlay-only: global/relay/local discovery OFF, NAT OFF, telemetry
# OFF; peers are wired by static device IDs from the etcd
# `/mesh/syncthing/<host>` registry (closes the discovery loop without any
# public discovery — lock #11). Conflicts/deletes land in `.stversions`
# (trash-can versioning, lock #4).
#
# Options:
#   --listen <ip>     this node's OVERLAY ip for the GUI/listen bind
#                     (default: auto-detect the nebula iface; falls back to
#                     127.0.0.1 so a not-yet-enrolled node still comes up)
#   --folder <dir>    the shared folder (default /mnt/mesh-storage)
#   --home <dir>      syncthing config/home (default /var/lib/mcnf-syncthing)
#   --folder-id <id>  shared folder id (default mcnf-mesh)
#   --self-test       prove non-mount dirs fail closed (temp fixtures only)
#
# Publishes this node's device ID to etcd `/mesh/syncthing/<host>` and wires
# every peer device found there (best-effort — skipped if etcdctl/endpoints are
# absent; a later run / the reconcile worker fills them in).
set -euo pipefail

LISTEN=""; FOLDER=/mnt/mesh-storage; HOME_DIR=/var/lib/mcnf-syncthing; FOLDER_ID=mcnf-mesh
ENDPOINTS_FILE="${MCNF_ETCD_ENDPOINTS_FILE:-/etc/mackesd/etcd-endpoints}"
SYSTEMD_DIR="${MCNF_SYSTEMD_DIR:-/etc/systemd/system}"

log() { echo "==> $*"; }

# Fail closed: a directory on the root LV (even if empty and writable) is not
# a healthy Syncthing file plane. node_grade uses the same `mountpoint -q`
# check for Workstation XDG Downloads. Missing `mountpoint` is also a failure.
require_mesh_folder_mount() {
  local folder="$1"
  if [ -d "$folder" ] && mountpoint -q "$folder"; then
    return 0
  fi
  echo "setup-syncthing: file-plane path $folder is not a mountpoint; an empty directory is not a healthy Syncthing plane (needs a bind or real mount)" >&2
  exit 1
}

self_test() {
  local not_a_mount missing fake_mount
  MCNF_SETUP_SYNCTHING_ST_TMP="$(mktemp -d /tmp/mcnf-setup-syncthing-st.XXXXXX)"
  cleanup_self_test() {
    case "${MCNF_SETUP_SYNCTHING_ST_TMP:-}" in
      /tmp/mcnf-setup-syncthing-st.*) rm -rf -- "$MCNF_SETUP_SYNCTHING_ST_TMP" ;;
    esac
  }
  trap cleanup_self_test EXIT
  not_a_mount="$MCNF_SETUP_SYNCTHING_ST_TMP/not-a-mount"
  missing="$MCNF_SETUP_SYNCTHING_ST_TMP/missing"
  fake_mount="$MCNF_SETUP_SYNCTHING_ST_TMP/fake-mount"
  mkdir -p "$not_a_mount" "$fake_mount"

  if ( require_mesh_folder_mount "$not_a_mount" ) 2>/dev/null; then
    echo "setup-syncthing: --self-test expected a non-mount directory to fail closed" >&2
    exit 1
  fi
  if ( require_mesh_folder_mount "$missing" ) 2>/dev/null; then
    echo "setup-syncthing: --self-test expected a missing path to fail closed" >&2
    exit 1
  fi

  mkdir -p "$MCNF_SETUP_SYNCTHING_ST_TMP/bin"
  cat > "$MCNF_SETUP_SYNCTHING_ST_TMP/bin/mountpoint" <<'SH'
#!/usr/bin/env bash
[ "${1:-}" = "-q" ] || exit 2
[ -d "${2:-}" ] || exit 1
exit 0
SH
  chmod +x "$MCNF_SETUP_SYNCTHING_ST_TMP/bin/mountpoint"
  PATH="$MCNF_SETUP_SYNCTHING_ST_TMP/bin:$PATH" require_mesh_folder_mount "$fake_mount"

  grep -Fq 'mountpoint -q' "$0" || {
    echo "setup-syncthing: --self-test expected mountpoint -q preflight" >&2
    exit 1
  }
  if grep -Eq 'mkdir -p "\$HOME_DIR" "\$FOLDER"' "$0"; then
    echo "setup-syncthing: --self-test found mkdir of the folder before the mount check" >&2
    exit 1
  fi
  echo "setup-syncthing: self-test passed"
}

if [ "${1:-}" = "--self-test" ]; then
  [ "$#" -eq 1 ] || { echo "usage: $0 --self-test" >&2; exit 2; }
  self_test
  exit 0
fi

while [ $# -gt 0 ]; do case "$1" in
  --listen) LISTEN="$2"; shift 2;;
  --folder) FOLDER="$2"; shift 2;;
  --home) HOME_DIR="$2"; shift 2;;
  --folder-id) FOLDER_ID="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
LISTEN="${LISTEN:-$(detect_overlay)}"
LISTEN="${LISTEN:-127.0.0.1}"
HOST="$(hostname -s)"

# Preflight before any package/config work: do not treat a non-mount
# directory as a healthy file plane, and do not mkdir one into existence.
require_mesh_folder_mount "$FOLDER"

# ---- syncthing binary (BIRTHRIGHT: Fedora repos carry syncthing) ----------
if ! command -v syncthing >/dev/null 2>&1; then
  log "installing syncthing"
  dnf install -y syncthing >/dev/null 2>&1 || {
    echo "syncthing not installed and dnf install failed" >&2; exit 1; }
fi

mkdir -p "$HOME_DIR"

# ---- mint cert + device id + a default config (idempotent) ----------------
if [ ! -f "$HOME_DIR/config.xml" ]; then
  log "generating syncthing identity in $HOME_DIR"
  syncthing generate --home "$HOME_DIR" >/dev/null 2>&1 || true
fi
# Device ID is printed by `generate`. Match the ID PATTERN (8 dash-separated
# 7-char groups), NOT a version-specific prefix — syncthing v1.30.0 (Fedora 43)
# prints "Device ID: <ID>" while v2.x prints "device=<ID>"; the old `device=`
# grep matched only v2 → empty → exit 1 on the live F43 build (SUBSTRATE-14
# rehearsal caught the version skew). The deterministic id is derived from the
# cert, so a second `generate` re-prints the same value.
DEVICE_ID="$(syncthing generate --home "$HOME_DIR" 2>&1 | grep -oE '[A-Z0-9]{7}(-[A-Z0-9]{7}){7}' | head -1)"
[ -z "$DEVICE_ID" ] && { echo "could not determine syncthing device id" >&2; exit 1; }
log "device id: $DEVICE_ID"

# ---- read peer device IDs from the etcd registry (best-effort) -------------
# Format passed to the python editor: "host=DEVICEID" lines.
PEER_DEVICES=""
REGISTRY_SNAPSHOT_NONEMPTY=0
if command -v etcdctl >/dev/null 2>&1 && [ -s "$ENDPOINTS_FILE" ]; then
  EPS="$(tr '\n' ',' < "$ENDPOINTS_FILE" | sed 's/,$//')"
  # Publish our ID + overlay IP so peers can dial us explicitly (discovery is off).
  ETCDCTL_API=3 etcdctl --endpoints="$EPS" put "/mesh/syncthing/$HOST" "$DEVICE_ID@$LISTEN" >/dev/null 2>&1 || true
  # The default etcdctl output is clean alternating key\nvalue lines:
  #   /mesh/syncthing/<host>\n<device-id>\n...  → pair them into host=device-id.
  # (The old `--print-value-only -w fields` combo returned NOTHING — --print-value-only
  # suppresses the keys the awk needed, and -w fields base64-encodes; the
  # SUBSTRATE-14 rehearsal caught the registry yielding 0 peers despite both
  # devices being present.)
  # A failed or empty read is NOT an authoritative empty registry. Preserve all
  # existing folder shares and global devices in that case; otherwise a brief
  # etcd outage during setup could destructively erase a healthy file plane.
  REGISTRY_RAW=""
  if REGISTRY_RAW="$(ETCDCTL_API=3 etcdctl --endpoints="$EPS" get --prefix /mesh/syncthing/ 2>/dev/null)" \
     && [ -n "$(printf '%s' "$REGISTRY_RAW" | tr -d '[:space:]')" ]; then
    PEER_DEVICES="$(printf '%s\n' "$REGISTRY_RAW" \
      | awk 'NR%2==1{sub(/.*\/mesh\/syncthing\//,"",$0); k=$0; next} {print k"="$0}')"
    REGISTRY_SNAPSHOT_NONEMPTY=1
  fi
fi

# ---- apply overlay-only config + the shared folder + peers (XML edit) ------
# Offline config edit (the `syncthing cli` config API needs a running daemon),
# done with real XML parsing so it's schema-tolerant across syncthing versions.
PEER_DEVICES="$PEER_DEVICES" REGISTRY_SNAPSHOT_NONEMPTY="$REGISTRY_SNAPSHOT_NONEMPTY" \
python3 - "$HOME_DIR/config.xml" "$DEVICE_ID" "$HOST" "$FOLDER" "$FOLDER_ID" "$LISTEN" <<'PY'
import os, re, sys, xml.etree.ElementTree as ET
cfg, my_id, my_host, folder, folder_id, listen = sys.argv[1:7]
tree = ET.parse(cfg); root = tree.getroot()

# Syncthing rejects the WHOLE config if any device id is malformed, so a corrupt
# registry entry must never poison the file — validate the base32 group shape and
# skip anything that doesn't match.
DEV_RE = re.compile(r'^[A-Z2-7]{7}(-[A-Z2-7]{7}){7}$')

def set_opt(name, val):
    opts = root.find('options')
    if opts is None:
        opts = ET.SubElement(root, 'options')
    el = opts.find(name)
    if el is None:
        el = ET.SubElement(opts, name)
    el.text = val

# Overlay-only + quiet (lock #11): no global/local/relay discovery, no NAT, no
# usage reporting, no crash reporting, no auto-upgrade (we ship via the RPM).
for k in ('globalAnnounceEnabled','localAnnounceEnabled','relaysEnabled',
          'natEnabled','crashReportingEnabled'):
    set_opt(k, 'false')
set_opt('urAccepted', '-1')
set_opt('autoUpgradeIntervalH', '0')

# GUI/listen bound to the overlay only.
gui = root.find('gui')
if gui is not None:
    addr = gui.find('address')
    if addr is None: addr = ET.SubElement(gui, 'address')
    addr.text = f'{listen}:8384'
# Listen for sync on the overlay only (no relays/dynamic).
opts = root.find('options')
for la in opts.findall('listenAddress'):
    opts.remove(la)
ET.SubElement(opts, 'listenAddress').text = f'tcp://{listen}:22000'

# Devices: ensure ourselves + every registry peer is present. Discovery is OFF
# (overlay-only), so a peer's address MUST be explicit (tcp://<overlay-ip>:22000)
# — the default 'dynamic' relies on global/local discovery and never connects on
# the overlay (the SUBSTRATE-14 rehearsal showed syncthing up + peered in config
# but never CONNECTING, so no file sync). Self stays 'dynamic'.
def ensure_device(dev_id, name, addr='dynamic'):
    d = None
    for e in root.findall('device'):
        if e.get('id') == dev_id:
            d = e; break
    if d is None:
        d = ET.SubElement(root, 'device', {'id': dev_id, 'name': name,
                                           'compression': 'metadata',
                                           'introducer': 'false'})
    for a in d.findall('address'):
        d.remove(a)
    ET.SubElement(d, 'address').text = addr
    return d

ensure_device(my_id, my_host)
peers = []
seen_peers = set()
valid_registry_entries = 0
saw_self_in_registry = False
for line in os.environ.get('PEER_DEVICES','').splitlines():
    line = line.strip()
    if '=' not in line: continue
    host, val = line.split('=', 1)
    host, val = host.strip(), val.strip()
    # Registry value is "<device-id>@<overlay-ip>" (the ip is for the explicit
    # peer address); tolerate a bare id (no @) by falling back to dynamic.
    dev, _, ip = val.partition('@')
    dev, ip = dev.strip(), ip.strip()
    if not dev: continue
    if not DEV_RE.match(dev):
        sys.stderr.write(f'skipping malformed device id for {host!r}\n'); continue
    valid_registry_entries += 1
    if dev == my_id:
        saw_self_in_registry = True
        continue
    if dev in seen_peers: continue
    ensure_device(dev, host, f'tcp://{ip}:22000' if ip else 'dynamic')
    seen_peers.add(dev)
    peers.append(dev)

# Destructive reconciliation is permitted only from a successful, non-empty
# registry snapshot containing this node's just-published identity. A failed,
# empty, stale, or wholly malformed snapshot is unknown state, never "no peers".
registry_authoritative = (
    os.environ.get('REGISTRY_SNAPSHOT_NONEMPTY') == '1'
    and valid_registry_entries > 0
    and saw_self_in_registry
)

# The shared folder at <folder>, full-mesh to every known device, with
# trash-can versioning (lock #4).
fol = None
for f in root.findall('folder'):
    if f.get('id') == folder_id:
        fol = f; break
if fol is None:
    fol = ET.SubElement(root, 'folder', {'id': folder_id})
fol.set('label', 'Mesh Sync')
fol.set('path', folder)
fol.set('type', 'sendreceive')
# Reset the managed folder only from an authoritative registry snapshot. When
# the registry is offline/empty, preserve every existing share and merely make
# sure this node remains associated with its own folder.
if registry_authoritative:
    for d in fol.findall('device'): fol.remove(d)
    ET.SubElement(fol, 'device', {'id': my_id})
    for dev in peers:
        ET.SubElement(fol, 'device', {'id': dev})
elif not any(d.get('id') == my_id for d in fol.findall('device')):
    ET.SubElement(fol, 'device', {'id': my_id})
for v in fol.findall('versioning'): fol.remove(v)
ver = ET.SubElement(fol, 'versioning', {'type': 'trashcan'})
ET.SubElement(ver, 'cleanupIntervalS').text = '3600'
ET.SubElement(ver, 'param', {'key': 'cleanoutDays', 'val': '30'})

# A full setup can remove stale global devices, but only after the guarded
# managed-folder reconciliation above. Keep self, every desired registry peer,
# and every device referenced by ANY folder (including non-MCNF folders). This
# safely removes stale unshared Lighthouse entries without damaging unrelated
# shares. The timer reconciler remains strictly additive and never runs this.
pruned = []
if registry_authoritative:
    desired = {my_id, *peers}
    referenced = {
        d.get('id')
        for shared_folder in root.findall('folder')
        for d in shared_folder.findall('device')
        if d.get('id')
    }
    for device in list(root.findall('device')):
        dev_id = device.get('id')
        if dev_id and dev_id not in desired and dev_id not in referenced:
            root.remove(device)
            pruned.append(dev_id)

tree.write(cfg, encoding='UTF-8', xml_declaration=True)
if registry_authoritative:
    print(f'configured folder {folder_id} at {folder} with {len(peers)} peer device(s); pruned {len(pruned)} stale unshared global device(s)')
else:
    print(f'configured folder {folder_id} at {folder}; registry unavailable/empty, preserved existing device shares')
PY

# ---- boot-durable unit ----------------------------------------------------
# Write OUR unit INLINE (same fix as setup-etcd.sh — the old path-relative
# `cp $(dirname $0)/../packaging/...` resolved to a non-existent path when run
# from /usr/libexec/mackesd, so the unit was never installed and syncthing never
# started; the SUBSTRATE-14 rehearsal caught it). Works identically from a git
# checkout, the RPM, or the installed libexec path.
mkdir -p "$SYSTEMD_DIR"
cat > "$SYSTEMD_DIR/syncthing.service" <<'UNIT'
[Unit]
Description=MCNF Mesh Sync (Syncthing) — overlay file replication
After=network-online.target nebula.service
Wants=network-online.target
ConditionPathExists=/etc/systemd/system/syncthing.service.d/10-home.conf
[Service]
Type=simple
Environment=MCNF_SYNCTHING_HOME=/var/lib/mcnf-syncthing
# HOME MUST be set: systemd services start with no $HOME, and syncthing v1.30.0
# calls os.UserHomeDir() at startup (even with --home) → "panic: Failed to get
# user home dir" + SIGABRT. A manual run inherits $HOME so it "works"; only the
# service crashes (the SUBSTRATE-14 rehearsal caught this). The 10-home.conf
# drop-in below overrides both to the real --home dir.
Environment=HOME=/var/lib/mcnf-syncthing
ExecStart=/usr/bin/syncthing serve --home=${MCNF_SYNCTHING_HOME} --no-browser --no-restart
Restart=always
RestartSec=10
SuccessExitStatus=3 4
LimitNOFILE=65536
[Install]
WantedBy=multi-user.target
UNIT
# The unit runs `syncthing serve --home <HOME_DIR>`; pin the home via a drop-in
# so a custom --home survives.
mkdir -p "$SYSTEMD_DIR/syncthing.service.d"
cat > "$SYSTEMD_DIR/syncthing.service.d/10-home.conf" <<EOF
[Service]
Environment=MCNF_SYNCTHING_HOME=$HOME_DIR
# HOME too — syncthing v1.30.0 panics without it under systemd (see the unit).
Environment=HOME=$HOME_DIR
EOF
systemctl daemon-reload 2>/dev/null || true
systemctl enable syncthing.service >/dev/null 2>&1 || true
systemctl restart syncthing.service 2>/dev/null || true

# ---- SUBSTRATE-5 self-heal: reconcile the device list on a timer -----------
# This script wires only the peers in the registry NOW; a node that joins LATER
# would never be learned by this one (and vice-versa). Ship a oneshot + timer that
# runs syncthing-reconcile every 2 min — it adds any newly-registered peer device
# to the RUNNING daemon LIVE (no restart; idempotent no-op at steady state), so the
# file plane self-heals like the etcd peer directory. Units written inline (same
# reason as syncthing.service above — a path-relative cp from /usr/libexec fails).
cat > "$SYSTEMD_DIR/syncthing-reconcile.service" <<UNIT
[Unit]
Description=MCNF Mesh Sync — reconcile Syncthing peer devices from the etcd registry
After=syncthing.service etcd.service
[Service]
Type=oneshot
Environment=MCNF_SYNCTHING_HOME=$HOME_DIR
ExecStart=/usr/libexec/mackesd/syncthing-reconcile
UNIT
cat > "$SYSTEMD_DIR/syncthing-reconcile.timer" <<'UNIT'
[Unit]
Description=MCNF Mesh Sync — periodic Syncthing device reconcile (SUBSTRATE-5)
[Timer]
OnBootSec=60
OnUnitActiveSec=120
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload 2>/dev/null || true
systemctl enable --now syncthing-reconcile.timer >/dev/null 2>&1 || true
log "done — folder $FOLDER shared full-mesh (overlay-only); reconcile timer armed"
