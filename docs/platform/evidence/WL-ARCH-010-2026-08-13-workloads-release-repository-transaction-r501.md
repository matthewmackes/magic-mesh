# WL-ARCH-010 — Workloads release repository transaction matrix gate (r501)

Date: 2026-08-13

## Gap closed

The canonical release matrix required generic farm CI plus workstation and
lighthouse RPM size claims, but it had no distinct, required claim proving the
Workloads compute package transaction before release. A candidate could
therefore satisfy the matrix without one revision-bound result joining the
compute RPM dependency headers and payload identity to repository install and
upgrade behavior and the ordered ownership transfer from retired
`mackesd.service` to grouped `mackesd.target`.

The matrix now requires
`farm-package-workloads-rpm-transaction`. Its sole evidence filename is bound
to the matrix source revision, and its exact contract requires all of:

1. hard Workloads compute runtime dependency headers;
2. exact compute RPM payload identity;
3. repository install and upgrade transactions; and
4. ordered retired-to-grouped daemon ownership handoff.

The verifier fails closed if that scope is absent, reordered, renamed, mapped
to another evidence file, made optional, pointed at a generic command, or
weakened to a size-only package claim. The added hostile mutation specifically
proves the last case is rejected. This does not duplicate the payload or owner
verifiers: it makes their combined release-transaction result a mandatory
promotion input.

## Farm evidence

- `.90`, slot `arch010-release-matrix-verifier-r501`:
  `python3 install-helpers/verify-release-gate-matrix.py --expected-revision
  46ea21b6f6c759fa6dcf09a28d62ba12040dc655
  install-helpers/release-gate-matrix.json` passed with 18 explicit required
  gates.
- `.170`, slot `arch010-release-matrix-hostile-r501`:
  `python3 -m py_compile install-helpers/verify-release-gate-matrix.py` passed,
  then `python3 install-helpers/verify-release-gate-matrix.py --self-test`
  passed one canonical fixture and rejected 20 hostile fixtures.
- `.196`, slot `arch010-release-matrix-json-r501`:
  `python3 -m json.tool install-helpers/release-gate-matrix.json` passed, and an
  independent JSON shape assertion found exactly one required
  `farm-package-workloads-rpm-transaction` gate with its dedicated
  revision-bound evidence filename.

All checks ran from isolated farm workspaces. No release was cut and no seat or
installed repository was mutated.

## Remaining acceptance

The first release transaction must produce the new revision-bound Workloads RPM
transaction evidence. Real libvirt/Quadlet `StartAndAttach`, native
KMS/Display1 recovery, and installed-fleet proof remain deferred and
non-blocking until after that release under the current operator direction.
