# WL-ARCH-008 host Browser negative boundary — 2026-08-08 r1

## Scope

This checkpoint audits production source, package, image, installer, and runtime
paths for Browser engines that violate the Browser-VM boundary. Guest image code
under `packaging/browser-vm`, Browser-VM control helpers, and the shell's typed
VDI controller remain allowed and are not treated as host Browser engines.

## Implementation

`install-helpers/lint-browser-vm-boundary.sh` rejects retired host Browser path
families and exact engine/package signatures. It also fails closed when a
required scan root is absent. Its self-test proves that guest/VDI integration is
accepted, a retired engine crate path and a renamed runtime policy are rejected,
a host Browser package and engine installer are rejected, and an incomplete scan
is rejected. The lint and self-test are part of the canonical `ci-gate.sh`
policy stage.

## Remediation result

The two production remnants identified by the first fail-closed audit are now
removed:

- The host matching engine, bundled host rule assets, workspace member, lockfile
  package, and `mackesd` dependency are deleted. The retained `adfilter` worker
  is now only a Browser-VM policy-envelope/allowlist replicator: it discovers
  operator mirror payloads, converges opaque policy over the mesh, and reports
  line-count metadata without matching browser requests on the host.
- `packaging/kickstart/magic-on-quasar.ks` no longer describes the retired host
  Browser SELinux domains.

The lint continues to reject retired crate/package paths and live source,
manifest, installer, image, and policy signatures. Rust comment-only lines are
excluded from signature matching because they cannot establish runtime
reachability; path and manifest checks remain fail closed.

## Verification

- `install-helpers/lint-browser-vm-boundary.sh --self-test`
- `bash -n install-helpers/lint-browser-vm-boundary.sh install-helpers/ci-gate.sh`
- `shellcheck install-helpers/lint-browser-vm-boundary.sh`
- `install-helpers/lint-browser-vm-boundary.sh` (clean real-repository audit)
- `.90`, slot `arch008-adfilter`: `cargo test -p mackesd --lib
  workers::adfilter::tests -- --nocapture` — **11 passed, 0 failed**.
- `.90`, slot `arch008-adfilter`: `cargo metadata --locked --no-deps
  --format-version 1` — passed, proving the lockfile and workspace graph no
  longer require the deleted crate.
- `.90`, slot `arch008-adfilter`: direct `rustfmt --edition 2021 --check` for
  `workers/adfilter.rs` — passed.

The first unscoped `cargo test -p mackesd ...` attempt also compiled the binary
test target and exposed an unrelated concurrent WL-ARCH-009 error in
`src/bin/mackesd.rs`: `expect_err` requires `Cli: Debug`. The library-only gate
above isolates and proves this tranche without modifying that agent-owned file.
