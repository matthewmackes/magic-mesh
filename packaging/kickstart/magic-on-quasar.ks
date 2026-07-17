# OW-12 — the Magic-on-Quazar Workstation ISO kickstart. A bootc/ostree-native
# installer for the ONE immutable MCNF image (E12-13, §5 delivery lock): the
# egui-DRM shell Workstation + mackesd + Podman + Nebula + the QC-1
# libvirt/QEMU-KVM/OVN host bits, with the desktop seat gated on the role flag
# so the SAME image serves a headless Server/Lighthouse (role is a config flag,
# not a build).
#
# Supersedes the heritage magic-on-cosmic.ks, which installed the RETIRED
# @^fedora-cosmic-desktop-environment via the package lane. This kickstart does
# NOT %packages-install anything: it deploys the pre-built **bootc Workstation
# image** with `ostreecontainer`, so the magic-mesh RPM, the Quazar DRM-seat unit
# (packaging/bootc/units/mde-shell-egui.service) + its preset
# (packaging/bootc/system-preset/45-mcnf-quasar.preset), the host virt bits and
# graphical.target default ALL arrive baked in from the image
# (packaging/bootc/Containerfile). The kickstart REFERENCES that image — it never
# re-packages the RPM or re-authors the seat unit (both already exist).
#
# Build the installer ISO (all steps operator-gated at /release; run on an
# execution-tagged build node):
#   1. Build the bootc WS image from a farm-built RPM — this produces the
#      localhost/magic-mesh-bootc:latest tag deployed below (an F43 rebase is the
#      same build with --base <fedora-bootc:43>):
#        packaging/bootc/build-image.sh --rpm <farm-built.rpm>
#   2. Cut an anaconda INSTALLER ISO that embeds that image + carries this
#      kickstart's role onboarding (bootc-image-builder honors a supplied ks):
#        sudo packaging/bootc/build-image.sh --rpm <rpm> --disk anaconda-iso
#      …or boot a stock Fedora installer with `inst.ks=cdrom:/magic-on-quasar.ks`
#      against a host whose container storage carries the image.
# NB: the heritage livemedia-creator `--make-iso` catalog lane (mackesd images
# --build --kind iso, crates/mesh/mackesd/src/image_build.rs) is the PACKAGE lane
# and drives magic-on-cosmic.ks; a bootc `ostreecontainer` install goes through
# the anaconda-iso lane above.
#
# HONESTY (§7): RPM GPG signing, the bootc image registry publish, and the .iso
# cut are operator-gated (/release). A LIVE display+headless boot of the produced
# ISO is the only acceptance a boot target can give — it is NOT verifiable on the
# farm and no boot is faked here.

text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
# Mesh-administered fleet, platform standard: no interactive root login — the box
# onboards over the mesh and magic-setup.service owns first-run. Mirrors heritage.
rootpw --lock
# SELinux PERMISSIVE at install — the QC-22 posture on-ramp (build-deploy-8
# reconciliation, 2026-07-11). The prior `--disabled` here cited a 2026-06-20
# "disabled" fleet standard as docs/THREAT_MODEL.md §5 — but §5 is "Out of scope /
# non-goals" (nothing about SELinux) and that standard is SUPERSEDED for
# Quazar-cloud nodes: THREAT_MODEL §3.2 (platform note) + §4.5 both state "shipped
# nodes target SELinux Enforcing and load the MCNF policy modules through the
# bounded boot-time policy oneshot." This kickstart installs QC (Quazar-cloud)
# nodes, so it must NOT leave SELinux off — doing so silently defeated the whole
# Enforcing stack the RPM ships:
#   - The RPM SHIPS + ENABLES that stack: the SELINUX-1/QC-22 CIL policy modules +
#     magic-mesh-selinux-policy.service (crates/mesh/mackesd/Cargo.toml SELINUX-1
#     assets ~L675-682; post_install_script enables the oneshot ~L823) whose loader
#     install-helpers/setup-selinux-policy.sh persists SELINUX=enforcing, loads
#     magicmesh-base.cil, and — when the current boot is PERMISSIVE — runs
#     `setenforce 1` (its L77). Plus two confined ENFORCING browser domains
#     (mde_web_preview_t / mde_web_cef_t) that self-skip ONLY where SELinux is off.
#   - `--disabled` installed the filesystem UNLABELED and defeated all of the
#     above: the oneshot would still rewrite the config to enforcing, so the NEXT
#     reboot would try to come up Enforcing on an unlabeled FS — a brick risk on an
#     ISO seat, not a security win.
# PERMISSIVE is the safe on-ramp the loader is DESIGNED to consume: the FS is
# labeled at install, the oneshot promotes Permissive -> Enforcing after the base
# policy loads, and any policy gap logs AVC denials instead of blocking boot on an
# ISO-installed seat. (Heritage magic-on-cosmic.ks uses --enforcing directly.)
# OPERATOR POSTURE DECISION (flag): this flips ISO-installed seats from SELinux-OFF
# to on-track-to-Enforcing. If a hard-enforcing FIRST boot is wanted AND the policy
# is boot-validated complete, change --permissive to --enforcing; if SELinux must
# stay OFF fleet-wide, revert to --disabled AND update THREAT_MODEL §3.2/§4.5 + the
# RPM SELINUX-1 assets to match (today those two lanes contradict a disabled node).
selinux --permissive
network --bootproto=dhcp --activate
bootloader --location=mbr
clearpart --all --initlabel
autopart --type=plain --nohome

