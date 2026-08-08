# WL-FUNC-021 — Music mutation authorization delivery (2026-08-06)

Status: implementation and farm-test evidence; live authorized mutation,
rotation, and installed-seat acceptance remain open.

## Delivered boundary

The Music lane now has a dedicated asymmetric contract in
`crates/mesh/mackes-mesh-types/src/music_auth.rs`:

- domain-separated Ed25519 signatures use key id `music-action-ed25519-v1`;
- the signature binds the exact canonical request digest, verb, node, target,
  nonce, and 30-second expiry;
- the verifier bounds nonce/signature/digest fields and rejects wrong scope,
  wrong key, body tampering, expiry, and replay;
- the legacy HMAC path remains available only for its existing compatibility
  tests and non-Music lanes.

The root DRM shell signs `music-workspace` actions using the dedicated
`music-action-private-key` systemd credential. `mde-musicd` loads only
`/etc/mde/music-action-public-key`, rejecting non-regular, oversized, malformed,
or non-absolute configured paths. The separate provisioning helper derives the
public key, encrypts the seed with `systemd-creds`, installs the root-shell
drop-in, and reconciles rotation without copying private bytes to the user
unit, Bus, catalog, or logs.

## Verification

- `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-auth-tests-20260806-r2 ./install-helpers/xcp-build.sh cargo test --locked -p mackes-mesh-types -p mde-musicd --lib` passed **431/431** shared-type tests and **174/174** daemon tests.
- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-auth-check-20260806-r2 ./install-helpers/xcp-build.sh cargo check -p mackes-mesh-types -p mde-musicd -p mde-shell-egui` passed.
- `install-helpers/provision-music-action-credential.sh --self-test` passed;
  `bash -n` passed for the helper and both loopback helpers.
- `git diff --check` passed.
- The expiry boundary was hardened after this audit: both legacy HMAC and
  Music Ed25519 capabilities now reject `now >= expires_at`, and replay-claim
  cleanup removes entries at the same boundary. Farm `.90` passed the four
  focused authorization regressions.

The farm-wide `cargo fmt --all -- --check` remains red on unrelated existing
dirty files, including older unformatted shell/mesh sources. The new helper
and source changes were inspected and the shared auth module is formatter-safe;
the formatter infrastructure also lost `.170` to a full filesystem during a
parallel attempt.

## Remaining acceptance

No credential was provisioned on Dell or seat 15, no service was restarted,
and no private seed was collected. Live authorized mutation delivery, wrong-key
rotation, and package-installed public-key verification still require a
controlled operator-owned seat test.
