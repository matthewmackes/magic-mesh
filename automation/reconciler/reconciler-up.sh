#!/usr/bin/env bash
# reconciler-up.sh — DAR-30: install BOTH reconciler timers plus the HEAD path
# unit on the control VM, PLAN-ONLY, the way state-backend-up.sh stands up its
# service. One idempotent script: render /etc/mcnf/reconciler.env (DAR-27),
# ensure the etcd /reconciler/* prefix (DAR-28), drop the units with
# EnvironmentFile + WorkingDirectory=/opt/mcnf, and enable both timers and
# mcnf-farm-reconcile.path WITHOUT arming the live apply.
#
#   - 5-min  autoscale reconciler (mcnf-farm-autoscale-reconcile) — apply-CAPABLE
#            but PLAN-ONLY here: FA_APPLY is left UNSET (the unit carries no inline
#            Environment=FA_APPLY=1). Arming the live apply is a SEPARATE explicit
#            operator step (install-helpers/enable-autoscale-timer.sh) — never this
#            script, never genesis, never the AI.
#   - 15-min @farm build reconciler (mcnf-farm-reconcile) — dispatches @farm jobs;
#            no apply gate (it never mutates infra).
#   - HEAD-change path unit (mcnf-farm-reconcile.path) — starts the same oneshot
#            when $REPO/.git/logs/HEAD is written (same-branch commit) or
#            .git/HEAD changes, so a commit does not wait up to 15 min idle.
#
# The units point at the DEDICATED release slot ${MCNF_REPO:-/opt/mcnf} — NEVER the
# resettable .52 build dir (the CI gremlin) and NEVER a .claude/worktrees path.
#
# Usage:  reconciler-up.sh [--repo <dir>] [--xcp-host <ip>] [--dry-run] [--self-test]
#   --dry-run    print what would be written + installed; mutate NOTHING.
#   --self-test  unit-shape + worktree-refusal checks (no systemd, no etcd).
#
# Env: MCNF_REPO (default /opt/mcnf), MCNF_XCP_HOST, MCNF_ETCD (or the endpoints
#      file), RECONCILER_ENV_OUT (default /etc/mcnf/reconciler.env).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${MCNF_REPO:-/opt/mcnf}"
ENV_OUT="${RECONCILER_ENV_OUT:-/etc/mcnf/reconciler.env}"
XCP_HOST="${MCNF_XCP_HOST:-}"
DRY=0
SELF_TEST=0
SYSTEMD_DIR="${MCNF_SYSTEMD_DIR:-/etc/systemd/system}"

while [ $# -gt 0 ]; do
  case "$1" in
    --repo)      REPO="$2"; shift 2 ;;
    --xcp-host)  XCP_HOST="$2"; shift 2 ;;
    --dry-run)   DRY=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help)   sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "reconciler-up: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

# Packaged unit templates come from the checkout this script LIVES in (so a
# --dry-run works before /opt/mcnf exists); the units they generate point WorkingDir
# at $REPO (the deploy slot). SRC_REPO = the running checkout root.
SRC_REPO="$(cd "$HERE/../.." && pwd)"
PKG="$SRC_REPO/packaging/systemd"
say() { echo "==> reconciler-up: $*"; }

# A unit WorkingDirectory that is a .claude/worktrees path dies 200/CHDIR the
# moment that worktree is removed (observed 2026-08-23: calm-ray-dcr8). Refuse
# at install time so the timer cannot be pointed at a disposable tree.
assert_repo_slot() {
  case "$1" in
    */.claude/worktrees/*|*/.claude/worktrees)
      echo "reconciler-up: refusing worktree slot $1 (deleted worktrees fail systemd with 200/CHDIR)" >&2
      return 2
      ;;
  esac
  return 0
}

# The two reconcile units, regenerated to point at the deployed slot via an
# EnvironmentFile (so render-env.sh owns every per-mesh value). The autoscale unit
# deliberately carries NO Environment=FA_APPLY=1 — it is plan-only until the
# operator runs enable-autoscale-timer.sh.
autoscale_unit() {
  cat <<EOF
# DAR-30 — FARM-AUTOSCALE reconciler, PLAN-ONLY on the control VM. Demand ->
# per-dom0 shapes -> (gate green AND FA_APPLY=1) tofu apply. Installed with NO
# inline FA_APPLY: this unit is plan-only until enable-autoscale-timer.sh arms it.
# Per-mesh config (repo slot, etcd quorum, XAPI gate host, golden template) comes
# from /etc/mcnf/reconciler.env (rendered by render-env.sh) — no .192/XO literals.
[Unit]
Description=MCNF FARM-AUTOSCALE reconciler (demand -> per-dom0 VM shapes; plan-only)
After=network-online.target mcnf-reconciler-bootstrap.service
Wants=network-online.target mcnf-reconciler-bootstrap.service

[Service]
Type=oneshot
Environment=HOME=/root
EnvironmentFile=$ENV_OUT
WorkingDirectory=$REPO
ExecStart=/bin/bash $REPO/install-helpers/farm-reconciler.sh --once
TimeoutStartSec=3600
Nice=10
EOF
}

