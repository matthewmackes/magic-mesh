#!/usr/bin/env bash
# install-drain-guardrails.sh — install/uninstall the DRAIN-ENGINE hard
# guardrails (operator-locked 2026-06-24): (1) cargo-farm-guard as `cargo` so
# local builds are impossible, (2) a 5-min systemd timer running disk-watchdog.
# Reversible: the real toolchain is preserved as `cargo-real`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MODE="${1:---install}"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

install_guard() {
  local real="$CARGO_BIN/cargo"
  if [ ! -x "$real" ]; then echo "no cargo at $real — skipping guard"; return; fi
  # Already guarded? (the installed cargo execs cargo-real)
  if [ -x "$CARGO_BIN/cargo-real" ] && grep -q cargo-farm-guard "$real" 2>/dev/null; then
    echo "cargo guard already installed";
  else
    [ -x "$CARGO_BIN/cargo-real" ] || cp -a "$real" "$CARGO_BIN/cargo-real"
    install -m 0755 "$HERE/cargo-farm-guard.sh" "$real"
    echo "cargo guard installed -> $real (real preserved as $CARGO_BIN/cargo-real)"
  fi
}
uninstall_guard() {
  if [ -x "$CARGO_BIN/cargo-real" ]; then
    mv -f "$CARGO_BIN/cargo-real" "$CARGO_BIN/cargo"; echo "restored real cargo"
  fi
}
install_timer() {
  command -v systemctl >/dev/null || { echo "no systemd — run disk-watchdog.sh from the loop instead"; return; }
  cat >/etc/systemd/system/mcnf-disk-watchdog.service <<EOF
[Unit]
Description=MCNF dev-host disk watchdog (DRAIN-ENGINE guardrail)
[Service]
Type=oneshot
ExecStart=$HERE/disk-watchdog.sh 8
EOF
  cat >/etc/systemd/system/mcnf-disk-watchdog.timer <<EOF
[Unit]
Description=Run the MCNF disk watchdog every 5 minutes
[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
[Install]
WantedBy=timers.target
EOF
  systemctl daemon-reload && systemctl enable --now mcnf-disk-watchdog.timer
  echo "disk-watchdog.timer installed + started (every 5 min)"
}
uninstall_timer() {
  systemctl disable --now mcnf-disk-watchdog.timer 2>/dev/null || true
  rm -f /etc/systemd/system/mcnf-disk-watchdog.{service,timer}
  systemctl daemon-reload 2>/dev/null || true
}
case "$MODE" in
  --install)     install_guard; install_timer;;
  --uninstall)   uninstall_guard; uninstall_timer;;
  --guard-only)  install_guard;;
  --timer-only)  install_timer;;
  *) echo "usage: $0 {--install|--uninstall|--guard-only|--timer-only}"; exit 2;;
esac
