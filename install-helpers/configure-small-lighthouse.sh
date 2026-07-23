#!/usr/bin/env bash
# configure-small-lighthouse.sh — apply the supported low-memory lighthouse
# profile to a DigitalOcean Basic 512 MiB droplet.
#
# The smallest Basic Droplet is one shared vCPU, 512 MiB RAM and 10 GiB SSD
# (currently s-1vcpu-512mb-10gb).  This profile deliberately keeps the node a
# relay/control-plane appliance: Nebula, etcd, mackesd and mesh-health remain;
# GUI, media, Netdata, broker and shell bootstrap work are disabled.  The
# limits are per-service so a burst cannot take down the whole droplet.
set -euo pipefail

PROFILE="${1:-small}"
[ "$PROFILE" = small ] || {
    echo "configure-small-lighthouse: only the 'small' profile is supported" >&2
    exit 2
}

install -d -m 0755 \
    /etc/mackesd \
    /etc/systemd/system/mackesd.service.d \
    /etc/systemd/system/etcd.service.d \
    /etc/systemd/system/nebula.service.d \
    /etc/systemd/system/caddy.service.d \
    /etc/systemd/journald.conf.d \
    /etc/sysctl.d

cat >/etc/systemd/system/mackesd.service.d/20-small-lighthouse.conf <<'UNIT'
[Service]
# Keep the relay daemon below the 512 MiB host ceiling while leaving room for
# etcd, Nebula and the Fedora base.  Swap is an emergency cushion, not a cache.
Environment=MDE_LIGHTHOUSE_PROFILE=small
MemoryAccounting=true
MemoryHigh=240M
MemoryMax=320M
MemorySwapMax=256M
CPUQuota=100%
TasksMax=512
OOMScoreAdjust=200
UNIT

cat >/etc/systemd/system/etcd.service.d/20-small-lighthouse.conf <<'UNIT'
[Service]
MemoryAccounting=true
MemoryHigh=96M
MemoryMax=128M
MemorySwapMax=128M
CPUQuota=50%
TasksMax=256
UNIT

cat >/etc/systemd/system/nebula.service.d/20-small-lighthouse.conf <<'UNIT'
[Service]
MemoryAccounting=true
MemoryHigh=48M
MemoryMax=80M
MemorySwapMax=64M
CPUQuota=50%
TasksMax=128
UNIT

cat >/etc/systemd/system/caddy.service.d/20-small-lighthouse.conf <<'UNIT'
[Service]
MemoryAccounting=true
MemoryHigh=48M
MemoryMax=80M
MemorySwapMax=64M
CPUQuota=50%
TasksMax=128
UNIT

cat >/etc/systemd/journald.conf.d/20-mcnf-small-lighthouse.conf <<'CONF'
[Journal]
SystemMaxUse=64M
RuntimeMaxUse=16M
MaxRetentionSec=7day
CONF

cat >/etc/sysctl.d/20-mcnf-small-lighthouse.conf <<'CONF'
# Avoid swapping ordinary control-plane pages; the swapfile is a last-resort
# burst cushion for package/bootstrap spikes.
vm.swappiness=10
vm.vfs_cache_pressure=50
CONF

# A 512 MiB droplet has no useful failure mode without emergency swap.  Keep
# this idempotent and never replace an operator-provided swap device/file.
if ! swapon --show=NAME --noheadings 2>/dev/null | grep -q .; then
    if [ ! -e /swapfile ]; then
        if command -v fallocate >/dev/null 2>&1; then
            fallocate -l 512M /swapfile
        else
            dd if=/dev/zero of=/swapfile bs=1M count=512 status=none
        fi
        chmod 600 /swapfile
        mkswap /swapfile >/dev/null
    fi
    swapon /swapfile 2>/dev/null || true
fi
if [ -e /swapfile ] && ! grep -qE '^[[:space:]]*/swapfile[[:space:]]' /etc/fstab 2>/dev/null; then
    printf '%s\n' '/swapfile none swap defaults 0 0' >>/etc/fstab
fi

# These oneshots are useful on a workstation but waste the entire memory
# budget on a headless relay.  Keep the units installed for role promotion;
# disabling them here is reversible and idempotent.
for unit in \
    mesh-shell-setup.service \
    mesh-broker-setup.service \
    mesh-netdata-setup.service \
    mesh-status.timer \
    magic-setup.service \
    mde-remote-proofing-plan.path; do
    systemctl disable --now "$unit" >/dev/null 2>&1 || true
done

systemctl daemon-reload
sysctl --system >/dev/null 2>&1 || true
# Reload limits immediately when bootstrap already started the services; a
# future boot applies them even if a service is currently absent.
systemctl try-restart nebula.service etcd.service caddy.service mackesd.service \
    >/dev/null 2>&1 || true
systemctl enable nebula.service mackesd.service mesh-health.timer >/dev/null 2>&1 || true

install -m 0644 /dev/null /etc/mackesd/lighthouse-profile
printf '%s\n' 'small' >/etc/mackesd/lighthouse-profile
echo "configure-small-lighthouse: profile=small applied (512 MiB / 1 vCPU baseline)"
