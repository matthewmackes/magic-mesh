# WL-CRIT-006 immutable source-revision receipt — 2026-08-10

The native farm and Fedora-container RPM paths now resolve one clean exact Git
commit before synchronization, export its 40- or 64-hex object ID and commit
epoch into Cargo, and synchronize an immutable `git archive` of that revision.
The `mde-theme` build stamp therefore places that complete revision in
`mackesd --version` and the shared About identity. It no longer emits `nogit`.

A promotable build fails before synchronization when HEAD is unresolved or the
checkout has tracked, staged, or untracked changes. The build script independently
rejects a missing, malformed, mismatched, or dirty promotable receipt when Git
metadata is available. An ordinary developer or gitless build remains buildable,
but reports an explicit `non-promotable-*` marker.

## Focused proof

- Build VM: `172.20.0.196`; slot `provenance-r1`.
- `install-helpers/test-source-revision-provenance.sh` passed. It compiled and
  executed the real `mde-theme/build.rs`, proved the exact fixture commit was
  stamped, proved mismatch and dirty source were rejected, and proved gitless
  source reported `non-promotable-unresolved` rather than `nogit`.
- `install-helpers/source-revision-receipt.sh --self-test` passed its clean exact
  receipt plus dirty and malformed refusal fixtures.
- On the same farm slot, `cargo test -p mde-theme brand::build::tests --locked
  -- --nocapture` passed 5/5 build-identity tests.
- `cargo fmt -p mde-theme -- --check` passed on the same farm slot.
- `install-helpers/xcp-build.sh --route-test` passed all routing checks, including
  the `camera; H` Cargo-argument regression. Cargo arguments are now shell-quoted
  before SSH, preventing trailing text from executing after a successful test.
- `bash -n` and `git diff --check` passed for the touched build helpers and stamp.
- A native `xcp-build.sh rpm` probe against the intentionally dirty concurrent
  checkout exited 1 before synchronization with `checkout is dirty; refusing a
  promotable source receipt`. A direct container-RPM probe without a receipt also
  exited 1 before Podman startup.

No release RPM was cut in this focused gate: doing so from the concurrent dirty
checkout would contradict the fail-closed acceptance condition. The next clean
candidate cut will consume the committed snapshot and must report its exact
receipt revision through `mackesd --version` before promotion.