# ── Deploy the ONE immutable bootc Workstation image (§5) ─────────────────────
# NOT a %packages install: `ostreecontainer` lays the pre-built bootc image down
# whole, bringing the magic-mesh RPM + the Quazar DRM-seat unit + its preset +
# libvirt/QEMU-KVM/OVN + the graphical.target default already materialized by the
# Containerfile's `systemctl enable` / `set-default`.
#
# --url is build-image.sh's DEFAULT --tag; --transport=containers-storage reads
# it from the installer's EMBEDDED container storage (the airgapped path — the
# anaconda-iso / bootc-image-builder lane embeds it, so no registry egress at
# install time). At /release the operator swaps this line for the published
# SIGNED registry ref and drops --no-signature-verification, e.g.:
#   ostreecontainer --url=<registry>/magic-mesh-bootc:<ver> --transport=registry
ostreecontainer --url=localhost/magic-mesh-bootc:latest --transport=containers-storage --no-signature-verification

# ── Role onboarding (mirrors the heritage %post: role-pin + firstboot join) ───
# The deployed image is UNPINNED on first boot — mackesd fails closed (ENT-2) and
# the seat unit's ExecCondition skips until a role lands in /var/lib/mde/role.toml.
# Pin it here from the boot-menu profile so every profile installs unattended
# (quasar-bootmenu.cfg appends mde.profile=<name> [+ mde.headless]).
%post --interpreter=/usr/bin/bash --erroronfail
set -euo pipefail

install -d /etc/magic-mesh

# W57 — read the boot-menu profile (+ an explicit headless flag) off the kernel
# cmdline. One ISO carries every profile; the chosen menu entry appended
# mde.profile=<name> (and, for a display-less box, mde.headless).
PROFILE=""
HEADLESS=0
for tok in $(cat /proc/cmdline 2>/dev/null || true); do
  case "$tok" in
    mde.profile=*)          PROFILE="${tok#mde.profile=}" ;;
    mde.headless|mde.headless=1) HEADLESS=1 ;;
  esac
done

# An explicit MDE_INSTALL_ROLE / MDE_INSTALL_HEADLESS still wins (unattended spins).
ROLE="${MDE_INSTALL_ROLE:-}"
[ "${MDE_INSTALL_HEADLESS:-0}" = 1 ] && HEADLESS=1

# The shipped core pack is one profile per role, so name == role; a custom profile
# pins via its baked TOML's role= line (heritage idiom).
if [ -z "$ROLE" ] && [ -n "$PROFILE" ]; then
  case "$PROFILE" in
    workstation|server|lighthouse) ROLE="$PROFILE" ;;
    *)
      ptoml="/etc/magic-mesh/profiles/${PROFILE}.toml"
      [ -f "$ptoml" ] && ROLE="$(sed -n 's/^role[[:space:]]*=[[:space:]]*"\?\([a-z]*\)"\?.*/\1/p' "$ptoml" | head -1)"
      ;;
  esac
