#!/bin/bash
# setup-etcd.sh — SUBSTRATE-1 (SUBSTRATE-V2): stand up the overlay-bound etcd
# coordination cluster (leader election + peer directory + health). This is the
# strong-consistency core that replaces the LizardFS QNM-Shared *coordination*
# plane; bulk files move to Syncthing (SUBSTRATE-5). Boot-durable + idempotent.
#
# etcd binds the Nebula overlay IP: peer :2380 / client :2379. Server + lighthouse
# nodes are cluster MEMBERS; workstations are CLIENTS only (no local member — just
# the endpoints file mackesd reads). Bootstrap is provisioned from the anchors.
#
# Roles (pick exactly one):
#   --init               bootstrap a NEW single-member cluster (the founding anchor)
#   --join <anchor-ip>   add this node to the existing cluster reachable at
#                        <anchor-ip>:2379 (run on each additional server/lighthouse)
#   --client-only        no local member; just record the anchor endpoints
#                        (workstations target the anchors)
#
# Options:
#   --listen <ip>     this node's OVERLAY ip (default: auto-detect the nebula iface)
#   --name <name>     etcd member name (default: hostname -s)
#   --anchors <csv>   comma-separated anchor OVERLAY ips for the client endpoints
#                     (required for --client-only; augments --join)
#   --data <dir>      data dir (default /var/lib/etcd)
#
# After this runs, /etc/mackesd/etcd-endpoints holds the comma-separated client
# URLs the mackesd etcd client (SUBSTRATE-2/3/4) connects to.
set -euo pipefail

MODE=""; JOIN_ANCHOR=""; LISTEN=""; NAME=""; ANCHORS=""; DATA_DIR=/var/lib/etcd
CLUSTER_TOKEN="mcnf-mesh"
ENDPOINTS_FILE=/etc/mackesd/etcd-endpoints
ENV_FILE=/etc/etcd/etcd.env

while [ $# -gt 0 ]; do case "$1" in
  --init) MODE="init"; shift;;
  --join) MODE="join"; JOIN_ANCHOR="$2"; shift 2;;
  --client-only) MODE="client"; shift;;
  --listen) LISTEN="$2"; shift 2;;
  --name) NAME="$2"; shift 2;;
  --anchors) ANCHORS="$2"; shift 2;;
  --data) DATA_DIR="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

