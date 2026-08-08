# WL-FUNC-021 current-tree RPM gate evidence — 2026-08-06

This is a fresh non-destructive package cut after the Music mutation
authorization and credential-provisioning changes. No RPM was installed, no
seat was rebooted, and Dell runtime state was not changed.

## Farm and build

- Farm host: BigBoy `172.20.0.130`.
- Isolated slot: `MCNF_BUILD_SLOT=music-auth-rpm-20260806-r1`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-auth-rpm-20260806-r1 ./install-helpers/xcp-build.sh rpm`.
- The workspace release build and feature-enabled `mde-shell-egui` build
  completed with the repository's existing warning output only.

## Artifacts and gates

```text
magic-mesh-12.1.6-4.x86_64.rpm
  87,299,902 bytes (83.3 MiB)
  SHA-256 4e9ac7f57305ecd6c41026c5b7728f7fa8142a512fac31b7e097273ac66e19e0

magic-mesh-lighthouse-12.1.6-4.x86_64.rpm
  12,466,082 bytes (11.9 MiB)
  SHA-256 e48fd61193943615667abe3358510be4c583552c3fb64ee334b5bf404b1f9796
```

The farm build's hard-requirement, manifest, payload, and size checks all
passed for both generated RPMs. The 90 MiB payload ceiling passed: the base
package is 83.3 MiB and the lighthouse package is 11.9 MiB.

The repository-wide `verify-rpm-payload.sh all` warning about the unrelated
pre-existing `mde-panel-egui` built-but-dead surface remains separate from
these Music/Media package gates.
