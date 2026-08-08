# WL-UX-013 — device-aware availability policy (2026-08-05)

The shared health contract now includes a pure, bounded device-class policy
assessment. Declared sleep/shutdown/reboot/maintenance absence remains
`ExpectedAbsence` through its grace window, then becomes warning/critical only
after the declared return is missed. Unannounced escalation requires an
independent last-seen timestamp; missing evidence remains `Unknown`.

Desktop, laptop, wireless-device, server, lighthouse, and unknown classes have
explicit bounded defaults. The helper does not mutate health, publish lifecycle
events, or infer a planned absence from a missing record.

## Verification

- Farm `.50`, slot `wl-ux013-health-policy-r1`:
  `cargo test -p mackes-mesh-types health::tests::availability_policy -- --nocapture`.
- Result: `2 passed; 0 failed; 0 ignored; 0 measured; 415 filtered out`.
- File-scoped Rust formatting passed; the disposable farm workspace was removed.
