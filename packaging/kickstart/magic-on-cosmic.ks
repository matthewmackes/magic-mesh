# PKG-9 — the Magic-on-Cosmic ISO kickstart (built with
# livemedia-creator). A Fedora-Cosmic spin that installs the
# magic-mesh RPM from the GitHub-hosted dnf repo (PKG-8) and runs the
# install-time role chooser (PKG-5) on first boot.
#
#   livemedia-creator --ks magic-on-cosmic.ks --no-virt \
#     --resultdir /var/lmc --project "Magic Mesh" --make-iso
#
# RPM GPG signing + the actual ISO build are operator-gated (/release).

text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
rootpw --lock
firstboot --enable
selinux --enforcing
network --bootproto=dhcp --activate
bootloader --location=mbr
clearpart --all --initlabel
autopart --type=plain --nohome
services --enabled=libvirtd,mackesd

repo --name=magic-mesh --baseurl=https://matthewmackes.github.io/magic-mesh/fedora-$releasever-$basearch/

%packages
@^fedora-cosmic-desktop-environment
nebula
ansible-core
magic-mesh
%end

# PKG-5 — install-time role chooser, kickstart %post path. A clean
# install lands UNPINNED; mackesd fails closed (ENT-2) until a role is
# pinned. The chooser runs on first login (Cosmic GUI, PKG-5) OR an
# operator pins inline here for an unattended Server/Lighthouse spin.
%post --interpreter=/usr/bin/bash
set -euo pipefail
# Drop a first-boot hint so the operator sees the next step.
install -d /etc/magic-mesh
cat > /etc/magic-mesh/first-boot.txt <<'HINT'
Magic Mesh is installed but UNPINNED — mackesd will refuse to start its
worker pool until a deployment role is pinned (ENT-2 fail-closed).

  Workstation (desktop): pinned by the Cosmic first-run chooser.
  Server / Lighthouse (headless): mackesd role-pin <server|lighthouse>

Bootstrap the founding lighthouse:  mackesd mesh-init --mesh-id <id> --external-addr <ip>:4242
Join an existing mesh:              mackesd enroll --token '<join token>'
HINT
# Unattended role pin: set MDE_INSTALL_ROLE in the ISO's ks to skip the
# chooser (e.g. a Server image). Upgrade-only is enforced (PKG-7).
if [ -n "${MDE_INSTALL_ROLE:-}" ]; then
  /usr/bin/mackesd role-pin "${MDE_INSTALL_ROLE}" || true
fi
%end
