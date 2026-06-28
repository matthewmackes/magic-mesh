#!/usr/bin/env bash
# render-env.sh — DAR-27: render /etc/mcnf/reconciler.env so the reconciler's
# control-IP, XAPI/XO URL, etcd endpoint, repo path, golden-template name and dom0
# topology are resolved AT DEPLOY TIME instead of baked as `.192`/XO literals in
# farm-reconciler.sh. The loop then runs unchanged on a new XCP-NG machine: only
# this file changes per mesh.
#
# WHAT it de-hardcodes (the v2 corrections):
#   - MCNF_ETCD       ← /etc/mackesd/etcd-endpoints (via the DAR-1b resolver), NOT
#                       the dead http://172.20.145.192:2379. Fails loud if absent.
#   - MCNF_REPO       ← the deployed checkout (a DEDICATED release slot, never the
#                       resettable .52 build dir — the CI gremlin). Default /opt/mcnf.
#   - MCNF_XCP_HOST   ← the FOUNDING dom0 XAPI host; the apply-reachability probe
#                       targets its :443 (DAR-29 swaps the dead XO ws for this).
#   - MCNF_XO_URL     ← kept ONLY for back-compat status output; no longer the gate.
#   - MCNF_GOLDEN_TEMPLATE ← MDE-VM-golden (canonical; the -tc drift is retired).
#   - the per-dom0 host/IP map → emitted from tofu state (local.dom0) when present,
#     else the live cold facts, so a new dom0 layout regenerates rather than
#     inheriting the LAN's three pools.
#
# Sourcing precedence for each value: explicit env > tofu state/site facts > the
# live LAN cold-fact fallback (so on `.192` the rendered file reproduces today's
# host values EXCEPT MCNF_ETCD, which now carries the live lighthouse quorum).
#
# Usage:
#   render-env.sh [--out <path>] [--control-ip <ip>] [--repo <dir>]
#                 [--xcp-host <ip>] [--print]
#     --print   write to stdout instead of $OUT (dry preview; mutates nothing)
#
# Env overrides (all optional — each wins over the fallback):
#   MCNF_ETCD, MCNF_REPO, MCNF_XCP_HOST, MCNF_XO_URL, MCNF_GOLDEN_TEMPLATE,
#   MCNF_CONTROL_IP, RECONCILER_ENV_OUT (default /etc/mcnf/reconciler.env),
#   MCNF_TOFU_DIR (default <repo>/infra/tofu — where local.dom0 lives).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/etcd-endpoints.sh
. "$HERE/../lib/etcd-endpoints.sh"
# shellcheck source=../lib/control-host.sh
. "$HERE/../lib/control-host.sh"   # DAR-17: portable control-HOST resolver

OUT="${RECONCILER_ENV_OUT:-/etc/mcnf/reconciler.env}"
PRINT=0
CONTROL_IP="${MCNF_CONTROL_IP:-}"
REPO="${MCNF_REPO:-/opt/mcnf}"
XCP_HOST="${MCNF_XCP_HOST:-}"
GOLDEN="${MCNF_GOLDEN_TEMPLATE:-MDE-VM-golden}"   # canonical; never -tc (DAR-34)

while [ $# -gt 0 ]; do
  case "$1" in
    --out)        OUT="$2"; shift 2 ;;
    --control-ip) CONTROL_IP="$2"; shift 2 ;;
    --repo)       REPO="$2"; shift 2 ;;
    --xcp-host)   XCP_HOST="$2"; shift 2 ;;
    --print)      PRINT=1; shift ;;
    -h|--help)    sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "render-env: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

TOFU_DIR="${MCNF_TOFU_DIR:-$REPO/infra/tofu}"

# ── MCNF_ETCD: the live quorum, NOT the dead .192:2379 (DAR-1b). Fail loud. ──
ETCD="$(mcnf_resolve_etcd)" || {
  echo "render-env: cannot resolve etcd endpoints — refusing to render a reconciler.env without a quorum." >&2
  exit 1
}

