# WL-ARCH-008 portable Browser boundary validator — 2026-08-06

`install-helpers/verify-browser-portable-boundary.py` validates the S2
portable-profile contract against disposable fixtures. It checks the
allowlisted profile/data policy, deterministic manifests, idempotent reruns,
symlink rejection, and the absence of credential-bearing stores from the
exported bundle. It invokes the migration helper self-test and never imports a
live profile.

Verification:

- Local `py_compile`, validator self-test, and source probe: passed.
- Farm `.50`, slot `browser-portable-boundary-20260806-r1`: syntax and
  validator probe passed.
- Source SHA-256:
  `46ffafa8410372a44dd4a6236729464d1e45ca1882aff050c5ce39e67257e6c7`.

Live legacy-profile inventory/import, guest-image proof, and Browser VM
acceptance remain open. Dell was not modified.
