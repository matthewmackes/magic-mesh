# Salvaged from the deleted `mde` desktop binary (E11.12 cutover)

These two source files lived inside `crates/shell/mde` — the labwc/Win-era
desktop shell binary that the E11 "Magic Mesh" pivot deletes. They are
**mesh-relevant surfaces** (not desktop chrome), so they were salvaged here
rather than lost, pending re-homing onto Cosmic.

- **`birthright.rs`** (1,464 lines) — the `mde birthright` first-boot
  health-attestation dashboard (desktop / mesh / SIP / network sections,
  auto-remediation, fleet-dashboard push). It is **iced 0.13 + `mde-ui`-bound**,
  so it cannot drop straight into this iced-0.14 workspace. Re-home it into
  `mde-workbench` (the Cosmic control surface) as a live-health view — tracked as
  **E11.9** ("birthright = ongoing live health in Workbench"). The port is
  0.13→0.14 widget translation + theming via `mde-theme` (see
  `workbench-iced014-vs-mdeui-iced013`).

- **`mesh_status.rs`** — the mesh-status readout (mostly logic, not iced-bound).
  Re-home into the Workbench mesh view / a Cosmic applet (E11.3/E11.9).

Neither is a workspace member yet (they would not compile against iced 0.14
as-is); they are preserved verbatim so the re-home work has the original source.
