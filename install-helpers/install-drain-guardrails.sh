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
  # Refresh an installed guard so a reinstall deploys repository fixes.
  if grep -q cargo-farm-guard "$real" 2>/dev/null; then
    if [ ! -x "$CARGO_BIN/cargo-real" ]; then
      echo "cargo guard is installed but saved real cargo is missing: $CARGO_BIN/cargo-real" >&2
      return 1
    fi
    install -m 0755 "$HERE/cargo-farm-guard.sh" "$real"
    echo "cargo guard refreshed -> $real"
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
self_test_fail() {
  echo "install-drain-guardrails.sh: SELF-TEST FAILED — $*" >&2
  exit 1
}
self_test() {
  local tmp test_home test_bin log expected unknown_err command rc
  tmp="$(mktemp -d)"
  SELF_TEST_TMP="$tmp"
  trap 'if [ -n "${SELF_TEST_TMP:-}" ]; then rm -rf "$SELF_TEST_TMP"; fi' EXIT
  test_home="$tmp/cargo-home"
  test_bin="$test_home/bin"
  log="$tmp/cargo.log"
  expected="$tmp/expected.log"
  unknown_err="$tmp/unknown-proxy.err"
  mkdir -p "$test_bin"

  cat >"$test_bin/rustup" <<'EOF'
#!/usr/bin/env bash
set -eu
proxy="${RUSTUP_FORCE_ARG0:-$(basename "$0")}"
if [ "$proxy" != cargo ]; then
  echo "error: unknown proxy name: $proxy" >&2
  exit 1
fi
printf '%s\n' "${1:-}" >>"${CARGO_GUARD_TEST_LOG:?}"
EOF
  chmod 0755 "$test_bin/rustup"
  ln -s rustup "$test_bin/cargo"

  CARGO_HOME="$test_home" "$HERE/install-drain-guardrails.sh" --guard-only >/dev/null
  [ "$(readlink "$test_bin/cargo-real")" = rustup ] || \
    self_test_fail 'installer did not preserve the rustup proxy as cargo-real'

  if CARGO_GUARD_TEST_LOG="$log" "$test_bin/cargo-real" fmt >/dev/null 2>"$unknown_err"; then
    self_test_fail 'rustup fixture accepted the cargo-real proxy name'
  fi
  grep -Fq 'unknown proxy name: cargo-real' "$unknown_err" || \
    self_test_fail 'rustup fixture did not reproduce the cargo-real proxy failure'

  for command in fmt metadata; do
    CARGO_GUARD_TEST_LOG="$log" "$test_bin/cargo" "$command" >/dev/null || \
      self_test_fail "cargo $command did not reach the preserved proxy"
  done
  printf 'fmt\nmetadata\n' >"$expected"
  cmp -s "$expected" "$log" || \
    self_test_fail 'allowed commands were not delegated exactly once'

  for command in build test check clippy run; do
    if CARGO_GUARD_TEST_LOG="$log" "$test_bin/cargo" "$command" \
        >"$tmp/$command.out" 2>"$tmp/$command.err"; then
      self_test_fail "cargo $command was allowed"
    else
      rc=$?
    fi
    [ "$rc" -eq 97 ] || self_test_fail "cargo $command exited $rc instead of 97"
  done
  cmp -s "$expected" "$log" || \
    self_test_fail 'a blocked command reached the preserved proxy'

  printf '# stale installed guard\n' >>"$test_bin/cargo"
  CARGO_HOME="$test_home" "$HERE/install-drain-guardrails.sh" --guard-only >/dev/null
  cmp -s "$HERE/cargo-farm-guard.sh" "$test_bin/cargo" || \
    self_test_fail 'reinstall did not refresh the installed guard'

  echo 'install-drain-guardrails.sh: self-test passed'
}
case "$MODE" in
  --install)     install_guard; install_timer;;
  --uninstall)   uninstall_guard; uninstall_timer;;
  --guard-only)  install_guard;;
  --timer-only)  install_timer;;
  --self-test)   self_test;;
  *) echo "usage: $0 {--install|--uninstall|--guard-only|--timer-only|--self-test}"; exit 2;;
esac
