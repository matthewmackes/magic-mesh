# WL-FUNC-021 RPM/release gate evidence — 2026-08-06

This is the current-tree package cut that includes the installed-seat proof
boundary and the CPU-spike source mitigations. It was non-destructive: no RPM
was installed, no seat was rebooted, and Dell runtime was not modified.

## Farm and artifacts

- BigBoy: `172.20.0.130`.
- Isolated slot: `music-current-rpm-20260806-r2`.
- Base RPM: `magic-mesh-12.1.6-5.x86_64.rpm`, 87,320,171 bytes (83.3 MiB),
  SHA-256 `15e312d54ae4f0c120f84a6d663c62f7553d8ad3ee13148122a8174664238049`.
- Lighthouse RPM: `magic-mesh-lighthouse-12.1.6-5.x86_64.rpm`, 12,465,057
  bytes (11.9 MiB), SHA-256
  `08db0e24caa39f1e512c9db2e862ba95ed68d7c5ec8567f14761c9ccd0bfec88`.

## Results

The release compilation completed with warnings only. The base and lighthouse
payload, hard-Requires, manifest, and size checks all passed; each stayed below
the 90 MiB cut ceiling. The repository-wide `all` lane still reports the known
unrelated built-but-dead `mde-panel-egui` surface and is not promoted to green.

The local installed-seat verifier correctly remains red until the package is
explicitly installed: seat 15 still reports `magic-mesh-12.1.6-4.x86_64`, while
this cut is release 5. This is an honest package-version mismatch, not a live
seat mutation or a CPU-fix deployment.
