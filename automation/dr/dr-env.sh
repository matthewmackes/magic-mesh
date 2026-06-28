#!/usr/bin/env bash
# dr-env.sh — DAR-37: the ONE place every DR script resolves its targets, so DR
# comes along to a fresh box without code edits. SOURCE it (not exec):
#
#   . "$(dirname "$0")/dr-env.sh"   # populates the MCNF_* DR vars
#   dr-env.sh print-config          # or run it to echo the resolved config
#
# Resolution:
#   MCNF_ETCD        ← the DAR-1b resolver (explicit env → /etc/mackesd/etcd-endpoints
#                      → FAIL LOUD). NO http://172.20.145.192:2379 default (v2).
#   MCNF_AGE_KEY     ← /root/.mcnf-age-key (the mesh/VM age identity)
#   MCNF_MESHFS_DIR  ← /mnt/mesh-storage      (Syncthing-replicated Mesh-Sync root)
#   MCNF_FORGEJO_DATA← /var/lib/mcnf-forgejo  (Forgejo sqlite + repos)
#   MCNF_DR_BUCKET   ← mcnf-dr-4533           (off-fleet DO Spaces bucket)
#   MCNF_DR_DIR      ← $HOME/mcnf-dr-backups  (local working dir for dr-<ts>.age)
#   MCNF_HOST_IP     ← detected overlay (nebula/mde-neb), else empty
#
# The path defaults keep their current-LAN values (meshfs dir, forgejo data,
# bucket) — only MCNF_ETCD lost its dead default. Each is overridable via env.
set -uo pipefail

# Resolve etcd via the shared DAR-1b resolver. We do this in a subshell-tolerant
# way: if the resolver is unavailable we still let an explicit MCNF_ETCD through.
_DR_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -r "$_DR_HERE/../lib/etcd-endpoints.sh" ]; then
  # shellcheck source=../lib/etcd-endpoints.sh
  . "$_DR_HERE/../lib/etcd-endpoints.sh"
fi

# MCNF_ETCD: explicit env wins; else the resolver (endpoints file); else fail loud
# WHEN a DR op actually needs etcd. We DON'T hard-exit at source time (so
# print-config can still show the other defaults on a box without a quorum file) —
# instead each etcd-using script calls dr_require_etcd before touching etcd.
if [ -z "${MCNF_ETCD:-}" ]; then
  if declare -f mcnf_resolve_etcd >/dev/null 2>&1; then
    MCNF_ETCD="$(mcnf_resolve_etcd 2>/dev/null || true)"
  fi
fi
export MCNF_ETCD="${MCNF_ETCD:-}"

export MCNF_AGE_KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
export MCNF_MESHFS_DIR="${MCNF_MESHFS_DIR:-/mnt/mesh-storage}"
export MCNF_FORGEJO_DATA="${MCNF_FORGEJO_DATA:-/var/lib/mcnf-forgejo}"
export MCNF_DR_BUCKET="${MCNF_DR_BUCKET:-mcnf-dr-4533}"
export MCNF_DR_DIR="${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}"

if [ -z "${MCNF_HOST_IP:-}" ]; then
  MCNF_HOST_IP="$(ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}')"
fi
export MCNF_HOST_IP="${MCNF_HOST_IP:-}"

# A DR script that needs etcd calls this first — fails loud if unresolved (NO
# silent .192:2379). Kept here so every DR script shares one error path.
dr_require_etcd() {
  if [ -z "${MCNF_ETCD:-}" ]; then
    echo "dr-env: no etcd endpoints resolved (MCNF_ETCD unset + /etc/mackesd/etcd-endpoints absent)." >&2
    echo "  Run setup-etcd.sh or export MCNF_ETCD=http://<lighthouse-overlay-ip>:2379[,...]. NO .192 default." >&2
    return 1
  fi
  return 0
}

# print-config subcommand (only when EXECUTED, not when sourced).
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  case "${1:-print-config}" in
    print-config|--print|-p)
      cat <<EOF
MCNF_ETCD=${MCNF_ETCD:-<unresolved — run setup-etcd.sh or set MCNF_ETCD>}
MCNF_AGE_KEY=$MCNF_AGE_KEY
MCNF_MESHFS_DIR=$MCNF_MESHFS_DIR
MCNF_FORGEJO_DATA=$MCNF_FORGEJO_DATA
MCNF_DR_BUCKET=$MCNF_DR_BUCKET
MCNF_DR_DIR=$MCNF_DR_DIR
MCNF_HOST_IP=${MCNF_HOST_IP:-<no overlay iface>}
EOF
      ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) echo "usage: dr-env.sh [print-config]" >&2; exit 2 ;;
  esac
fi
