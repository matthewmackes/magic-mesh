#!/usr/bin/env bash
# etcd-endpoints.sh — DAR-1b: the ONE etcd-endpoint resolver shared by every
# backoffice script (state-backend, secrets, DR, the queue lib, reconciler env).
#
# WHY this exists: the backoffice scripts each hard-defaulted MCNF_ETCD to the
# DEAD LAN control node `http://172.20.145.192:2379`. That node is gone; the live
# etcd quorum runs on the LIGHTHOUSES (nyc3 10.42.0.4 / fra1 10.42.0.5 /
# sfo3 10.42.0.6 + Eagle 10.42.0.2) and is recorded — per node — in
# /etc/mackesd/etcd-endpoints by setup-etcd.sh (the SUBSTRATE-1 ↔ SUBSTRATE-2
# contract; see crates/.../substrate/etcd.rs::endpoints_from_file). This resolver
# reads the SAME file so a script talks to whatever mesh it is actually on, and
# FAILS LOUD rather than silently pointing at a dead host.
#
# Resolution order (matches design §2.2 + DAR-1b):
#   1. explicit MCNF_ETCD env (comma-separated http://<ip>:2379 list)  → used as-is
#   2. comma-joined contents of /etc/mackesd/etcd-endpoints            → resolved
#   3. EXIT non-zero with a loud remediation hint (run setup-etcd.sh). NO .192.
#
# Source it, then call:  MCNF_ETCD="$(mcnf_resolve_etcd)" || exit 1
# The parser mirrors substrate/etcd.rs::parse_endpoints: comma / whitespace /
# newline separators, trims, drops blanks + `#` comment lines.

# The file setup-etcd.sh writes (overridable for tests).
MCNF_ETCD_ENDPOINTS_FILE="${MCNF_ETCD_ENDPOINTS_FILE:-/etc/mackesd/etcd-endpoints}"

# Parse a raw endpoints-file body into a comma-joined client-URL list on stdout.
# Pure + testable; no I/O. (mirror of substrate/etcd.rs::parse_endpoints)
mcnf_parse_endpoints() {
  awk '
    { sub(/#.*/, "") }                       # drop inline + whole-line comments
    {
      gsub(/[,\t ]+/, "\n")                   # comma/space/tab -> newline
      n = split($0, parts, "\n")
      for (i = 1; i <= n; i++)
        if (parts[i] != "") print parts[i]
    }
  ' | paste -sd, -
}

# Resolve the etcd client endpoints. Prints the comma-joined list on stdout;
# returns non-zero (and prints a loud hint to stderr) if none can be found.
mcnf_resolve_etcd() {
  # 1. explicit env wins (already a comma list).
  if [ -n "${MCNF_ETCD:-}" ]; then
    printf '%s\n' "$MCNF_ETCD"
    return 0
  fi
  # 2. the live quorum file written by setup-etcd.sh.
  if [ -r "$MCNF_ETCD_ENDPOINTS_FILE" ]; then
    local eps
    eps="$(mcnf_parse_endpoints < "$MCNF_ETCD_ENDPOINTS_FILE")"
    if [ -n "$eps" ]; then
      printf '%s\n' "$eps"
      return 0
    fi
  fi
  # 3. fail loud — NEVER fall back to the dead 172.20.145.192:2379.
  cat >&2 <<EOF
etcd-endpoints: no etcd endpoints resolved.
  MCNF_ETCD env is unset and $MCNF_ETCD_ENDPOINTS_FILE is missing/empty.
  This node is not provisioned onto the mesh etcd quorum.
  Remediation: run setup-etcd.sh (--init on the founding lighthouse, or
  --client-only --anchors <quorum-overlay-ips> on a member), OR export
  MCNF_ETCD=http://<lighthouse-overlay-ip>:2379[,...].
EOF
  return 1
}

# Convenience: first endpoint only (for callers that take a single URL).
mcnf_resolve_etcd_first() {
  local all
  all="$(mcnf_resolve_etcd)" || return 1
  printf '%s\n' "${all%%,*}"
}