build_unit() {
  cat <<EOF
# DAR-30 — @farm build reconciler on the control VM. Converges the worklist's
# active @farm jobs onto the fleet every 15 min. No apply gate (never mutates
# infra). Per-mesh config from /etc/mcnf/reconciler.env (no .192/XO literals).
[Unit]
Description=MCNF build-farm reconciler (worklist @farm jobs -> fleet builds)
After=network-online.target mcnf-reconciler-bootstrap.service
Wants=network-online.target mcnf-reconciler-bootstrap.service

[Service]
Type=oneshot
Environment=HOME=/root
EnvironmentFile=$ENV_OUT
WorkingDirectory=$REPO
ExecStart=/bin/bash $REPO/automation/reconciler/farm-reconcile.sh
TimeoutStartSec=3600
Nice=10
EOF
}

# Same oneshot as the 15-min timer; fires when HEAD moves so agents do not wait
# for OnUnitActiveSec after a commit. Watches the deployed slot only.
build_path_unit() {
  cat <<EOF
# DAR-30 — start mcnf-farm-reconcile.service when HEAD changes. Complements
# the 15-min timer; does not replace it. Never a disposable agent worktree.
[Unit]
Description=MCNF build-farm reconciler — start on HEAD change

[Path]
PathModified=$REPO/.git/logs/HEAD
PathModified=$REPO/.git/HEAD
Unit=mcnf-farm-reconcile.service

[Install]
WantedBy=paths.target
EOF
}

install_file() { # <dest> <generator-fn|src-path>
  local dest="$1" src="$2"
  if [ "$DRY" -eq 1 ]; then
    echo "--- would install $dest:"
    if declare -f "$src" >/dev/null 2>&1; then "$src"; else cat "$src"; fi
    echo "---"
    return 0
  fi
  if declare -f "$src" >/dev/null 2>&1; then "$src" >"$dest"; else cp "$src" "$dest"; fi
}

self_test() {
  local fails=0 unit
  echo "reconciler-up --self-test:"
  chk() { if [ "$2" = "$3" ]; then echo "  ok: $1"; else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails+1)); fi; }
  if assert_repo_slot "/root/magic-mesh"; then chk "live control-host tree accepted" yes yes; else chk "live control-host tree accepted" no yes; fi
  if assert_repo_slot "/root/magic-mesh/.claude/worktrees/calm-ray-dcr8"; then
    chk "worktree slot refused" accepted refused
  else
    chk "worktree slot refused" refused refused
  fi
  REPO=/opt/mcnf
  unit="$(build_unit)"
  case "$unit" in *"bash -l"*) chk "build unit has no login shell" has-lc no-lc ;; *) chk "build unit has no login shell" no-lc no-lc ;; esac
  case "$unit" in
    *"/bin/bash /opt/mcnf/automation/reconciler/farm-reconcile.sh"*) chk "build unit execs via bash" yes yes ;;
    *) chk "build unit execs via bash" no yes ;;
  esac
  case "$unit" in *".claude/worktrees"*) chk "build unit has no worktree path" has none ;; *) chk "build unit has no worktree path" none none ;; esac
  unit="$(autoscale_unit)"
  case "$unit" in *"bash -l"*) chk "autoscale unit has no login shell" has-lc no-lc ;; *) chk "autoscale unit has no login shell" no-lc no-lc ;; esac
  case "$unit" in
    *"/bin/bash /opt/mcnf/install-helpers/farm-reconciler.sh --once"*) chk "autoscale unit execs via bash" yes yes ;;
    *) chk "autoscale unit execs via bash" no yes ;;
  esac
  case "$unit" in *"infra/tofu/env.sh"*) chk "autoscale unit does not source missing env.sh" has none ;; *) chk "autoscale unit does not source missing env.sh" none none ;; esac
  unit="$(build_path_unit)"
  case "$unit" in
    *"PathModified=/opt/mcnf/.git/logs/HEAD"*) chk "path unit watches deployed logs/HEAD writes" yes yes ;;
    *) chk "path unit watches deployed logs/HEAD writes" no yes ;;
  esac
  case "$unit" in
    *"Unit=mcnf-farm-reconcile.service"*) chk "path unit starts existing oneshot" yes yes ;;
    *) chk "path unit starts existing oneshot" no yes ;;
  esac
  case "$unit" in *".claude/worktrees"*) chk "path unit has no worktree path" has none ;; *) chk "path unit has no worktree path" none none ;; esac
  if [ "$fails" -eq 0 ]; then echo "reconciler-up: self-test passed"; return 0; fi
  echo "reconciler-up: SELF-TEST FAILED ($fails)" >&2; return 1
}

