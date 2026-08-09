# WL-ARCH-008 Display1 migration rollback — 2026-08-09

## Outcome

The one-time legacy Browser VM cutover helper no longer accepts an arbitrary
libvirt domain. Its only mutation target is the fixed `browser-vm` domain.

After defining the Display1 candidate, the helper now re-reads the inactive
definition and verifies exact disk-source fingerprints, one peer-to-peer D-Bus
graphics device, and virtio 3D video. Any failed post-define verification
immediately redefines the timestamped original XML. If restoration itself
fails, the error names the root-only manual-recovery XML instead of claiming a
safe state.

## Verification

- Farm `.90`, slot `arch008-display1-migration-rollback-r3-20260809`.
- Shell syntax and migration self-test: passed.
- Hostile `--domain foreign-vm` invocation was rejected before SSH/libvirt.
- Full Browser VM activation contract: passed.
- No live legacy domain was available for forced verification-failure proof;
  this checkpoint makes no live migration claim.
