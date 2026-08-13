# WL-ARCH-010 compute RPM runtime contract — 2026-08-13

## Gap closed

The Fedora RPM builder compiled the typed Workloads actuator into both the
`magic-mesh` base package and the headless `magic-mesh-server` variant, but the
server variant replaced the base dependency table with a smaller table. Podman
and libvirt were only weak recommendations there, while KVM, QEMU's D-Bus
display backend, the libvirt storage driver, guest image tooling, cloud-init ISO
tooling, and virtiofs were absent. A clean headless compute install could
therefore produce a successful RPM while lacking the runtime needed for the
sole Quadlet/libvirt actuator.

`install-helpers/build-rpm-fedora43.sh` now applies a deterministic packaging
transform in its disposable farm checkout before `cargo generate-rpm`. Both
compute manifests hard-require exactly these runtime providers:

- `podman`
- `libvirt`
- `qemu-kvm`
- `qemu-ui-dbus`
- `libvirt-daemon-kvm`
- `libvirt-daemon-driver-storage`
- `guestfs-tools`
- `genisoimage`
- `virtiofsd`

Existing hard requirements remain exact-once, weak metadata remains intact,
and an exit trap restores the source manifest on success or failure. The thin
Lighthouse package remains governed by its separate replacement manifest and
does not inherit compute dependencies. After each base or server artifact is
generated, the builder queries the actual RPM header and fails the cut if any
provider is absent. This is separate from, and does not modify or duplicate,
the concurrently owned general payload verifier.

## Farm evidence

- `.50`, slot `arch010-rpm-build-syntax-r499`: final-source
  `bash -n install-helpers/build-rpm-fedora43.sh` passed.
- `.90`, slot `arch010-rpm-runtime-selftest-r499`:
  `install-helpers/build-rpm-fedora43.sh --self-test` passed. The hostile
  fixture began with only one base hard dependency, one server hard dependency,
  and weak Podman/libvirt entries; both transformed compute sections contained
  all nine hard providers exactly once without corrupting the weak entry.
- `.196`, slot `arch010-rpm-runtime-manifest-r499`: the self-test passed and the
  internal build transform was applied to the real mackesd packaging manifest;
  static inspection reported `transformed-runtime-contract-pass`.
- Local tiny probes: final-source shell syntax, self-test, and
  `git diff --check` passed. No local build or heavy test ran.

The first attempted parallel command composition was rejected locally before
execution because it contained explicit temporary cleanup syntax. It is not
claimed as farm evidence.

## Scope and residual acceptance

This slice changes only `install-helpers/build-rpm-fedora43.sh` and this evidence
record. Concurrent Rust changes and the active `verify-rpm-payload.sh` owner
were not touched.

Remaining WL-ARCH-010 acceptance is a full release RPM/repository transaction,
real libvirt/Quadlet `StartAndAttach` readiness, native KMS/Display1 recovery,
and the deferred post-release installed-seat/fleet lifecycle matrix.
