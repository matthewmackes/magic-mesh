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
  `ExecCondition=mackesd role-gate --min-rank 2` skips it cleanly on
  Servers/Lighthouses (SVC-7/Q70 — desktop services are Workstation
  surfaces; the voice-agent autostart carries the same gate inline)

- `kickstart/`    → the Magic-on-Cosmic ISO kickstart (PKG-9) with the
  install-time role-chooser `%post` (PKG-5); built with livemedia-creator.
- `repo/`         → the GitHub-hosted `.repo` (PKG-8), shipped by the
  `magic-mesh-release` RPM (gpgcheck on, project GPG key; GitHub Pages baseurl).
- `ENROLLMENT.md` → the post-install enroll/mesh-init steps (PKG-10).
