# WL-UX-011 Surface Pro 5/6 shared contract checkpoint (r15)

Date: 2026-08-09

## Scope

This checkpoint establishes one bounded wire-contract source for Microsoft
Surface hardware observations and privileged intent, and makes the daemon's DMI
model explicitly distinguish Surface Pro 5 and Surface Pro 6 from unsupported
Surface generations. It also replaces several production stubs with bounded,
fail-closed observation paths and an explicitly gated activation path. It is a WL-UX-011 checkpoint, not
completion of image packaging or physical acceptance.

Changed production paths:

- `crates/mesh/mackes-mesh-types/src/surface_hardware.rs`
- `crates/mesh/mackes-mesh-types/src/lib.rs`
- `crates/mesh/mackesd/src/surface/mod.rs`
- `crates/mesh/mackesd/src/surface/enable.rs`
- `crates/mesh/mackesd/src/surface/mok_credential.rs`
- `crates/mesh/mackesd/src/surface/firmware.rs`
- `crates/mesh/mackesd/src/surface/verify.rs`
- `crates/desktop/mde-shell-egui/src/surface_card.rs`
- `crates/shared/mde-egui/src/display.rs`
- `crates/shared/mde-egui/src/drm.rs`
- `crates/shared/mde-egui/src/lib.rs`
- `packaging/bootc/Containerfile`
- `packaging/bootc/README.md`
- `packaging/bootc/verify-image.sh`
- `packaging/surface/README.md`
- `packaging/surface/surface-build-inputs.f44.json`
- `packaging/surface/surface-stack.schema.json`
- `packaging/surface/surface-stack.f44.json`
- `install-helpers/collect-surface-acceptance.py`
- `install-helpers/build-surface-userspace-f44.sh`
- `install-helpers/fetch-surface-build-inputs.sh`
- `install-helpers/verify-surface-stack.sh`
- `docs/ops/surface-pro56-acceptance-collection.md`

The shared schema provides:

- an exact schema version and Pro 5 / Pro 6 generation enum;
- bounded node, request, model, reason, probe-row, firmware-device, and wire
  sizes;
- explicit observation source, publication time, and fresh/stale/unavailable
  states;
- shared enable and firmware action envelopes; dead verify/display action
  contracts were removed;
- pre-effect rejection of unknown or duplicate fields, oversized records,
  foreign-node requests, and stale or implausibly future-dated requests.

The daemon preserves honest detection of other Surface models but records them
as `Unsupported` in this first-class Pro 5/6 contract.

Real Surface Pro 5 SMBIOS identity is also admitted without relying on the
synthetic `"Surface Pro 5"` fixture name. The Wi-Fi and LTE variants both report
the generic product name `"Surface Pro"`; only exact Microsoft vendor identity
plus product SKU `Surface_Pro_1796` or `Surface_Pro_1807` promotes that generic
name into the Pro 5 action contract. A missing, partial, padded, or foreign-vendor
SKU remains unsupported. Surface Pro 6 continues to require its exact product
name. The raw SKU is retained in the DMI observation while the shared product
label is canonicalized to `"Surface Pro 5"`.

The daemon enable/firmware consumers use the shared request types and perform
bounded admission before HMAC validation. The shell publishes only the firmware
request whose fresh inventory/release/checksum fields are present; Surface
activation and MOK reboot controls are visibly disabled until the shared
Preview/Commit/Cancel/Audit authority carries a fresh provider generation.
Externally published enable requests still cannot retrigger udev or write the
platform profile until those changes join the shared staged-control authority.
iptsd presets remain package-owned and rotation remains DRM-runner-owned. The
MOK sub-action is narrower: an already-authorized exact request may import only
the fixed package certificate when matching sealed systemd credentials are
present; the shell still emits no reboot intent.

Secure Boot and enrolled-key posture use fixed, bounded `mokutil` reads; module
checks are fixed sysfs reads. Pending MOK proof derives the complete SHA-1
fingerprint of the fixed DER package certificate in-process with the Rust
`sha1` crate and compares it with complete bounded `mokutil --list-new`
fingerprints; malformed, duplicate, truncated, or non-UTF-8 output fails closed.
SHA-1 is used only as mokutil's certificate identifier, not as a trust primitive.
MOK import consumes a sealed permit bound to the node, request id, action-auth
nonce/expiry, and exact certificate fingerprint, then confirms that fingerprint
through `mokutil --list-new`. Packaging the short-lived credential provisioner
and the typed host-state reboot handoff remains open.

