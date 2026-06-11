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
# PLANES-21 / W57 — boot-menu profile choice. One image carries every
# profile; the boot menu (see profile-bootmenu.cfg) appends
# `mde.profile=<name>` to the kernel cmdline for the chosen entry. Read it
# here and pin the matching role so a headless Server/Lighthouse spin
# installs unattended; absent → UNPINNED (the Cosmic first-run chooser
# handles Workstation). An explicit MDE_INSTALL_ROLE still wins.
PROFILE=""
for tok in $(cat /proc/cmdline 2>/dev/null || true); do
  case "$tok" in
    mde.profile=*) PROFILE="${tok#mde.profile=}" ;;
  esac
done
ROLE="${MDE_INSTALL_ROLE:-}"
# The shipped core pack is one profile per role, so name == role; a custom
# profile pins via its baked TOML's role= line.
if [ -z "$ROLE" ] && [ -n "$PROFILE" ]; then
  case "$PROFILE" in
    lighthouse|server|workstation) ROLE="$PROFILE" ;;
    *)
      ptoml="/etc/magic-mesh/profiles/${PROFILE}.toml"
      [ -f "$ptoml" ] && ROLE="$(sed -n 's/^role[[:space:]]*=[[:space:]]*"\?\([a-z]*\)"\?.*/\1/p' "$ptoml" | head -1)"
      ;;
  esac
fi
if [ -n "$ROLE" ]; then
  /usr/bin/mackesd role-pin "$ROLE" || true
fi

# PLANES-21 / W60 — firstboot auto-join via a single-use bearer. An
# auto-join profile bakes its join token at /etc/magic-mesh/join-token;
# install a firstboot unit that enrolls once then erases the token +
# self-disables (so the bearer never lingers and can't replay).
if [ -f /etc/magic-mesh/join-token ]; then
  chmod 600 /etc/magic-mesh/join-token
  cat > /etc/systemd/system/mde-firstboot-join.service <<'UNIT'
[Unit]
Description=Magic Mesh firstboot auto-join (single-use bearer)
After=network-online.target mackesd.service
Wants=network-online.target
ConditionPathExists=/etc/magic-mesh/join-token

[Service]
Type=oneshot
ExecStart=/bin/sh -c '/usr/bin/mackesd enroll --token "$(cat /etc/magic-mesh/join-token)"'
# Erase the single-use bearer + disable the unit whether or not enroll
# succeeded — a stale token must never sit on disk or replay.
ExecStartPost=/bin/sh -c 'rm -f /etc/magic-mesh/join-token; systemctl disable mde-firstboot-join.service'
RemainAfterExit=no

[Install]
WantedBy=multi-user.target
UNIT
  systemctl enable mde-firstboot-join.service || true
fi
%end
