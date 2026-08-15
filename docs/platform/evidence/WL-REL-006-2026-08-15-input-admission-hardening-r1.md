# WL-REL-006 input-admission hardening — r1

Date: 2026-08-15

Commit `074dde8a8ba6b78652ef9c3bc77d3b662612ac84` closes two admission gaps
without claiming that first-release inputs exist:

- App VM preflight now requires the canonical base-image receipt, image
  reference, and architecture. The owning inspector re-resolves the registry
  manifest and binds its platform digest to the exact source revision and
  epoch before any build mutation. The former raw digest input is removed.
- Kiron preflight now passes the requested source revision to the owning
  package verifier. The verifier requires the canonical package paths and
  proves that the admitted manifest and payload are unchanged from that exact
  Git commit.
- The Surface kernel producer now accepts upstream's legitimate parent-
  relative in-tree links while still refusing absolute and normalized escaping
  links. The reusable archive verifier covers those hostile cases before
  extraction.
- Commit `0a540feedb70a362eb3513d8323aa9099dbf4a03` adds the executable private
  argv contract. It accepts one owner-only, single-link, mode-0400 JSON file,
  enforces an exact duplicate-free schema and digest-pinned image references,
  maps exactly two canonical Cuttlefish DEBs, and executes the owning preflight
  without logging values. The RPM lane binds that document to its independently
  resolved clean-checkout revision and epoch before immutable farm sync.

Focused farm results:

```text
.50 / slot preflight-app-kiron
install-helpers/test-release-input-preflight.sh
PASS: all mandatory fixture inputs admitted
PASS: App VM receipt revision/epoch/reference/architecture/manifest/platform binding
PASS: missing, mismatched, and substituted inputs stopped before build mutation

.196 / slot kiron-revision
packaging/kiron/verify-package.sh --self-test
PASS: schema hostility, RPM wiring, revision binding, and missing package rejection

.130 / slot surface-kernel-r2
install-helpers/verify-surface-source-archive.py --self-test
PASS: safe in-tree parent-relative link accepted; escaping and absolute links refused
python3 install-helpers/verify-surface-source-archive.py <linux-surface archive> <kernel-ark archive>
PASS: both exact digest-locked upstream archives admitted

.50 / slot private-argv
python3 install-helpers/test-release-input-argv.py
install-helpers/test-release-input-preflight.sh
PASS: strict private file/schema/path/reference/source-identity contract
PASS: canonical preflight and phase-boundary hostile suites
```

The current full-release freeze remains
`1dfe6906609d71da9ee2ce20c860912a09b32855` at epoch `1786813297`; the stale
worklist text that paired `d248ba2f` with that parent's epoch is corrected.
The newer `faa1b0de...`/`074dde8a...` work belongs to the separately governed
`DEV-SNAPSHOT — NOT A FULL RELEASE` lane and does not silently move the full-
release freeze.

Real Maps approval/source bytes, App VM trust/base inputs, a governed
Cuttlefish image and declaration, bootc receipt, current signer receipt, and
the private preflight argv remain absent. WL-REL-006 therefore remains
`Remaining`, and every downstream release epic remains blocked.