The live firmware provider now performs fixed-argv, locale-stable, 20-second
bounded `fwupdmgr get-devices` and `get-updates` JSON reads, with concurrent
256-KiB pipe caps and explicit status, UTF-8, overflow, duplicate-key, duplicate
ID, row-count, and field-bound failures. A failed updates query makes the whole
inventory unavailable; it cannot fabricate an up-to-date result. Firmware apply
now binds the selected device to a fresh inventory publication timestamp, exact
release version, and exact lowercase SHA-256. The daemon immediately re-reads
fwupd inventory and requires the same device/release/checksum tuple before the
provider seam can run. The live seam then binds the exact HTTPS location and
declared size from the refreshed release, downloads into a private bounded
stage, verifies the SHA-256 in-process, and invokes only device-scoped
`fwupdmgr local-install`; broad update, activation, and reboot are never called.

Camera capability now uses fixed `/usr/bin/cam --list` enumeration with a
five-second timeout and 64-KiB caps on both pipes. It never opens a stream or
captures pixels. A strictly parsed enumerated pipeline is green for the
non-capturing provider fact, while separate privacy-armed frame proof remains
open. Fingerprint capability likewise uses only fixed read-only
`fprintd` `Manager.GetDevices`; it never claims a reader or reads enrolled-print
data. Missing, malformed, failed, ambiguous, or oversized output remains an
honest failure.

The verification worker now publishes the shared bounded verify-board and fleet
summary types directly, and the shell consumes those same types instead of
private serde mirrors. Hostile or oversized state is rejected while cursors
advance, so a bad record cannot pin polling. The shell additionally rejects
foreign-node, older-than-90-second, and implausibly future publications rather
than displaying an indefinitely `Fresh` producer assertion. Contradictory
unavailable/skipped facts with healthy rows, non-neutral summaries, or firmware
devices are rejected. This Node binds only its exact local hostname and clears
retained Surface state when that local summary lane disappears; a remote Surface
topic can neither populate the card nor receive an action from it.

The read-only acceptance collector records exact DMI model/SKU, F44 immutable
deployment and package NEVRAs, kernel/module signers and MOK posture, iptsd,
touch/pen/Type Cover/button candidates, MMC, SAM/IIO/platform profile, DRM,
enumeration-only cameras, identifier-free radios, projected fwupd inventory,
inventory-only audio, battery/suspend/hibernate/S0ix, and service/binary
revisions. Commands and artifacts are bounded, output is redacted and
SHA-256-bound, and the manifest always records
`physical_acceptance_claimed: false`. It cannot substitute for hands-on proof.

The bootc compose no longer treats a missing linux-surface repository, signing
key, or RPM as a successful best-effort build. Its static image verifier now
requires the Surface RPM set, the `iptsd@.service` per-device template, the
linux-surface Secure Boot certificate, and explicit iptsd presets for both Pro 5
and Pro 6. This deliberately exposes the current Fedora 44 repository gap as a
compose failure instead of shipping a false support claim. The verifier passed
`bash -n`; no image compose is claimed because the required F44 repository is
not yet available. The compose copies the governed F44 manifest and refuses to
continue while its exact status is not `ready`, preventing a later upstream
repository appearance from silently bypassing the provenance gate.

The hardened provenance verifier requires exactly `kernel-surface`, `iptsd`,
`libwacom-surface`, `surface-control`, and `surface-secureboot`, including local
artifact filenames, exact whole-file hashes and NEVRAs, a pinned local RPM key,
an exact Fedora 44 bootc base digest, and required
kernel/module signer plus certificate bindings. The committed manifest is
honestly `blocked` with all unavailable immutable values null.

The producer side now has a separate Fedora 44 build-input lock. It binds a
digest-pinned Fedora 44 builder image and the complete official source set:
linux-surface packaging, the exact Fedora kernel-ark tag, iptsd, the
libwacom-surface patch set plus its upstream libwacom archive, surface-control,
secureboot-mok, and the exact Surface certificate. Every input carries an
immutable commit/ref, HTTPS URL, filename, SHA-256, and license; the five package
rows map to their complete ordered input sets. This lock is not RPM readiness.

The build driver now runs that strict verifier before RPM staging, registry
access, or Podman. A valid blocked manifest exits through
`GATED[WL-UX-011/surface-provenance]`; malformed provenance is a refusal. If the
manifest later becomes ready, the driver and Containerfile require its exact
Fedora 44 base digest and install only verified local RPM paths; they do not
import a mutable network key or resolve unconstrained Surface package names.
This records rather than silently
settles the project-wide baseline decision.

