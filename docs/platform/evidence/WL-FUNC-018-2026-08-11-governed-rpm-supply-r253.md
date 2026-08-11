# WL-FUNC-018 governed App-VM RPM supply — 2026-08-11

- Scope: the production App-VM image driver accepts exactly one regular,
  bounded, immutable `magic-mesh` RPM whose signature verifies in a temporary
  database containing only the committed governed key. Candidate NEVRA/payload
  metadata is consistency-only; authenticated revision authority comes from the
  exact compile-time BuildInfo in both signed `mackesd` and `mde-shell-egui` ELF
  members. A bounded non-executing parser streams those members and requires one
  `VERSION + Construct + SOURCE_COMMIT` identity in each. Verification runs on
  source and staged copies and again inside the Containerfile, with DNF
  `localpkg_gpgcheck` enabled. The repository lane uses the same local key and,
  after DNF installation, opens the root-owned installed binaries and applies
  that same bounded BuildInfo parser against the installed RPM version and
  requested source revision. Each opened ELF must also have exactly one owning
  RPM manifest row with a well-formed SHA-256 algorithm/digest that matches its
  bytes before the image layer can continue.
- Production path: governed RPM → driver admission → read-only staging →
  in-image governed-key verification → DNF signature enforcement → App-VM
  image.
- Focused local gates:
  - `packaging/app-vm/verify-contract.sh`: PASS, including unsigned,
    wrong-package, writable, oversized, symlink, multi-RPM, stale BuildInfo,
    forged-current-manifest, and pre-receipt/`nogit` refusals;
  - `verify-rpm-build-identity.py --self-test`: PASS;
  - `verify-installed-rpm-identity.sh --self-test`: PASS, including stale
    BuildInfo, duplicate/missing ownership, malformed algorithm/digest, and
    executable-byte replacement refusals;
  - shell syntax and targeted `git diff --check`: PASS.
- Farm: BigBoy (`172.20.0.130`), slot 3; shell syntax and the focused App-VM
  contract/self-test both passed with 12–13 GiB free.
- No image build was run because no current governed candidate was supplied.
- Remaining epic boundary: produce a current governed image digest, live
  boot/probe trace, and sandbox/VDI acceptance.
