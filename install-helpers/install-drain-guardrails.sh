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
  local tmp test_home test_bin log expected command rc rustup_bin rustup_home integration_home
  tmp="$(mktemp -d)"
  SELF_TEST_TMP="$tmp"
  trap 'if [ -n "${SELF_TEST_TMP:-}" ]; then rm -rf "$SELF_TEST_TMP"; fi' EXIT
  test_home="$tmp/cargo-home"
  test_bin="$test_home/bin"
  log="$tmp/cargo.log"
  expected="$tmp/expected.log"
  mkdir -p "$test_bin"

  cat >"$test_bin/rustup" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "${1:-}" >>"${CARGO_GUARD_TEST_LOG:?}"
EOF
  chmod 0755 "$test_bin/rustup"
  ln -s rustup "$test_bin/cargo"

  CARGO_HOME="$test_home" "$HERE/install-drain-guardrails.sh" --guard-only >/dev/null
  [ "$(readlink "$test_bin/cargo-real")" = rustup ] || \
    self_test_fail 'installer did not preserve the rustup proxy as cargo-real'

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

  # Exercise the real rustup proxy when available. A shell fixture cannot
  # observe the argv[0] supplied to a shebang interpreter, while rustup's ELF
  # proxy dispatch depends on it. This catches both the cargo-real-name failure
  # and recursive cargo-fmt dispatch that motivated the guard repair.
  rustup_bin="$(command -v rustup || true)"
  rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
  if [ -n "$rustup_bin" ] && [ -x "$rustup_bin" ] && [ -d "$rustup_home" ]; then
    integration_home="$tmp/rustup-cargo-home"
    mkdir -p "$integration_home/bin"
    install -m 0755 "$(readlink -f "$rustup_bin")" "$integration_home/bin/rustup"
    ln -s rustup "$integration_home/bin/cargo"
    ln -s rustup "$integration_home/bin/cargo-fmt"
    CARGO_HOME="$integration_home" \
      "$HERE/install-drain-guardrails.sh" --guard-only >/dev/null
    if CARGO_HOME="$integration_home" RUSTUP_HOME="$rustup_home" \
        "$integration_home/bin/cargo-real" --version \
        >"$tmp/cargo-real.out" 2>"$tmp/cargo-real.err"; then
      self_test_fail 'rustup accepted the preserved cargo-real proxy name'
    fi
    grep -Fq 'unknown proxy name' "$tmp/cargo-real.err" || \
      self_test_fail 'rustup did not reproduce the cargo-real proxy failure'
    PATH="$integration_home/bin:$PATH" CARGO_HOME="$integration_home" \
      RUSTUP_HOME="$rustup_home" "$integration_home/bin/cargo" fmt --version \
      >"$tmp/cargo-fmt-version.out"
    grep -Fq 'rustfmt ' "$tmp/cargo-fmt-version.out" || \
      self_test_fail 'guarded cargo fmt did not dispatch to rustfmt'
  fi

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
