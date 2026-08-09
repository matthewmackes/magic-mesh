# WL-FUNC-020 signed Android release-artifact admission — 2026-08-09

## Bounded correction

Source baseline: `3f260a944468145626360850c0eaa5fa82fce687`.

The production Cuttlefish placement verifier previously admitted a release
artifact from caller-supplied SHA-256 values alone. Those values detected byte
substitution but did not authenticate the artifact to the MCNF release signer.

Placement schema v3 now requires a detached signature path and refuses
provisioning unless all of these conditions hold:

- the release artifact and signature are bounded, stable regular files;
- the installed project public key is present at the fixed RPM-owned path;
- that ASCII-armored key is dearmored into a private temporary keyring;
- `gpgv` reports exactly one valid signature whose primary fingerprint is the
  pinned MCNF release key `B546CC2EF9489F1899657AC9E6C820DAFBD1B07A`;
- the artifact digest is unchanged after signature verification; and
- the existing image/package/payload/architecture/compatibility bindings and
  fresh guest-tool receipt all still pass.

Missing GnuPG tooling, public key, signature, binary keyring materialization,
invalid signatures, or signer substitution produces a typed `unavailable`
release-artifact check. None can become `ready_for_provisioning`. The verifier
continues to report `live_android_guest_proof: unavailable` even after package
admission succeeds.

Fedora 44 provides `gpg` and `gpgv` in separate packages. The base and Server
RPM metadata now hard-require both `gnupg2` and `gnupg2-verify`; the Android
package contract parses the exact two Requires tables and fails if either
dependency is absent or weakened from `"*"`.

## Verification

BigBoy (`172.20.0.130`), slot `func020-signed-artifact-r102`:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=func020-signed-artifact-r102 \
install-helpers/xcp-build.sh sync

ssh mm@172.20.0.130 \
  'cd magic-mesh-farm-func020-signed-artifact-r102 && \
   bash packaging/android/verify-contract.sh'

Android/Cuttlefish packaging contract checks passed
```

The contract includes hostile missing-signature, invalid-signature, and
substituted-signer readiness cases. It also generates an ephemeral Ed25519
signer, exports an ASCII-armored public key, dearmors it, and verifies the
detached artifact signature with real `gpgv`. This proves the shipped key-format
transition rather than assuming `gpgv` accepts ASCII armor directly. Local
Python/shell syntax checks and `git diff --check` also passed.

## Remaining live gap

No signed production Cuttlefish guest-tools artifact or nested-KVM Android guest
was available. This slice therefore does not claim guest package installation,
Android boot, app inventory/launch, WebRTC attachment, display, audio/input,
reconnect, isolation, upgrade, or five-seat acceptance. Those live states remain
explicitly unavailable.
