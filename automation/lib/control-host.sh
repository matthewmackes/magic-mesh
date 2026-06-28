#!/usr/bin/env bash
# control-host.sh — DAR-17 (DEVOPS-AUTOMATION-REBUILD §2.7): resolve the backoffice
# HOST (MCNF_CONTROL_IP / MCNF_BACKOFFICE_HOST) to the founding mesh's control VM
# overlay IP, so the backoffice COMES ALONG to a new Nebula instead of phoning the
# dead LAN node 172.20.145.192.
#
# WHY this exists: scripts used to default the control HOST to the literal
# 172.20.145.192 (the hand-built LAN node). On a NEW mesh that host does not exist;
# the state-backend/secret/Forgejo endpoints all live on the per-mesh control VM's
# OVERLAY IP. This resolver decouples the HOST from the etcd ENDPOINT (DAR-1b owns
# the latter) and gives one fail-soft resolution chain every script can share.
#
# Resolution order (most-specific wins):
#   1. explicit MCNF_CONTROL_IP / MCNF_BACKOFFICE_HOST env   → used verbatim
#      (export MCNF_CONTROL_IP=172.20.145.192 = the explicit RECONSTITUTE ARM:
#       reproduce today's LAN host byte-for-byte; it is opt-in only, never a default)
#   2. the per-mesh identity doc /mcnf/site/control-overlay-ip  (written at found
#      time by mcnf-config.sh gen — DAR-4); the portable, self-describing source
#   3. the control VM's own overlay IP via `mackesd peers --json` (the peer whose
#      name_label/notes mark it the control VM), else THIS node's overlay iface
#   4. this node's nebula/mde-neb overlay IPv4 (running on the control VM itself)
#
# There is NO hardcoded 172.20.145.192 default anywhere in the chain — the only way
# to get that host is to pass it explicitly (the reconstitute arm).
#
# Source it, then call:  HOST="$(mcnf_resolve_control_host)"   (empty = un-resolvable)
#
# Env:
#   MCNF_CONTROL_IP        explicit override (the reconstitute arm uses .192).
#   MCNF_BACKOFFICE_HOST   alias for MCNF_CONTROL_IP (back-compat with older docs).
#   MCNF_CONTROL_OVERLAY_IFACE  overlay iface name match (default nebula|mde-neb).

# This node's Nebula overlay IPv4 (the only address the backoffice binds — lock 7).
mcnf_overlay_ip() {
  local pat="${MCNF_CONTROL_OVERLAY_IFACE:-nebula|mde-neb}"
  ip -o -4 addr show 2>/dev/null \
    | awk -v pat="$pat" '$2 ~ pat {split($4,a,"/"); print a[1]; exit}'
}

# Look up the control VM's overlay IP from the joined-peer directory. The control
# VM is tagged (name_label / notes) "mcnf-control"; fall back to empty if absent or
# `mackesd` isn't on PATH (e.g. running off-mesh). NEVER errors the caller.
mcnf_control_ip_from_peers() {
  command -v mackesd >/dev/null 2>&1 || return 0
  mackesd peers --json 2>/dev/null | python3 -c '
import sys, json
try:
    peers = json.load(sys.stdin)
except Exception:
    sys.exit(0)
# Accept either a bare list or {"peers":[...]}.
if isinstance(peers, dict):
    peers = peers.get("peers") or peers.get("nodes") or []
for p in peers if isinstance(peers, list) else []:
    if not isinstance(p, dict):
        continue
    name = str(p.get("name") or p.get("name_label") or p.get("hostname") or "")
    notes = str(p.get("notes") or p.get("role") or "")
    ip = p.get("overlay_ip") or p.get("ip") or p.get("vpn_ip") or ""
    if ("mcnf-control" in name or "control" in notes) and ip:
        print(str(ip).split("/")[0]); break
' 2>/dev/null || true
}

# Read the per-mesh control overlay IP from the DAR-4 identity doc. Uses the shared
# etcd resolver + the mcnf-config accessor when available; fail-soft (empty).
mcnf_control_ip_from_site() {
  local here site
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  [ -r "$here/mcnf-config.sh" ] || return 0
  # Source the lib (its sourcing guard keeps it side-effect-free) and ask for the
  # one field; suppress its fail-loud if etcd is un-resolvable here.
  ( . "$here/mcnf-config.sh" 2>/dev/null
    site="$(mcnf_config_get_field control-overlay-ip 2>/dev/null || true)"
    [ -n "$site" ] && printf '%s\n' "$site"
  ) 2>/dev/null || true
}

# The single resolution chain. Prints the resolved HOST on stdout (may be empty if
# nothing resolves — callers decide whether that is fatal). NO .192 default.
mcnf_resolve_control_host() {
  # 1. explicit env (MCNF_CONTROL_IP, or its MCNF_BACKOFFICE_HOST alias).
  local explicit="${MCNF_CONTROL_IP:-${MCNF_BACKOFFICE_HOST:-}}"
  if [ -n "$explicit" ]; then
    printf '%s\n' "$explicit"
    return 0
  fi
  # 2. the per-mesh identity doc (the portable, self-describing source).
  local v
  v="$(mcnf_control_ip_from_site)"
  if [ -n "$v" ]; then printf '%s\n' "$v"; return 0; fi
  # 3. the control VM in the peer directory.
  v="$(mcnf_control_ip_from_peers)"
  if [ -n "$v" ]; then printf '%s\n' "$v"; return 0; fi
  # 4. this node's own overlay IP (we ARE the control VM).
  v="$(mcnf_overlay_ip)"
  if [ -n "$v" ]; then printf '%s\n' "$v"; return 0; fi
  # Un-resolvable: print nothing, let the caller fail loud with its own message.
  return 0
}
