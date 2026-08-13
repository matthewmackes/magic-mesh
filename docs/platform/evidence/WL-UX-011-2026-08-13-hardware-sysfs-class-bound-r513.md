# WL-UX-011 hardware sysfs-class provider bound — r513

Date: 2026-08-13

## Implementation

The credential-free hardware probe no longer copies the raw `/sys/class`
directory into a fleet-visible `PeerProbe` without a bound. It now admits only
typed directory or symlink class rows, sorts and deduplicates them before
selection, and caps the deterministic projection at 256 classes. A noisy or
compromised provider mount therefore cannot grow the replicated probe without
limit or change the retained identities through directory iteration order.

Production scope:

- `crates/mesh/mackesd/src/workers/hardware_probe.rs`

The focused hostile regression constructs 272 classes in reverse order plus a
non-class regular file and verifies the exact lexical 256-row projection and
type rejection. This is distinct from the device-inventory input-provider and
power-supply bounds.

## Farm evidence

- `.90`, slot `ux011-hw-sysfs-test`: focused hostile regression passed:

  `cargo test -p mackesd --lib sysfs_class_projection_is_typed_deterministic_and_bounded -- --nocapture`

- `.170`, slot `ux011-hw-sysfs-clippy`: strict all-feature library Clippy
  passed in 4m42s:

  `cargo clippy -p mackesd --lib --all-features -- -D warnings`

- `.196`, slot `ux011-hw-sysfs-fmt`: file-scoped Rustfmt passed:

  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/hardware_probe.rs`

The initial farm command wave used the library target name as a Cargo package
name and failed before compilation. Commands were corrected to package
`mackesd`; no duplicate implementation gate was run. Crate-wide formatting
also exposed unrelated existing drift outside the owned file, so the exact
file-scoped gate is the claimed formatting evidence. `.130` was not used.

`git diff --check` passed. Concurrent notification, worker, and Music edits
were preserved.

## Remaining acceptance

WL-UX-011 still requires complete provider/control coverage, first-release
package integration, and the explicitly deferred non-blocking post-release
one-node installed provider/control/fleet proof.
