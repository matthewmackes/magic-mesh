# Sysfs identity equivocation evidence — 2026-08-11

- Scope: hardware inventory canonicalizes independently enumerated sysfs aliases
  to one stable kernel-object identity before publication.
- Admission behavior: exact duplicate observations collapse to one row. If two
  sources claim incompatible categories or bodies for the same identity, only
  that identity is suppressed while unrelated hardware remains visible.
- Hostile regression: aliased PCI and class-net paths produce conflicting claims
  for one adapter while a distinct interface remains published; an exact alias
  pair also proves deterministic deduplication.
- Production path: `HardwareProbeWorker → publish_system → enumerate →
  suppress_conflicting_sysfs_identities`.
- Focused gate: `cargo test -p mackesd workers::device_inventory::tests::conflicting_sysfs_sources_suppress_only_the_equivocated_hardware_identity -- --exact --nocapture`.
- Farm: BigBoy, with 12.1 GiB free at admission.
- Result: **PASS**, 1 passed, 0 failed, 4,839 filtered out; scoped
  `git diff --check` passed.
