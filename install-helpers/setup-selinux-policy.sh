#!/bin/bash
# setup-selinux-policy.sh - QC-22 SELinux tightening bootstrap.
#
# Quasar-cloud restored the Red Hat-conventions target: SELinux should be
# Enforcing on shipped nodes, with MCNF/OpenStack policy loaded explicitly. This
# helper is intentionally safe in RPM flows: it is run by a bounded systemd
# oneshot, never synchronously from dnf %post.
#
# It does three things:
#   1. Persist SELINUX=enforcing for the next boot.
#   2. Load the shipped MCNF CIL policy modules with bounded semodule calls.
#   3. If the current boot is Permissive, try to switch to Enforcing after the
#      policy is loaded. A Disabled kernel cannot be changed until reboot.
#
# Optional modules are best-effort because their referenced policy types only
# exist when the matching subsystem is installed (for example container-selinux
# for Podman, libvirt SELinux policy for virtqemud).
set -uo pipefail

CONFIG=${MCNF_SELINUX_CONFIG:-/etc/selinux/config}
POLICY_DIR=${MCNF_SELINUX_POLICY_DIR:-/usr/share/magic-mesh/selinux}
SEMODULE_TIMEOUT=${MCNF_SEMODULE_TIMEOUT:-90}
ENFORCE_NOW=${MCNF_SELINUX_ENFORCE_NOW:-1}

cur="$(getenforce 2>/dev/null || echo Disabled)"
rc=0

persist_enforcing() {
  if [ ! -f "$CONFIG" ]; then
    echo "WARN: $CONFIG absent; cannot persist SELINUX=enforcing"
    return 0
  fi

  if grep -q '^SELINUX=' "$CONFIG"; then
    sed -i 's/^SELINUX=.*/SELINUX=enforcing/' "$CONFIG"
  else
    printf '\nSELINUX=enforcing\n' >>"$CONFIG"
  fi
  echo "==> persisted SELINUX=enforcing in $CONFIG"
}

load_cil() {
  local name=$1
  local file=$2
  local required=$3

  if [ ! -f "$file" ]; then
    echo "WARN: SELinux module $name missing at $file"
    [ "$required" = "required" ] && rc=1
    return 0
  fi

  if timeout "$SEMODULE_TIMEOUT" semodule -i "$file" >/dev/null 2>&1; then
    echo "==> loaded SELinux module $name"
    return 0
  fi

  if [ "$required" = "required" ] && [ "$cur" != "Disabled" ]; then
    echo "ERROR: failed to load required SELinux module $name from $file" >&2
    rc=1
  else
    echo "WARN: skipped SELinux module $name; optional type may be absent or SELinux disabled"
  fi
}

persist_enforcing

if ! command -v semodule >/dev/null 2>&1; then
  echo "WARN: semodule not installed; policy load deferred until selinux-policy tools exist"
  exit 0
fi

load_cil magicmesh-base "$POLICY_DIR/magicmesh-base.cil" required
load_cil magicmesh-podman "$POLICY_DIR/magicmesh-podman.cil" optional
load_cil magicmesh-libvirt "$POLICY_DIR/magicmesh-libvirt.cil" optional

if [ "$cur" = "Permissive" ] && [ "$ENFORCE_NOW" = "1" ] && command -v setenforce >/dev/null 2>&1; then
  if setenforce 1 >/dev/null 2>&1; then
    cur="Enforcing"
    echo "==> setenforce 1 after loading MCNF policy"
  else
    echo "ERROR: setenforce 1 failed after loading MCNF policy" >&2
    rc=1
  fi
elif [ "$cur" = "Disabled" ]; then
  echo "==> SELinux kernel state is Disabled; reboot required for Enforcing"
fi

echo "==> SELinux mode now: $(getenforce 2>/dev/null || echo Disabled); target = Enforcing"
exit "$rc"