The live mutation seams are no longer blanket stubs. Firmware apply now
re-reads updates, binds one exact device/version/SHA-256/HTTPS location/declared
size, downloads into a private bounded stage, verifies the cabinet in-process,
and invokes only device-scoped `fwupdmgr local-install`; it never invokes broad
update, reboot, or activation. MOK import now consumes a short-lived mde-seal
permit plus password from two fixed systemd credentials, binds them to the
node, request, action-auth nonce/expiry, and exact certificate, keeps the secret
off argv/environment/hashfiles, and proves the exact pending fingerprint after
`mokutil` returns. The Surface display control now queues a typed request to the
sole DRM runner, which tears down and rebuilds its existing GBM/EGL session at
an advertised connector mode, acknowledges only after committed scanout, and
rebuilds the prior mode on a pre-commit target failure.

On 2026-08-09, a bounded direct request for
`https://pkg.surfacelinux.com/fedora/f44/repodata/repomd.xml` returned HTTP 404.
The linux-surface package-repository branch observed at the same time was
`u/staging` commit `038d18390aaaa487c42fe914101c3b5dd29ec682`. No fallback to
the F43 repository, mutable workflow artifact, or guessed RPM identity was
admitted.

## Farm proof

Shared hostile contract tests ran on build host `172.20.0.50`, slot
`surface-contract-r1`:

```text
cargo test -p mackes-mesh-types surface_hardware --locked -- --nocapture
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 493 filtered out
```

The tests cover explicit Pro 5/6 admission, duplicate/unknown fields, wire and
field bounds, duplicate probe and firmware rows, stale/future/foreign action
requests, and the closed display-mode vocabulary.

The focused daemon mapping test ran on build host `172.20.0.90`, slot
`surface-pro56-contract-r2` after BigBoy (`172.20.0.130`) was unavailable:

```text
cargo test -p mackesd --features async-services --lib \
  surface::tests::surface_pro_5_and_6_map_to_the_explicit_shared_contract \
  --locked -- --nocapture
running 1 test
test surface::tests::surface_pro_5_and_6_map_to_the_explicit_shared_contract ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 4632 filtered out
```

The daemon crate emitted its existing warning baseline; the focused test passed.
`git diff --check` also passed.

Consumer gates ran on separate farm slots:

```text
172.20.0.90 / surface-enable-contract-r1
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 4629 filtered out

172.20.0.90 / surface-firmware-contract-r1
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 4625 filtered out

172.20.0.50 / surface-shell-contract-r1
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 1508 filtered out
```

These cover stale/foreign enable refusal, duplicate/foreign firmware refusal,
typed arming plus exact-body HMAC enforcement, replay/cursor behavior, and shell
shared-contract round trips. All suites emitted only their existing warning
baselines.

The coherent live-path suites then ran on three farm lanes:

```text
172.20.0.90 / surface-enable-r2
running 29 tests
test result: ok. 29 passed; 0 failed; 0 ignored; 4621 filtered out

172.20.0.130 / surface-firmware-r2
running 31 tests
test result: ok. 31 passed; 0 failed; 0 ignored; 4619 filtered out

172.20.0.50 / surface-verify-r2
running 21 tests
test result: ok. 21 passed; 0 failed; 0 ignored; 4629 filtered out
```

These prove the fail-closed activation gate, Pro 5/6-only identity gate, MOK
pending-proof refusal, bounded fwupd reads and no-apply seam, hostile inventory
admission, bounded libcamera enumeration, no-capture classification, and exact
Pro 5/6 model evidence. Targeted `rustfmt --check` passed for every changed Rust
implementation file. The repository-wide formatter still reports unrelated
pre-existing formatting drift outside this checkpoint.

After replacing the synthetic Pro 5 fixture identity with the real generic
product-name plus exact SKU rule, the complete daemon Surface namespace was
rerun on `172.20.0.90`, warm slot `surface-mok-hardening-r1`:

```text
cargo test -p mackesd --features async-services --lib surface:: --locked \
  -- --nocapture
running 105 tests
test result: ok. 105 passed; 0 failed; 0 ignored; 4552 filtered out
```

This includes positive Wi-Fi/LTE Pro 5 SKU tests, missing/near-miss/foreign
identity refusal, Pro 6 admission, all enable/MOK tests, fwupd observation and
refusal tests, and shared verification publication tests. The warning output is
the crate's existing baseline.

