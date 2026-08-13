# WL-ARCH-010 — Workloads Podman hard requirement (r518)

Date: 2026-08-13

## Missing first-release behavior closed

The production base and headless Server RPM manifests exposed `podman` only as
a weak `Recommends`, while the sole Workloads container actuator directly uses
Podman to inspect/load OCI images and materialize Quadlets for systemd. A fresh
installation with weak dependencies disabled could therefore install and
activate `mackesd-compute.service` but could not reconcile any container
workload.

`podman` is now a hard `Requires` in both compute-capable RPM shapes. The package
verifier checks the base and Server source tables independently, checks the
built base RPM Requires header, and rejects weak-only or similarly named
dependencies such as `podman-remote`.

No release or RPM was built in this slice.

## Changed files

- `crates/mesh/mackesd/Cargo.toml` (RPM metadata only)
- `install-helpers/verify-rpm-payload.sh`

## Farm evidence

- `172.20.0.130`, slot `arch010-podman-selftest-r518`:
  `install-helpers/verify-rpm-payload.sh --self-test` passed every assertion,
  including weak-only source metadata and similarly named built-RPM dependency
  rejection.
- `172.20.0.170`, slot `arch010-podman-metadata-r518`:
  `cargo metadata --no-deps --format-version 1` and
  `install-helpers/verify-rpm-payload.sh requirements` passed. The exact source
  gate reported hard `podman` requirements for both base and Server shapes.
- `172.20.0.50`, slot `arch010-podman-shellcheck-r518`:
  `shellcheck -e SC2015,SC2016,SC2053,SC2254
  install-helpers/verify-rpm-payload.sh` passed. The exclusions are nine
  pre-existing findings in untouched Browser/App/Android verifier lines; the
  initial unexcluded run surfaced only those findings.
- Local orchestration checks: `bash -n install-helpers/verify-rpm-payload.sh`
  and scoped `git diff --check` passed.

## Remaining WL-ARCH-010 acceptance

The first full release must still build the RPM and prove `podman` is present in
the actual RPM Requires header alongside the existing KVM/Display1 dependencies.
After that release, the deferred non-blocking one-node acceptance must exercise
VM and container lifecycle, Display1/KMS attachment, recovery, and installed
payload behavior.
