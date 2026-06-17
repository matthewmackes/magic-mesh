#!/bin/bash
# setup-selinux-policy.sh — install the Magic Mesh local SELinux policy modules so
# a node runs clean under Enforcing SELinux (no AVC flood, no repeating
# "SELinux security alert" toasts). Idempotent: safe to re-run; `semodule -i`
# replaces an already-loaded module.
#
# Self-skips when SELinux is disabled or the tooling is absent. NEVER weakens the
# node — it does not set permissive or disable SELinux; it grants exactly the
# legitimate accesses the platform daemon stack needs (rationale per rule in each
# .cil). The modules are split by precondition so a missing optional type (podman
# not installed, libvirt not installed) only skips that one module:
#   magicmesh-base     base-policy types — always loads
#   magicmesh-podman   container_runtime_t — needs container-selinux (podman)
#   magicmesh-libvirt  virtqemud_t — needs the libvirt SELinux policy
#
# Called from the RPM %post (all roles) and can be run by hand:
#   /usr/libexec/mackesd/setup-selinux-policy
set -uo pipefail

# Locate the shipped CIL dir: next to this script in the tree, or the RPM asset.
DIR=""
for d in "$(dirname "$0")/selinux" /usr/share/magic-mesh/selinux; do
  [ -d "$d" ] && DIR="$d" && break
done
[ -n "$DIR" ] || { echo "magicmesh CIL dir not found; skipping"; exit 0; }

command -v getenforce >/dev/null 2>&1 || { echo "SELinux tooling absent; skipping"; exit 0; }
[ "$(getenforce 2>/dev/null || echo Disabled)" = "Disabled" ] && { echo "SELinux disabled; skipping"; exit 0; }
command -v semodule >/dev/null 2>&1 || { echo "semodule absent; skipping"; exit 0; }

rc=0
for m in magicmesh-base magicmesh-podman magicmesh-libvirt; do
  cil="$DIR/$m.cil"
  [ -f "$cil" ] || continue
  if semodule -i "$cil" 2>/dev/null; then
    echo "==> loaded $m"
  else
    # Best-effort: a missing optional type (no podman / no libvirt) is expected
    # and harmless — that node never trips the denial. Warn, do not fail.
    echo "WARN: skipped $m (optional type absent or load failed)"
  fi
done
echo "==> magicmesh SELinux policy applied; SELinux mode: $(getenforce)"
exit $rc