# ── the founding dom0 XAPI host (the DAR-29 apply-reachability probe target) ──
# Precedence: explicit --xcp-host/env > the first dom0 host in tofu local.dom0 >
# the live BigBoy fallback (172.20.145.165). We read local.dom0's pool→host by a
# light grep of main.tf comments (`# <ip> — …`) when present; the authoritative
# host is whatever the operator passes for a fresh mesh.
if [ -z "$XCP_HOST" ]; then
  # Try the cold facts in main.tf (pool comment carries the dom0 IP).
  if [ -f "$TOFU_DIR/main.tf" ]; then
    XCP_HOST="$(grep -oE '172\.20\.[0-9]+\.[0-9]+' "$TOFU_DIR/main.tf" 2>/dev/null | head -1 || true)"
  fi
  [ -n "$XCP_HOST" ] || XCP_HOST="172.20.145.165"   # live BigBoy fallback
fi

# ── control HOST (DAR-17): the per-mesh control VM, resolved via the shared chain
# (explicit MCNF_CONTROL_IP > the /mcnf/site doc > the peer directory > this node's
# overlay), NEVER the dead .192. The reconstitute arm passes =172.20.145.192. ──
CONTROL_IP="$(MCNF_CONTROL_IP="$CONTROL_IP" mcnf_resolve_control_host)"

# ── XO URL (back-compat status only; XO is retired — no longer the apply gate). ──
# Built from the resolved control host, not a dead-LAN literal; if the host is
# un-resolvable the URL is left empty (status-only, never gates an apply).
if [ -n "${MCNF_XO_URL:-}" ]; then
  XO_URL="$MCNF_XO_URL"
elif [ -n "$CONTROL_IP" ]; then
  XO_URL="ws://${CONTROL_IP}:8080"
else
  XO_URL=""
fi

# ── render ──
TS="$(date -u +%FT%TZ 2>/dev/null || echo unknown)"
render() {
  cat <<EOF
# /etc/mcnf/reconciler.env — DAR-27 deploy-time reconciler config (rendered $TS).
# Sourced by mcnf-farm-autoscale-reconcile.service + mcnf-farm-reconcile.service
# (EnvironmentFile=). Regenerate with automation/reconciler/render-env.sh — do NOT
# hand-edit; per-mesh values come from the founding bundle / tofu state.
#
# NO dead .192:2379 etcd default; NO XO websocket as the apply gate (the gate is
# the founding-dom0 XAPI :443, DAR-29). NO .claude/worktrees path.

# The deployed checkout — a DEDICATED release slot, NOT the resettable .52 build
# dir (the CI gremlin that resets to b6d4ca0 and would run stale code).
MCNF_REPO=$REPO

# The live etcd quorum (lighthouses), resolved from /etc/mackesd/etcd-endpoints.
MCNF_ETCD=$ETCD

# The xen-xapi root (no-XO, XAPI-native) the reconciler plans/applies (DAR-29).
MCNF_TOFU_DIR=$REPO/infra/tofu/xen-xapi

# The FOUNDING dom0 XAPI host — the apply-reachability probe targets its :443.
MCNF_XCP_HOST=$XCP_HOST

# XO URL retained for status output only (XO is dead live; not the gate).
MCNF_XO_URL=$XO_URL

# The ONE canonical golden template (DAR-34) — never MDE-VM-golden-tc.
MCNF_GOLDEN_TEMPLATE=$GOLDEN
TF_VAR_golden_template_name=$GOLDEN

# Reconciler durable state lives in etcd /reconciler/* (DAR-28), not /var/lib.
# The seam farm-reconciler.sh reads to route busy-state through reconciler-state.sh.
MCNF_RECONCILER_STATE_ETCD=1
EOF
}

if [ "$PRINT" -eq 1 ]; then
  render
  exit 0
fi

mkdir -p "$(dirname "$OUT")"
tmp="$(mktemp)"
render >"$tmp"
# Idempotent: only rewrite if the content (minus the rendered-at timestamp) changed.
if [ -f "$OUT" ] && diff -q <(grep -v '^# /etc/mcnf/reconciler.env' "$tmp") <(grep -v '^# /etc/mcnf/reconciler.env' "$OUT") >/dev/null 2>&1; then
  rm -f "$tmp"
  echo "render-env: $OUT unchanged (idempotent)"
else
  mv "$tmp" "$OUT"
  chmod 644 "$OUT"
  echo "render-env: wrote $OUT (etcd=$ETCD repo=$REPO xcp=$XCP_HOST golden=$GOLDEN)"
fi
