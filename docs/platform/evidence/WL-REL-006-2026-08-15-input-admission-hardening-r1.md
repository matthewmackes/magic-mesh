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

## 2026-08-16 farm progress

The approved fixture lanes were spread across separate farm VMs:

- `.50`: `packaging/maps/test-produce-offline-catalog.py` — hostile producer
  suite passed. A bounded OpenStreetMap-derived fixture was then produced from
  exact immutable tile bytes, with the current source revision, ODbL-1.0
  attribution, and a 1 KiB quota. Bundle hashes: `manifest.json`
  `4a69db176c09a126bc56d69513cc45d32d51be0534b7e318888ccfae514b12c9`,
  `catalog.json`
  `94436590b0c1ecbea2da54ad4c6dfc5439d4e0308a708d39e2a2f99137a9d801`, and
  `payload/index.json`
  `1a7e375dcbc60d083a16578ec0c45ba0e7a09b04422122045f601f61b02a2d6a`.
  This remains a fixture pending the production Maps verifier/materializer and
  live-provider proof. The standalone Rust verifier was built on `.50` and the
  canonical materializer then admitted the bundle into the cache. Materialized
  hashes: `catalog.json`
  `94436590b0c1ecbea2da54ad4c6dfc5439d4e0308a708d39e2a2f99137a9d801`,
  `index.json`
  `1a7e375dcbc60d083a16578ec0c45ba0e7a09b04422122045f601f61b02a2d6a`, and
  the cache tile
  `a4ea7e324c31756db914b68b72094a5366da7922267f04a1ba6570339f5e9d44`.
- `.90`: `packaging/app-vm/test-produce-base-image-receipt.py` — hostile base
  receipt suite passed. The real official Fedora registry inspection then
  produced an App VM fixture receipt for `quay.io/fedora/fedora:42`, bound to
  the current source revision and epoch. Receipt SHA-256:
  `3b1b27592c84715a8261a2e646c8d5f45b35c59b52d7980b45b5531afe9144be`;
  resolved manifest digest:
  `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`;
  amd64 platform digest:
  `sha256:63773f454664cd77e239f8e0b13ae7f18effe9e3d6612a325b5646eb3bda11f1`.
  It remains labeled a fixture pending downstream Wayland/App VM acceptance.
- `.170`: `install-helpers/test-produce-bootc-digest-receipt.py` — hostile
  receipt suite passed; the real official Fedora bootc index was then resolved
  against source revision `daf3c695928e96553fe839450bd86aa6f371e3aa`, epoch
  `1786817528`, producing digest
  `sha256:35f5a8e7e7417a3b15a4d62d1a950ab8a873af0a0a8c20105d079224c01ac64c`.
- `.130`: after supplying a detached source-revision bundle for the fail-closed
  Git check, `packaging/android/test-stage-guest-runtime-artifacts.sh` built and
  verified the x86_64 release guest artifacts in 1m23s. The staging hostile
  suite passed. This proves the MCNF guest package stage only; no Android image,
  signed declaration, or external Cuttlefish host package is claimed. The
  follow-on guest DEB build completed from that stage, producing:
  `guest-deb-manifest.json` SHA-256
  `f3f40332a1a32d9ffa6c16507db258c1c00736d35047d3759f55d5994780f37f`,
  `mcnf-cuttlefish-readiness-relay.deb` SHA-256
  `02651defef8ef71e30538613ed364c6df77ab866f9026f4e125923bbc54fb557`, and
  `mcnf-cuttlefish-vdi-agent.deb` SHA-256
  `ec4800a05b0c3aaceac9a19436c3e461c90925b8ef09b20bbdc4ddb4a5efedb5`.

These results advance the fixture-verification lane but do not close WL-REL-006
until the remaining bytes, receipts, signer identity, and release argv are
actually admitted.

UX-014 S6 progress: `produce-kiron-original-assets.py` regenerated the complete
18-scene / 6-cue authored package for the current checkout. The canonical
verifier accepted `assets/kiron/manifest-v2.json` with manifest SHA-256
`feb10a215415dc8a8a392a0b35481cd7f98497d86fee77efbbe1c9c4ab417c86`; its
self-test also passed. The package remains subject to downstream RPM admission.
The source package gate then passed with
`packaging/kiron/verify-package.sh --source`, and the full static RPM payload
self-test passed. Expected-source-revision admission remains deferred until the
current working tree is frozen into the release revision.
