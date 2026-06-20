#!/bin/bash
# setup-caddy.sh — CONNECT-4 (packaging half): install + prepare Caddy as the
# public reverse-proxy ingress on a lighthouse. The mackesd `connect_firewall`
# worker renders/writes the per-service ingress fragment to
# /etc/caddy/Caddyfile.d/mcnf-ingress.caddy and reloads Caddy; this script makes
# that drop-in actually take effect — it installs Caddy, ensures the fragment dir
# exists, and wires the main Caddyfile to `import Caddyfile.d/*.caddy`.
#
# Run on the INGRESS LIGHTHOUSE role only (the public boundary). Idempotent +
# boot-durable: re-running is a no-op; the caddy.service is enabled so ingress
# survives a reboot. Caddy fetches Let's Encrypt certs automatically for the DDNS
# hostnames in the rendered sites — no cert handling here.
#
# The managed fragment is the ONLY thing MCNF owns; the operator's own Caddyfile
# sites (if any) are left untouched — we only append the `import` line once.
#
# Options:
#   --fragment-dir <dir>   the drop-in dir (default /etc/caddy/Caddyfile.d)
#   --caddyfile <path>     the main Caddyfile (default /etc/caddy/Caddyfile)
#   --no-start             install + wire only; don't enable/start the service
set -euo pipefail

FRAGMENT_DIR="/etc/caddy/Caddyfile.d"
CADDYFILE="/etc/caddy/Caddyfile"
DO_START=1

while [ $# -gt 0 ]; do case "$1" in
  --fragment-dir) FRAGMENT_DIR="$2"; shift 2;;
  --caddyfile) CADDYFILE="$2"; shift 2;;
  --no-start) DO_START=0; shift;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

log() { echo "==> setup-caddy: $*"; }

# 1. Install Caddy if absent. Fedora ships `caddy` in the default repos.
if ! command -v caddy >/dev/null 2>&1; then
  log "installing caddy"
  dnf install -y caddy
else
  log "caddy already installed ($(caddy version 2>/dev/null | head -1))"
fi

# 2. Ensure the managed fragment dir exists (the worker writes here).
mkdir -p "$FRAGMENT_DIR"

# 3. Ensure the main Caddyfile imports the fragment dir — exactly once.
#    Caddy's default Caddyfile is a single site block; an `import` at global scope
#    must precede site blocks, so we prepend it if it isn't already present.
IMPORT_LINE="import ${FRAGMENT_DIR}/*.caddy"
if [ ! -f "$CADDYFILE" ]; then
  log "no $CADDYFILE — creating one with just the MCNF import"
  printf '# MCNF ingress (CONNECT-4): mackesd renders sites into %s/\n%s\n' \
    "$FRAGMENT_DIR" "$IMPORT_LINE" > "$CADDYFILE"
elif grep -qF "$IMPORT_LINE" "$CADDYFILE"; then
  log "import already wired in $CADDYFILE"
else
  log "prepending MCNF import to $CADDYFILE"
  tmp="$(mktemp)"
  printf '# MCNF ingress (CONNECT-4): mackesd renders sites into %s/\n%s\n\n' \
    "$FRAGMENT_DIR" "$IMPORT_LINE" > "$tmp"
  cat "$CADDYFILE" >> "$tmp"
  cat "$tmp" > "$CADDYFILE"
  rm -f "$tmp"
fi

# 4. Validate the combined config (an empty fragment dir is valid).
if caddy validate --config "$CADDYFILE" --adapter caddyfile >/dev/null 2>&1; then
  log "Caddyfile validates"
else
  log "WARN: caddy validate reported issues — leaving config in place for inspection"
fi

# 5. Enable + start so ingress is boot-durable.
if [ "$DO_START" = "1" ]; then
  systemctl enable caddy.service >/dev/null 2>&1 || true
  systemctl reload caddy.service 2>/dev/null || systemctl restart caddy.service || {
    log "WARN: could not (re)start caddy.service — check 'journalctl -u caddy'"; }
  log "caddy.service: $(systemctl is-active caddy.service 2>/dev/null || echo inactive)"
fi

log "done — mackesd will render public sites into $FRAGMENT_DIR/mcnf-ingress.caddy"
