#!/bin/bash
# verify-boot-recovery.sh — BOOT-REC-4: the reboot-recovery RELEASE GATE.
#
# Operator hard requirement (2026-06-16): every node MUST fully recover from a
# power outage / reboot / shutdown with ZERO manual steps. This script asserts a
# node is a healthy mesh member; run it AFTER a clean reboot (give the node a
# minute to settle) — a non-zero exit means recovery is incomplete and a release
# is gated.
#
# Run locally on the node:   verify-boot-recovery.sh
# Or remotely:               ssh <node> 'bash -s' < verify-boot-recovery.sh
#
# Checks (each is a recovery invariant):
#   1. mackesd is active.
#   2. /mnt/mesh-storage is a real FUSE mount (the shared-state plane).
#   3. a leader lock exists on the mount (leadership held / visible).
#   4. the bus answers action/shell/healthz (no readonly-DB latch — BOOT-REC-3).
#   5. on a Workstation (desktop user present): ~/Documents is a bind mountpoint
#      (FPG-7 communal sync — AUDIT-MESH-15).
set -u
QNM="${MDE_WORKGROUP_ROOT:-/mnt/mesh-storage}"
fail=0
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=1; }

echo "== BOOT-REC-4 recovery gate =="

systemctl is-active --quiet mackesd && ok "mackesd active" || bad "mackesd not active"

if mount 2>/dev/null | grep -q " $QNM type fuse"; then ok "$QNM mounted (fuse)"; else bad "$QNM not a fuse mount"; fi

if [ -f "$QNM/.mackesd-leader.lock" ]; then ok "leader lock present"; else bad "no leader lock on $QNM"; fi

hz="$(MDE_BUS_ROOT=/run/mde-bus timeout 8 mde-bus request action/shell/healthz --timeout-secs 6 2>&1)"
if printf '%s' "$hz" | grep -qiE '"?(ok|ready|healthy)"?|node_count'; then
    ok "bus healthz answers"
else
    bad "bus healthz no reply (readonly-DB latch? BOOT-REC-3) — $(printf '%s' "$hz" | head -c 80)"
fi

# Workstation-only: a desktop user (uid 1000-60000 under /home) → expect the binds.
duser_home="$(awk -F: '$3>=1000 && $3<60000 && $6 ~ /^\/home/ {print $6; exit}' /etc/passwd)"
if [ -n "$duser_home" ]; then
    if mountpoint -q "$duser_home/Documents"; then
        ok "~/Documents bind-mounted (FPG-7 sync)"
    else
        bad "$duser_home/Documents not bind-mounted (AUDIT-MESH-15)"
    fi
else
    ok "no desktop user — XDG bind not expected (headless role)"
fi

echo
if [ "$fail" = 0 ]; then
    echo "BOOT-REC-4: PASS — node fully recovered."
else
    echo "BOOT-REC-4: FAIL — recovery incomplete; release gated."
fi
exit "$fail"