if [ "$SELF_TEST" -eq 1 ]; then
  self_test
  exit $?
fi

assert_repo_slot "$REPO" || exit 2
if [ "$DRY" -eq 0 ] && [ ! -d "$REPO" ]; then
  echo "reconciler-up: repo slot $REPO does not exist — pass --repo <checkout>" >&2
  exit 2
fi
if [ "$DRY" -eq 0 ] && [ ! -f "$REPO/automation/reconciler/farm-reconcile.sh" ]; then
  echo "reconciler-up: $REPO is not a magic-mesh checkout (missing automation/reconciler/farm-reconcile.sh)" >&2
  exit 2
fi

# 1) Render the env file (idempotent).
say "render $ENV_OUT (etcd quorum, repo slot, XAPI gate host, golden template)"
if [ "$DRY" -eq 1 ]; then
  MCNF_REPO="$REPO" MCNF_XCP_HOST="$XCP_HOST" RECONCILER_ENV_OUT="$ENV_OUT" \
    bash "$HERE/render-env.sh" --print
else
  MCNF_REPO="$REPO" MCNF_XCP_HOST="$XCP_HOST" RECONCILER_ENV_OUT="$ENV_OUT" \
    bash "$HERE/render-env.sh"
fi

# 2) Install the bootstrap oneshot (ensures /reconciler/* prefix) + the units +
#    the two timers (timers are static — copied from packaging/systemd verbatim)
#    + the HEAD path unit (generated against $REPO).
say "install units into $SYSTEMD_DIR (plan-only — FA_APPLY UNSET)"
install_file "$SYSTEMD_DIR/mcnf-reconciler-bootstrap.service" "$PKG/mcnf-reconciler-bootstrap.service"
install_file "$SYSTEMD_DIR/mcnf-farm-autoscale-reconcile.service" autoscale_unit
install_file "$SYSTEMD_DIR/mcnf-farm-reconcile.service" build_unit
install_file "$SYSTEMD_DIR/mcnf-farm-reconcile.path" build_path_unit
install_file "$SYSTEMD_DIR/mcnf-farm-autoscale-reconcile.timer" "$PKG/mcnf-farm-autoscale-reconcile.timer"
install_file "$SYSTEMD_DIR/mcnf-farm-reconcile.timer" "$PKG/mcnf-farm-reconcile.timer"

if [ "$DRY" -eq 1 ]; then
  say "--dry-run: nothing installed; FA_APPLY would remain UNSET (plan-only)."
  exit 0
fi

# 3) Enable both timers + the bootstrap oneshot. NOT arming FA_APPLY.
systemctl daemon-reload
systemctl enable --now mcnf-reconciler-bootstrap.service || true
systemctl enable --now mcnf-farm-autoscale-reconcile.timer
systemctl enable --now mcnf-farm-reconcile.timer
systemctl enable --now mcnf-farm-reconcile.path

say "timers + HEAD path unit enabled (plan-only):"
systemctl list-timers mcnf-farm-autoscale-reconcile.timer mcnf-farm-reconcile.timer --no-pager || true
systemctl is-enabled mcnf-farm-reconcile.path --no-pager || true
cat <<EOF

reconciler-up: DONE. Both timers and the HEAD path unit are active, PLAN-ONLY.
  - autoscale (5-min): apply-capable but FA_APPLY is UNSET → every tick logs
    'apply-gate … → plan-only (FA_APPLY!=1)'. No VM is touched.
  - @farm build (15-min timer + HEAD path): dispatches build jobs; never mutates infra.

To ARM the live autoscale apply (the ONE explicit operator step — never the AI):
  sudo bash install-helpers/enable-autoscale-timer.sh
EOF
