# WL-FUNC-021 RPM/release gate evidence — 2026-08-06

This bounded sidecar ran only non-destructive RPM/release gates. It did not
install an RPM, reboot a physical seat, or modify source or `WORKLIST.md`.

## Farm and artifact

- Farm host: BigBoy `172.20.0.130`.
- Isolated slot: `MCNF_BUILD_SLOT=wl-func-021-rpm-gates-r1`, remote directory
  `/home/mm/magic-mesh-farm-wl-func-021-rpm-gates-r1`.
- The slot was populated with the current existing artifacts from
  `/root/mcnf-release-artifacts`; no RPM build was started.
- Base RPM: `magic-mesh-12.1.6-4.x86_64.rpm`, 86,949,007 bytes (82.9 MiB),
  SHA-256 `66f06426a6503f4853473a0b9e7641fdeb9a079d7dccdd8ffe178c881dee2526`.
- Lighthouse RPM: `magic-mesh-lighthouse-12.1.6-4.x86_64.rpm`, 12,414,674
  bytes (11.8 MiB), SHA-256
  `6f82cb0a3a27e80b347a75d5dcb985f70382f5bd7b8e3439ab38c238064901b7`.

## Commands and results

Farm sync:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=wl-func-021-rpm-gates-r1 \
  ./install-helpers/xcp-build.sh sync
result: pass
```

On the isolated farm slot:

```text
./install-helpers/verify-rpm-payload.sh --self-test
result: pass; all assertions passed

./install-helpers/verify-rpm-payload.sh payload
result: pass; 191 manifest asset entries, key binaries, and source payload
  checks passed

./install-helpers/verify-rpm-payload.sh requirements
result: pass; source hard-Requires include libvirt, qemu-kvm,
  libvirt-daemon-kvm, and libvirt-daemon-driver-storage

./install-helpers/verify-rpm-payload.sh size magic-mesh-12.1.6-4.x86_64.rpm
result: pass; 82.9 MiB within the 90 MiB limit

./install-helpers/verify-rpm-payload.sh size magic-mesh-lighthouse-12.1.6-4.x86_64.rpm
result: pass; 11.8 MiB within the 90 MiB limit

./install-helpers/verify-rpm-payload.sh requirements \
  magic-mesh-12.1.6-4.x86_64.rpm
result: pass; actual RPM Requires header contains qemu-kvm and
  libvirt-daemon-kvm

./install-helpers/verify-rpm-payload.sh payload \
  magic-mesh-12.1.6-4.x86_64.rpm
result: pass; /usr/bin/mde-shell-egui and /usr/bin/mackesd present,
  every manifest asset present, actual Requires pass, size pass

./install-helpers/verify-rpm-payload.sh payload \
  magic-mesh-lighthouse-12.1.6-4.x86_64.rpm
result: pass; /usr/bin/mackesd present, every lighthouse manifest asset
  present, size pass
```

The repository-wide static command `./install-helpers/verify-rpm-payload.sh
all` returned 1 because it reports the unrelated existing `mde-panel-egui`
surface as built-but-dead (not mounted or shipped). The WL-FUNC-021 Music and
Media payload entries and both real RPM payload checks passed; this sidecar did
not alter that separate surface boundary.

## Current-tree RPM cut

After the dirty Music/Media manifests were reconciled with the minimal
Cargo.lock refresh, a fresh current-tree cut ran on BigBoy in isolated slot
`MCNF_BUILD_SLOT=music-current-rpm-20260806-r2`. The full release compilation
completed with pre-existing warning output only; the generated payloads then
passed `verify-rpm-payload.sh` hard-requirement, manifest, and size checks.

```text
magic-mesh-12.1.6-4.x86_64.rpm       87,249,507 bytes (83.2 MiB)
SHA-256 4bdc9f78f115f5d1242a5addff59eb4bb043e4f23c5fbf5e51986c69dc1b2f73

magic-mesh-lighthouse-12.1.6-4.x86_64.rpm  12,466,217 bytes (11.9 MiB)
SHA-256 3d81adeb373dac56a8bcffe211aefc62d24206ca2c0838133ea59474d905efd8
```

No RPM was installed and no seat was rebooted. The repository-wide `all`
surface warning described above remains unrelated and is not silently promoted
to a pass.
