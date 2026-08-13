# WL-CRIT-006 — canonical release-output end-to-end integration (r528)

Date: 2026-08-13

`install-helpers/test-release-output-plan-collector-integration.py` closes the
previously untested boundary between the canonical seven-role plan producer and
the immutable output collector. It constructs internally consistent governed
fixtures and invokes `produce-release-output-plan.py`,
`collect-release-outputs.py`, and all seven repository-owned verifier entry
points. It does not substitute repository verifiers.

Only external system observations are hermetic: RPM metadata/signature/key and
payload extraction tools, plus `qemu-img` metadata. The fixture uses real RPM
candidate schemas, a frozen Browser profile and real Browser manifest builder,
the App VM qcow2 schema, a Cuttlefish artifact receipt, and a bootc digest
receipt, all bound to the exact checked-out Git revision and commit epoch.

The positive assertion admits exactly Workstation RPM, Server RPM, Lighthouse
RPM, Browser VM, App VM, Cuttlefish image, and bootc image. It independently
checks each canonical media type, source revision, signing fingerprint,
SHA-256, size, immutable output mode, and the permanently forbidden promotion
state. Hostile integration checks prove that cross-role companion reuse is
refused before plan publication and that artifact mutation after plan
publication is refused during collection.

## Farm evidence

- `.50`, slot `crit006-plan-collector-e2e-r528`: a clean clone at `ba79bd84`
  ran `python3 install-helpers/test-release-output-plan-collector-integration.py`.
  Result: `release-output plan/collector seven-verifier integration: PASS`.
  A clean clone was required because the collector intentionally verifies each
  owning executable against the pinned Git tree; ordinary farm rsync copies do
  not contain `.git` metadata.
- `.170`, slot `crit006-plan-collector-compile-r528`:
  `python3 -m py_compile install-helpers/test-release-output-plan-collector-integration.py`
  passed.
- `.196`, slot `crit006-plan-collector-tabnanny-r528`:
  `python3 -m tabnanny install-helpers/test-release-output-plan-collector-integration.py`
  passed.
- Local `git diff --check` passed.

ShellCheck is not applicable because the implementation is Python. No existing
producer, collector, packaging verifier, Browser/profile, Maps, Android/Rust,
first-release driver, or worklist file was changed by this slice.

## Remaining WL-CRIT-006 acceptance

- Supply the real governed signing key and signed Workstation, Server, and
  Lighthouse RPM candidates for the release revision.
- Supply the real immutable Browser, App, Cuttlefish, and bootc outputs and
  their owning manifests/receipts.
- Run the canonical plan/collector chain as part of the first full release and
  retain its immutable manifest with release evidence.
- Complete deferred, non-blocking post-release one-node acceptance and recovery
  proof.
