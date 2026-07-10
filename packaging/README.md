# packaging/ — non-crate distribution assets (PKG-2)

Top-level home for everything the RPM/ISO installs that isn't a crate:
desktop entries, autostart files, (future) systemd units, kickstarts,
and .repo files. The PKG epic's cargo-generate-rpm metadata maps these
into the filesystem; the hicolor icon set lives in `../assets/icons/`.

- `applications/` → `/usr/share/applications/` (app launchers; Icon=magic-mesh
  resolves via the hicolor set)
- `autostart/`    → `/etc/xdg/autostart/` (the SVC-4 voice agent autostart —
  Cosmic honors XDG autostart)
- `systemd/`      → `/usr/lib/systemd/system/` (ENT-6: `mackesd.service`,
  Restart=on-failure — kill -9 recovers in seconds; in-daemon worker
  restarts are the supervisor's bounded-backoff + circuit-breaker job).
  `mde-musicd.service` is a user unit (`default.target`) whose
  `ExecCondition=mackesd role-gate --min-rank 1` skips it cleanly on
  Servers/Lighthouses (SVC-7/Q70 — desktop services are Workstation
  surfaces; the voice-agent autostart carries the same gate inline)

- `bootc/`        → the E12-13 **immutable bootc/ostree image lane** (§5: ONE
  image for every role — role is a config flag; a Lighthouse runs the same
  image with the desktop seat skipped/masked). Containerfile + the DRM-seat
  unit + preset + `build-image.sh` (typed-gated) + `verify-image.sh` (static
  image acceptance); doctrine + verification status in `bootc/README.md`.
- `kickstart/`    → the install ISO kickstarts + boot-menu profile snippets.
  `magic-on-quasar.ks` (OW-12) is the current **Quasar Workstation** kickstart:
  a bootc/ostree-native installer that `ostreecontainer`-deploys the immutable
  bootc WS image (`bootc/`, so the magic-mesh RPM + the egui DRM-seat unit +
  QC-1 libvirt/QEMU-KVM/OVN host bits arrive baked in — it references that
  image, never re-packages it), with a `%post` that pins the role from the boot menu and supports a
  headless (no-seat, mesh-daemons-only) variant via `mde.headless`; its menu is
  `quasar-bootmenu.cfg`, cut through the bootc anaconda-iso lane. The heritage
  `magic-on-cosmic.ks` (PKG-9) + `profile-bootmenu.cfg` remain the retired
  Cosmic-era package lane (`@^fedora-cosmic-desktop-environment`, livemedia-creator).
- `repo/`         → the GitHub-hosted `.repo` (PKG-8) + the committed public
  signing key `RPM-GPG-KEY-magic-mesh` (EFF-17). Both ship inside the main
  `magic-mesh` RPM (one-RPM design, §5 — no separate release sub-package):
  the `.repo` lands at `/etc/yum.repos.d/`, the key at `/etc/pki/rpm-gpg/`,
  so a one-shot `dnf install <url>` leaves a gpgcheck'd upgrade channel.
- `ENROLLMENT.md` → the post-install enroll/mesh-init steps (PKG-10).
