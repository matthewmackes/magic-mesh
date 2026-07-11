#!/usr/bin/env bash
# enable-ci-gate.sh — install + enable the always-on farm CI gate (test-obs-1, P0).
#
# This is the ONE step the AI does not self-perform: starting a self-perpetuating
# recurring farm job (it dispatches heavy BigBoy builds on a schedule) removes the
# human-in-the-loop, so it requires an explicit operator action. Running this
# script IS that authorization.
#
# It installs four units into $MCNF_SYSTEMD_DIR (default /etc/systemd/system):
#   mcnf-ci-gate.service/.timer            — poll origin/master; gate on advance
#   mcnf-ci-gate-liveness.service/.timer   — dead-man alert if the gate goes stale
# then enables both timers and fires ONE liveness check now (instant, proves the
# Bus wiring). It does NOT auto-fire a full gate — the gate timer picks that up on
# its own cadence, or trigger one immediately with the printed command.
#
# Idempotent — safe to re-run. Reverse with:
#   systemctl disable --now mcnf-ci-gate.timer mcnf-ci-gate-liveness.timer
#
# Env: MCNF_REPO (deployed checkout; default /root/magic-mesh — the same slot the
#      nightly-tests timer uses), MCNF_SYSTEMD_DIR (default /etc/systemd/system).
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root: sudo bash $0"; exit 1; }

REPO="${MCNF_REPO:-/root/magic-mesh}"
SYSTEMD_DIR="${MCNF_SYSTEMD_DIR:-/etc/systemd/system}"
SRC="$REPO/packaging/systemd"

[ -d "$REPO" ]  || { echo "MCNF_REPO=$REPO does not exist — set MCNF_REPO to the deployed checkout" >&2; exit 1; }
[ -x "$REPO/install-helpers/ci-gate.sh" ] || { echo "missing $REPO/install-helpers/ci-gate.sh" >&2; exit 1; }

UNITS=(
  mcnf-ci-gate.service
  mcnf-ci-gate.timer
  mcnf-ci-gate-liveness.service
  mcnf-ci-gate-liveness.timer
)

for u in "${UNITS[@]}"; do
  [ -f "$SRC/$u" ] || { echo "missing packaged unit $SRC/$u" >&2; exit 1; }
  if [ "$REPO" = /root/magic-mesh ]; then
    cp "$SRC/$u" "$SYSTEMD_DIR/$u"
  else
    # Rewrite the hardcoded /root/magic-mesh paths to the deployed slot.
    sed "s#/root/magic-mesh#$REPO#g" "$SRC/$u" > "$SYSTEMD_DIR/$u"
  fi
  echo "installed $SYSTEMD_DIR/$u"
done

systemctl daemon-reload
systemctl enable --now mcnf-ci-gate.timer mcnf-ci-gate-liveness.timer

echo "== firing one liveness check now (instant; proves the Bus wiring) =="
systemctl start mcnf-ci-gate-liveness.service || true

systemctl list-timers 'mcnf-ci-gate*' --no-pager || true
echo
echo "ENABLED:"
echo "  · mcnf-ci-gate.timer          polls origin/master every 20 min; gates on advance (routes to BigBoy)"
echo "  · mcnf-ci-gate-liveness.timer checks freshness every 6h; alerts if the gate goes stale (>= 2d)"
echo "repo slot: $REPO"
echo
echo "Trigger a full gate against current master right now:"
echo "  systemctl start mcnf-ci-gate.service    # runs ci-gate.sh poll (gates if master advanced)"
echo "  $REPO/install-helpers/ci-gate.sh run    # force a gate of the current checkout"
echo
echo "Point the Bus publish at the operator's live shell node if it is not Eagle (.13):"
echo "  add  Environment=MCNF_CI_BUS_HOST=<ip>  to $SYSTEMD_DIR/mcnf-ci-gate.service"
