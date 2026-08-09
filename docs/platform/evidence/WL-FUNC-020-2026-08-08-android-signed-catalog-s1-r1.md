# WL-FUNC-020 signed Android catalog contract — 2026-08-08

The Android contract now admits an Ed25519-signed, time-bounded catalog that
binds the closed AOSP starter-app set to exact package versions, an immutable
image digest and provenance, closed permissions/capabilities, bounded outer-VM
resources, and explicit guest-readiness requirements. Stable topic helpers and
content digests reject unsafe identities, stale/untrusted/tampered envelopes,
reordered or duplicate policy, incompatible packages, and excess resources.

A default runtime worker consumes the node-scoped import action, verifies the
locally provisioned signer identity/public key, accepts only increasing
revisions, atomically persists the bounded last-good catalog, republishes it
after restart, and never embeds private key material.

## Verification

- BigBoy focused signed-catalog proof passed 2/2.
- BigBoy full `mackes-mesh-types` library passed 465/465.
- BigBoy runtime import, hostile-input retention, and restart/corruption proof
  passed 3/3 in slot `func020-android-catalog-runtime-r1`.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Production release-key provisioning/rotation and a shipped signed catalog/image
artifact remain. FUNC-020 S1 and the epic stay `Remaining`.