The provenance verifier ran on `172.20.0.50`, slot `surface-provenance-r1`:

```text
Surface artifact provenance self-test passed (18 hostile fixtures rejected)
blocked_rc=3
BLOCKED: Fedora 44 Surface stack provenance is unavailable
```

`git diff --check`, shell syntax, shellcheck, and JSON parsing passed. The
blocked exit is the intended result, not a passing package/image claim.
The bootc driver preflight was also exercised with a no-op Podman fixture and
returned code 3 at the Surface provenance gate before invoking Podman.

The acceptance collector's local parser/admission suite passed 14 hostile
redaction, identity, and bounded-fwupd fixtures. A non-Surface dry collection
and its hash validator both returned the expected incomplete exit 3; no live
Surface bundle is claimed because governed seat access remains unavailable.

The shared firmware selection cutover was then verified on `172.20.0.90`, warm
slot `surface-mok-hardening-r1`:

```text
cargo test -p mackesd --features async-services --lib \
  surface::firmware:: --locked -- --nocapture
running 32 tests
test result: ok. 32 passed; 0 failed; 0 ignored; 4626 filtered out

cargo test -p mde-shell-egui surface_card::tests --locked -- --nocapture
running 8 tests
test result: ok. 8 passed; 0 failed; 0 ignored; 1508 filtered out
```

These suites cover SHA-256 selection, stale/missing/mismatched binding refusal,
the immediate inventory re-read, exact seam arguments, shared publication,
wire-shape serialization, and shell refusal of hostile, foreign, future, and
aged state.

The current shared Surface tree was then re-synchronized and gated as a unit:

```text
cargo test -p mackes-mesh-types surface_hardware --locked -- --nocapture
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 493 filtered out

cargo test -p mackesd --features async-services --lib surface:: --locked \
  -- --nocapture
running 106 tests
test result: ok. 106 passed; 0 failed; 0 ignored; 4552 filtered out
```

The corrected final review wave used the exact current worktree on isolated
farm slots:

- `172.20.0.50 / surface-final-types-r19`: all 10 shared hardware-contract
  tests passed after separating fresh/skipped contradiction coverage from the
  nonfresh-with-rows/device rejection cases;
- `172.20.0.130 / surface-corrected-daemon-r18`: all 106 daemon `surface::`
  tests passed;
- `172.20.0.130 / surface-corrected-shell-r18`: all 8 shell Surface-card tests
  passed;
- `172.20.0.130 / surface-final-fmt-r19`: exact-file `rustfmt --check` passed
  for the six changed Rust implementation modules;
- `172.20.0.90 / surface-final-scripts-r19`: the provenance verifier rejected
  18 hostile fixtures and returned the intended blocked code 3 for the absent
  governed artifacts, while the collector rejected its 14 hostile/bounded
  fixtures; and
- `172.20.0.50 / surface-final-types-r19`: `bash -n`, `shellcheck`, and JSON
  parsing passed for the changed packaging/verifier inputs.
- `172.20.0.130 / surface-source-lock-r1`: the new build-input validator
  rejected 10 hostile fixtures, then fetched all eight locked upstream inputs
  and revalidated the emitted ten-file bundle with `sha256sum -c`.
- `172.20.0.130 / surface-userspace-producer-r3`: the governed producer
  revalidated the exact eight-input bundle, built inside the digest-pinned
  Fedora 44 image, and emitted hash-checked unsigned
  `iptsd-3.1.0-1.fc44.x86_64.rpm` plus its source RPM and explicit unsigned
  build manifest. Two earlier fail-closed iterations exposed and corrected the
  archive-without-Git-metadata case and deterministic Git date portability.
  Fedora build dependencies were resolved from the live F44 repositories, so
  this is hash-bound producer proof, not a hermetic or bit-reproducible claim.
- Follow-on isolated BigBoy producer runs emitted and hash-checked the exact
  Fedora 44 unsigned artifact sets for `libwacom-surface` (main, data, devel,
  utils, and source RPMs), `surface-control` (x86_64 and source RPMs), and
  `surface-secureboot` (noarch and source RPMs). Each output includes its exact
  source rows, digest-pinned builder, `signed:false`, installed build-environment
  RPM inventory, and `SHA256SUMS`; hostile extra-directory and symlink bundle
  fixtures were refused.
