# Changelog

All notable changes to Magic Mesh are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning is the
single workspace version (`[workspace.package] version`, every crate
inherits). Release tags are **`magic-mesh-v<version>`**; the RPM NEVRA pairs
that version with the packaging `release` field (bumped alone for
asset-only changes).

Pre-release history (the E1–E11 epochs, the MackesWorkstation split, and the
v2.x–v6.x phase plans) lives in the git log and `docs/design/` — this file
starts at the first packaged release line.

## [Unreleased]

## [10.0.0] - 2026-06-13

First packaged release.

### Added
- Full **libcosmic** desktop cutover: every GUI (`mde-workbench`, `mde-files`,
  `mde-music`, `mde-voice-hud`, `mde-cosmic-applet`, `mde-role-chooser`, the
  live-map wallpaper) runs on libcosmic's vendored iced fork with the IBM
  Carbon look; the Workbench exposes an accesskit (a11y) tree.
- Direct-vs-relay tunnel path classification + chosen underlay endpoint in the
  Peers map trace card, via Nebula's loopback debug-SSH hostmap.
- One-RPM packaging (`cargo generate-rpm`): every workspace binary, systemd
  units (incl. the disabled voice pair), `.desktop` launchers/autostarts,
  icons, the swappable brand pack, DISCLAIMER/LICENSE/NOTICE/SUPPORT, help
  docs, the dnf `.repo` + the project's public signing key.
- First-run deployment-role chooser GUI (`mde-role-chooser`) and the
  cosmic-panel mesh-health applet (`mde-cosmic-applet`).
- Real cross-mesh file transfer (Send-To over the LizardFS-replicated
  volume), confined to the operator share root.
- KDC outbound drainer (ring / send-file / clipboard / share reach devices).
- Live `healthz` (node-health buckets + audit-chain status), the Prometheus
  textfile exporter worker (node health, CA-cert days-remaining, the router
  decision-time histogram), and the configurable `[[alert_hooks]]` layer
  (event JSON on stdin, post-commit dispatch).
- Transport scorer in the routing path with a per-class encryption floor
  (AES-256-class for content; operator-tunable in policy.toml) and
  hash-chained PathSwitch audit events.
- Runtime disclaimer accept gate; governance lint gates (§2 bus names,
  §4 Carbon single-source, §6 mesh boundary) wired into CI; nightly
  `--include-ignored` CI job.

### Security
- FileXfer send-to source allowlist (no exfil outside the share root;
  symlink escapes refused).
- 64 KiB body cap on every Bus responder before parse.
- Worker shell-outs bounded by kill-on-timeout (15 s) helpers.
- Netdata dashboard confined to loopback + overlay bind.
- Own KDC RSA keys pinned 4096-bit (stock-client 2048 accepted for
  verify-interop only).
- Secrets never on argv/inherited env (`--*-stdin` / systemd-creds; env
  scrubbed at boot); enrollment passcode piped via stdin.
- Nebula debug-SSH for path introspection bound loopback-only, key-auth
  (Ed25519); GPG-signed RPM + `SHA256SUMS`/`.asc`; full GPL-3.0 text shipped
  + a `SECURITY.md` disclosure policy.

[Unreleased]: https://github.com/matthewmackes/magic-mesh/compare/magic-mesh-v10.0.0...HEAD
[10.0.0]: https://github.com/matthewmackes/magic-mesh/releases/tag/magic-mesh-v10.0.0
