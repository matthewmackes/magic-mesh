#!/bin/bash
# setup-selinux-policy.sh — OPERATOR PLATFORM STANDARD (2026-06-20):
# **SELinux is DISABLED platform-wide.** The operator set "SELinux Disabled" as
# the new platform standard, superseding SELINUX-1 (the run-clean-under-Enforcing
# CIL policy — now retired; the magicmesh-*.cil modules are no longer loaded).
#
# This writes SELINUX=disabled to /etc/selinux/config (fully effective after the
# next reboot, when the kernel skips SELinux init entirely) and drops the running
# system to permissive immediately so enforcement stops without waiting for a
# reboot. Idempotent — safe to re-run. Called from the RPM %post (all roles) and
# runnable by hand:  /usr/libexec/mackesd/setup-selinux-policy
set -uo pipefail

CONFIG=/etc/selinux/config
cur="$(getenforce 2>/dev/null || echo Disabled)"

# Already at the standard (kernel disabled + config persisted)? nothing to do.
if [ "$cur" = "Disabled" ] && grep -q '^SELINUX=disabled' "$CONFIG" 2>/dev/null; then
  echo "SELinux already disabled (platform standard)"
  exit 0
fi

# Persist disabled for the next boot.
if [ -f "$CONFIG" ]; then
  sed -i 's/^SELINUX=.*/SELINUX=disabled/' "$CONFIG"
  echo "==> set SELINUX=disabled in $CONFIG (kernel-disabled after reboot)"
else
  echo "WARN: $CONFIG absent — SELinux likely already disabled at build"
fi

# Stop enforcing NOW (permissive until the reboot makes it fully disabled).
if command -v setenforce >/dev/null 2>&1 && [ "$cur" = "Enforcing" ]; then
  setenforce 0 2>/dev/null && echo "==> setenforce 0 (permissive until reboot)"
fi

echo "==> SELinux mode now: $(getenforce 2>/dev/null || echo Disabled); platform standard = disabled (reboot to finalize)"
exit 0
