#!/bin/bash
# NETDATA-1 — fetch + verify the pinned netdata static build (the live-metrics
# substrate the platform's PD-2 health tiers + PD-7 flow map consume from
# `<peer-overlay-ip>:19999`). netdata is NOT in the Fedora repos, so it can't be
# an RPM `Requires:`; this installs the upstream static self-installer,
# sha256-verified. At ~181 MB it's too large to bundle, so it is FETCH-ONLY (a
# network birthright; runs at first boot once the overlay is up). Idempotent +
# one-way. The `netdata_aggregator` mackesd worker then confines the dashboard's
# [web] bind to loopback + this node's overlay IP on its next tick.
set -u
VER="v2.10.3"
SHA256="81bb70eba9bbfcd42b26dca432aa6b7953d2ba6c4e18fd449d29442f9feaddad"
ASSET="netdata-x86_64-${VER}.gz.run"
URL="https://github.com/netdata/netdata/releases/download/${VER}/${ASSET}"
OPT_BIN=/opt/netdata/bin/netdata
log(){ echo "mesh-install-netdata: $*"; }

# Idempotent: already installed → just (re)assert the symlinks + service, exit.
if [ -x "$OPT_BIN" ]; then
  log "netdata already present at $OPT_BIN"
else
  # NETDATA-1 SAFETY GATE (2026-06-17): the 181 MB static build extracts to
  # hundreds of MB + runs an installer; on a low-RAM node (≤~2.5 GB droplet/VM)
  # this OOM-thrashes the box — it once wedged a lighthouse's LizardFS master and
  # cascaded a mesh-wide QNM-Shared outage. Skip the install on low-RAM hosts; the
  # live-metrics map degrades gracefully (that peer just has no :19999). Override
  # with MDE_NETDATA_FORCE=1. Workstations (the surfaces that read the map) have
  # the headroom; tiny headless droplets don't need to self-monitor via netdata.
  MIN_MB="${MDE_NETDATA_MIN_MB:-3072}"
  totmb=$(awk '/MemTotal/{print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 0)
  if [ "${MDE_NETDATA_FORCE:-0}" != "1" ] && [ "$totmb" -lt "$MIN_MB" ]; then
    log "skipping: host has ${totmb}MB RAM (< ${MIN_MB}MB) — netdata static install would thrash; set MDE_NETDATA_FORCE=1 to override"
    exit 0
  fi
  command -v curl >/dev/null || { log "curl missing — skipping (retry next boot)"; exit 0; }
  command -v sha256sum >/dev/null || { log "sha256sum missing — skipping"; exit 0; }
  TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
  log "fetching netdata ${VER} static (~181 MB)"
  curl -fsSL "$URL" -o "$TMP/$ASSET" || { log "download failed (retry next boot)"; exit 0; }
  echo "${SHA256}  $TMP/$ASSET" | sha256sum -c - >/dev/null 2>&1 \
    || { log "SHA256 MISMATCH — refusing to install"; exit 1; }
  log "installing (telemetry disabled, not auto-started)"
  # makeself: --accept the license; args after -- go to netdata's installer.
  sh "$TMP/$ASSET" --accept --quiet -- --dont-wait --disable-telemetry --dont-start-it \
    || { log "installer returned non-zero (continuing to wire what landed)"; }
  [ -x "$OPT_BIN" ] || { log "netdata binary not present after install — aborting"; exit 1; }
fi

# Wire the static /opt/netdata layout into the platform's expected paths:
#  - /etc/netdata -> the static conf dir, so the netdata_aggregator worker
#    (DEFAULT_NETDATA_CONF=/etc/netdata/netdata.conf) edits the conf netdata reads.
#  - /usr/sbin/netdata -> the static binary, for any CLI/health probe on PATH.
ln -sfn /opt/netdata/etc/netdata /etc/netdata 2>/dev/null || true
ln -sfn "$OPT_BIN" /usr/sbin/netdata 2>/dev/null || true

# Pre-confine the dashboard to loopback BEFORE first start so :19999 never binds
# 0.0.0.0 on a public lighthouse, even for the window before the aggregator's
# first tick (which then adds this node's overlay IP for peer fetches).
conf=/opt/netdata/etc/netdata/netdata.conf
if [ -f "$conf" ] && ! grep -qE '^\[web\]' "$conf"; then
  printf '\n[web]\n    bind to = 127.0.0.1\n' >> "$conf"
fi

systemctl daemon-reload >/dev/null 2>&1 || true
systemctl enable --now netdata.service >/dev/null 2>&1 \
  && log "netdata enabled + started (loopback-confined; aggregator adds the overlay bind)" \
  || log "could not enable netdata.service (will retry next boot)"