fi

# A Server/Lighthouse is inherently headless (no seat); default an otherwise
# unspecified headless install to the Server role (mesh daemons, no display).
case "$ROLE" in server|lighthouse) HEADLESS=1 ;; esac
if [ "$HEADLESS" = 1 ] && [ -z "$ROLE" ]; then ROLE="server"; fi
# No profile and not headless ⇒ Workstation (the default menu entry / egui seat).
[ -z "$ROLE" ] && ROLE="workstation"

# Pin the role flag the seat unit + mackesd read (ENROLLMENT.md; heritage idiom).
# A fresh node fails closed until this exists, so pin it even if mackesd cannot
# run in the constrained installer chroot (magic-setup.service re-prompts then).
/usr/bin/mackesd role-pin "$ROLE" || true

if [ "$HEADLESS" = 1 ]; then
  # HEADLESS path — mesh role daemons only, no local display or seat. The image's
  # seat unit already self-skips off a non-workstation role (its ExecCondition),
  # but belt-and-braces hard-off per bootc/README.md "Roles" Option 1 so a
  # display-less box (incl. a headless *Workstation* role, mde.headless) never
  # targets graphical or lights a user surface. Re-roling later is the documented
  # unmask + set-default graphical.target — no reinstall (§5).
  systemctl mask mde-shell-egui.service || true
  systemctl --global mask mde-musicd.service || true
  systemctl set-default multi-user.target || true
else
  # DISPLAY Workstation path — the egui DRM seat. The deployed image already
  # enabled mde-shell-egui.service + podman.socket (45-mcnf-quasar.preset) and set
  # graphical.target; re-assert for symmetry with the headless branch and to stay
  # correct after a factory `systemctl preset-all`.
  systemctl unmask mde-shell-egui.service || true
  systemctl enable mde-shell-egui.service podman.socket || true
  systemctl set-default graphical.target || true
fi

# First-boot hint (Quazar wording — the egui shell, not the retired Cosmic chooser).
SEATNOTE=""
[ "$HEADLESS" = 1 ] && SEATNOTE=" (headless — no seat)"
cat > /etc/magic-mesh/first-boot.txt <<HINT
MCNF Quazar is installed as role: ${ROLE}${SEATNOTE}.

A fresh node is UNPINNED until a role lands in /var/lib/mde/role.toml; this
install pinned "${ROLE}" so mackesd starts and, on a Workstation seat, the egui
shell (mde-shell-egui.service) takes the DRM/KMS seat directly — no display
manager, no compositor.

  Workstation (seat):  boots straight into the egui shell.
  Headless / Server / Lighthouse: mesh daemons only. Re-role to a seat with
        systemctl unmask mde-shell-egui.service
        systemctl set-default graphical.target
        mackesd role-pin workstation        # no reinstall, §5

Found the founding lighthouse:  mackesd mesh-init --mesh-id <id> --external-addr <ip>:4242
Join an existing mesh:          mackesd join '<join token>'
HINT

# W60 — firstboot single-use auto-join. An auto-join profile bakes its bearer at
# /etc/magic-mesh/join-token; install a oneshot that enrolls once, then erases the
# token + self-disables so the bearer never lingers or replays (heritage idiom;
# onboards the mesh on BOTH the display and the headless path).
if [ -f /etc/magic-mesh/join-token ]; then
  chmod 600 /etc/magic-mesh/join-token
  cat > /etc/systemd/system/mde-firstboot-join.service <<'UNIT'
[Unit]
Description=MCNF firstboot auto-join (single-use bearer)
After=network-online.target mackesd.service
Wants=network-online.target
ConditionPathExists=/etc/magic-mesh/join-token

[Service]
Type=oneshot
ExecStart=/bin/sh -c '/usr/bin/mackesd join "$(cat /etc/magic-mesh/join-token)"'
# Erase the single-use bearer + disable the unit whether or not join succeeded —
# a stale token must never sit on disk or replay.
ExecStartPost=/bin/sh -c 'rm -f /etc/magic-mesh/join-token; systemctl disable mde-firstboot-join.service'
RemainAfterExit=no

[Install]
WantedBy=multi-user.target
UNIT
  systemctl enable mde-firstboot-join.service || true
fi
%end