[ -z "$MODE" ] && { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
log() { echo "==> $*"; }

# This node's overlay IP — the nebula interface's v4 address.
detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
LISTEN="${LISTEN:-$(detect_overlay)}"
NAME="${NAME:-$(hostname -s)}"

# ---- etcd binary (BIRTHRIGHT: Fedora repos carry etcd) ---------------------
if [ "$MODE" != "client" ] && ! command -v etcd >/dev/null 2>&1; then
  log "installing etcd"
  dnf install -y etcd >/dev/null 2>&1 || {
    echo "etcd not installed and dnf install failed — install etcd then re-run" >&2
    exit 1
  }
fi
# etcdctl is needed by --join (member add) and is handy everywhere.
command -v etcdctl >/dev/null 2>&1 || dnf install -y etcd >/dev/null 2>&1 || true

mkdir -p "$(dirname "$ENDPOINTS_FILE")" "$(dirname "$ENV_FILE")"

# Compose the client-endpoints list (self for members, anchors for clients).
compose_endpoints() {
  local eps=""
  [ "$MODE" != "client" ] && eps="http://$LISTEN:2379"
  if [ -n "$ANCHORS" ]; then
    local IFS=','; for a in $ANCHORS; do
      [ -n "$a" ] && eps="${eps:+$eps,}http://$a:2379"
    done
  fi
  [ -n "$JOIN_ANCHOR" ] && eps="${eps:+$eps,}http://$JOIN_ANCHOR:2379"
  echo "$eps"
}

# Common WAN-tuned tuning (DO anchors are ~15 ms; give the election headroom).
common_env() {
  cat <<EOF
ETCD_NAME=$NAME
ETCD_DATA_DIR=$DATA_DIR
ETCD_LISTEN_PEER_URLS=http://$LISTEN:2380
ETCD_LISTEN_CLIENT_URLS=http://$LISTEN:2379,http://127.0.0.1:2379
ETCD_ADVERTISE_CLIENT_URLS=http://$LISTEN:2379
ETCD_INITIAL_ADVERTISE_PEER_URLS=http://$LISTEN:2380
ETCD_INITIAL_CLUSTER_TOKEN=$CLUSTER_TOKEN
ETCD_HEARTBEAT_INTERVAL=250
ETCD_ELECTION_TIMEOUT=2500
EOF
}

enable_member() {
  # The RPM ships our unit at /etc/systemd/system/etcd.service already; when run
  # from a git checkout, copy it from the repo. Either way, end up with our unit.
  local unit=/etc/systemd/system/etcd.service
  local src="$(dirname "$0")/../packaging/systemd/etcd.service"
  [ -f "$unit" ] || { [ -f "$src" ] && cp "$src" "$unit"; }
  mkdir -p "$DATA_DIR"
  chown -R etcd:etcd "$DATA_DIR" 2>/dev/null || true
  # mackesd is ordered after etcd (never after a mount) — SUBSTRATE-7.
  mkdir -p /etc/systemd/system/mackesd.service.d
  cat > /etc/systemd/system/mackesd.service.d/20-etcd.conf <<EOF
[Unit]
# SUBSTRATE-7: mackesd coordinates via etcd, so gate it on etcd (Wants+After),
# NOT on any filesystem mount. A Syncthing/file-sync hiccup never stalls mesh
# liveness — only file access degrades.
After=etcd.service
Wants=etcd.service
EOF
  systemctl daemon-reload
  systemctl enable etcd.service >/dev/null 2>&1 || true
  systemctl restart etcd.service
}

case "$MODE" in
  init)
    [ -z "$LISTEN" ] && { echo "no overlay IP (pass --listen)" >&2; exit 1; }
    log "bootstrapping new etcd cluster: $NAME @ $LISTEN"
    { common_env
      echo "ETCD_INITIAL_CLUSTER=$NAME=http://$LISTEN:2380"
      echo "ETCD_INITIAL_CLUSTER_STATE=new"
    } > "$ENV_FILE"
    enable_member
    ;;
  join)
    [ -z "$LISTEN" ] && { echo "no overlay IP (pass --listen)" >&2; exit 1; }
    [ -z "$JOIN_ANCHOR" ] && { echo "--join needs an anchor IP" >&2; exit 1; }
    log "joining etcd cluster via $JOIN_ANCHOR: $NAME @ $LISTEN"
    # member add returns the ETCD_INITIAL_CLUSTER this node must start with.
    ADD_OUT="$(ETCDCTL_API=3 etcdctl --endpoints="http://$JOIN_ANCHOR:2379" \
      member add "$NAME" --peer-urls="http://$LISTEN:2380" 2>/dev/null || true)"
    INIT_CLUSTER="$(echo "$ADD_OUT" | sed -n 's/^ *ETCD_INITIAL_CLUSTER="\(.*\)"/\1/p')"
    if [ -z "$INIT_CLUSTER" ]; then
      echo "member add did not return ETCD_INITIAL_CLUSTER (anchor $JOIN_ANCHOR reachable? already a member?)" >&2
      exit 1
    fi
    { common_env
      echo "ETCD_INITIAL_CLUSTER=$INIT_CLUSTER"
      echo "ETCD_INITIAL_CLUSTER_STATE=existing"
    } > "$ENV_FILE"
    enable_member
    ;;
  client)
    [ -z "$ANCHORS" ] && { echo "--client-only needs --anchors <csv>" >&2; exit 1; }
    log "client-only: recording anchor endpoints"
    # No local member; remove any stale member env so etcd.service stays inert.
    rm -f "$ENV_FILE"
    ;;
esac

EPS="$(compose_endpoints)"
echo "$EPS" > "$ENDPOINTS_FILE"
chmod 0644 "$ENDPOINTS_FILE"
log "etcd endpoints: $EPS  → $ENDPOINTS_FILE"

# Best-effort readiness probe for member roles.
if [ "$MODE" != "client" ]; then
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if ETCDCTL_API=3 etcdctl --endpoints="http://$LISTEN:2379" endpoint health >/dev/null 2>&1; then
      log "etcd healthy on $LISTEN:2379"; break
    fi
    sleep 1
  done
fi
log "done"
