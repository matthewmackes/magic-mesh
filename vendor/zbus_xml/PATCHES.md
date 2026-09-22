# Vendored zbus_xml 4.0.0 patch

This directory contains `zbus_xml` 4.0.0 from crates.io under its upstream
MIT license. The crate is vendored because it is the last 4.x release and is
still required by `atspi-common` 0.6.0 (`egui` 0.31 → `accesskit_unix`
0.13.1 → `zbus-lockstep` 0.4.4). That line pulls `quick-xml` 0.30.0, which
cargo-deny flags (yank / RUSTSEC-2026-0194). Upstream 5.2 dropped
`quick-xml` but requires `zvariant` 5, which this AccessKit generation
cannot take.

The local patch:

- depends on `quick-xml` 0.41 (already in the workspace lock via
  `wayland-scanner`) instead of 0.30;
- maps `quick-xml` 0.41's `SeError` / `WriteResult` serialize result back
  onto the existing `Error::QuickXml` arm so `zbus-lockstep` 0.4.4 keeps
  compiling.

Re-evaluate and remove this vendored copy when the platform upgrades the
egui / AccessKit family onto an `atspi` stack that uses `zbus_xml` 5.x.
