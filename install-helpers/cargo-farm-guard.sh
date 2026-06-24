#!/usr/bin/env bash
# cargo-farm-guard.sh — DRAIN-ENGINE guardrail (hard-enforcement, operator-locked
# 2026-06-24). Installed AS `cargo` (ahead of the real cargo) on the dev host so
# that heavy LOCAL builds are impossible — they fill the disk and wedge the whole
# autonomous drain. Compiles MUST go to the XCP farm via install-helpers/xcp-build.sh
# (which runs cargo on the farm over SSH, never through this shim).
#
# Allowed locally (no big target/): fmt, metadata, tree, --version, locate-project,
# pkgid, read-manifest. Blocked locally: build, test, check, clippy, bench, run,
# install, doc, rustc.
#
# Install/uninstall: install-helpers/install-drain-guardrails.sh {--install|--uninstall}
# The real toolchain is preserved next to the shim as `cargo-real`.
set -uo pipefail

# Resolve the real cargo: prefer the saved sibling, else the first OTHER cargo
# on PATH that is not this shim.
self="$(readlink -f "$0" 2>/dev/null || echo "$0")"
real=""
sib="$(dirname "$self")/cargo-real"
if [ -x "$sib" ]; then
  real="$sib"
else
  IFS=':' read -ra _dirs <<<"$PATH"
  for d in "${_dirs[@]}" "$HOME/.cargo/bin" /usr/bin /usr/local/bin; do
    c="$d/cargo"
    [ -x "$c" ] || continue
    [ "$(readlink -f "$c" 2>/dev/null || echo "$c")" = "$self" ] && continue
    real="$c"; break
  done
fi

case "${1:-}" in
  build|test|check|clippy|bench|run|install|doc|rustc)
    echo "✋ cargo-farm-guard: LOCAL 'cargo $1' is DISABLED on the dev host." >&2
    echo "   Local target/ dirs fill the disk and wedge the autonomous drain" >&2
    echo "   (it happened 4x on 2026-06-24). Build on the XCP farm instead:" >&2
    echo "     ./install-helpers/xcp-build.sh cargo $*" >&2
    echo "   (fmt / metadata / tree run locally and are allowed.)" >&2
    exit 97 ;;
  *)
    if [ -n "$real" ]; then exec "$real" "$@"; fi
    echo "cargo-farm-guard: real cargo not found alongside the shim" >&2; exit 1 ;;
esac