- `172.20.0.130 / surface-fw-exact-test-r2`: all 36 exact firmware inventory,
  admission, private-stage, bounded-download, hash/size, and device-scoped
  local-install tests passed.
- `172.20.0.90 / surface-mok-hardening-r1`: all 36 Surface enable/MOK tests
  passed after the sealed credential and exact pending-certificate path landed.
- `172.20.0.90 / surface-verify-contract-r2`: all 10 DRM display tests and all
  8 DRM-enabled Surface-card tests passed for request/commit/rollback behavior.
- `172.20.0.90 / surface-coherent-daemon-r20`: the final synchronized
  current-tree daemon Surface suite passed all 117 tests, with 4,552 filtered
  and no failures, including the privacy-preserving camera/fprintd probes.
- `172.20.0.50 / surface-coherent-shell-r20`: the synchronized DRM-enabled
  shell Surface-card suite passed all 8 tests, with 1,514 filtered and no
  failures.
- `172.20.0.170`: the privacy-preserving camera/fingerprint focused suite
  passed all 26 verify tests, with 4,643 filtered and no failures; exact-file
  `rustfmt --check` also passed.
- `172.20.0.130 / surface-integrate-daemon-r22`: the final synchronized daemon
  Surface suite passed all 117 tests, with 4,552 filtered and no failures.
- `172.20.0.90 / surface-integrate-drm-r22`: the complete DRM-enabled shared
  `mde-egui` library passed all 329 tests.
- `172.20.0.50 / surface-integrate-shell-r22`: the final synchronized
  DRM-enabled shell Surface-card suite passed all 8 tests, with 1,514 filtered
  and no failures.
- `172.20.0.196 / surface-integrate-fmt-r23`: exact-file `rustfmt --check`
  passed for all nine changed Rust implementation modules.
- `172.20.0.170 / surface-integrate-packaging-r23` and
  `172.20.0.50 / surface-integrate-shellcheck-r23`: Bash syntax, ShellCheck,
  JSON parsing, the ten-hostile-fixture source-lock self-test, and lock
  validation passed for the retained producer files.

A proposed secret-bearing kernel producer was rejected during integration and
removed before commit. Independent review found that it mounted the Secure Boot
private key into a networked container executing upstream build code and then
embedded that key in a transient SRPM. It also did not verify the complete
kernel/module signer set. This checkpoint therefore retains kernel production
as missing rather than recording a misleading readiness pass.

`git diff --check` passed after the exact formatter wave. These are checkpoint
gates only: the missing governed Fedora 44 artifacts and live Surface
credential still prevent image/deployment acceptance.

## Remaining acceptance work

- Package a privileged provisioner that atomically mints the action token,
  seals its nonce-bound MOK permit, and injects both fixed systemd credentials
  inside the 30-second window; add the typed host-state reboot handoff.
- Run privacy-armed camera frame and fingerprint functional hardware proof; the
  production inventory probes now verify libcamera/fprintd stack enumeration
  without capture, claim, enrollment, authentication, or enrolled-print reads.
  Exercise the new firmware and DRM mutation paths in a recovery-ready Surface
  hardware window; unit/farm gates deliberately performed no firmware or KMS
  mutation.
- Build kernel-surface from the pinned inputs without exposing a signer to
  networked or upstream build code; sign the kernel and every module in a
  minimal network-disabled stage, rebuild `surface-secureboot` with the same
  public certificate, sign the complete RPM set with the project release key,
  then compose and deploy the current candidate to the `Surface` Pro 6 seat.
  The existing unsigned userspace/data builds are producer proof only, and the
  compose still fails closed without the complete governed artifact set.
- Restore governed SSH/current-release access; the documented key is currently
  rejected on LAN and the overlay path is unavailable. On 2026-08-09 the Pro 6
  repeatedly answered LAN ICMP at `172.20.146.79` (latest 0.443 ms), but
  rejected the governed key for both `root` and `mm`; `10.42.0.7` did not answer
  ICMP.
- Run direct touch, pen, Type Cover, SAM, accelerometer/rotation, camera,
  Wi-Fi/Bluetooth, S0ix, fingerprint, DRM mode, audio, suspend, reboot, and
  upgrade-return proof on Surface Pro 6.
- Add the user's Surface Pro 5 seat when physical parity proof is required and
  run the same acceptance board there.

No first-class Surface acceptance or active-worklist closure is claimed by this
checkpoint. The canonical worklist records this bounded checkpoint while
retaining WL-UX-011 as `Remaining`.
