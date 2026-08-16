# WL-REL-006 RPM signing identity — staged r1

- Date: 2026-08-16 UTC
- Current checkout revision when staged: `5d5e379a6300b88400e25df4a9a02436a88c0ce5`
- Classification: operator-authorized staging evidence; not a release approval

## Identity

- Algorithm: OpenPGP Ed25519 signing key
- Public fingerprint: `06B1C27EA0E08A225155EB3314018AA1497DDC7C`
- Repository public-key path: `packaging/repo/RPM-GPG-KEY-magic-mesh`
- Private storage: private DigitalOcean Spaces bucket `saved-keys`
- Private object: `release-signing/5d5e379a6300b88400e25df4a9a02436a88c0ce5/signing-key.asc`
- Public object: `release-signing/5d5e379a6300b88400e25df4a9a02436a88c0ce5/signing-key-public.asc`

## Verification

- `gpg --batch --with-colons --show-keys --fingerprint packaging/repo/RPM-GPG-KEY-magic-mesh`
  returned the fingerprint above.
- `rclone lsl mcnf-spaces:saved-keys/release-signing/5d5e379a6300b88400e25df4a9a02436a88c0ce5/`
  returned exactly the private and public objects.
- Temporary local private-key material was securely removed after upload.
- `git diff --check`: passed.

## Admission boundary

This record does not claim that the key is a current release signer receipt.
After WL-FUNC-023 and WL-REL-006 inputs are complete, WL-REL-001 must freeze a
clean pushed revision and the canonical `produce-rpm-signing-identity-receipt.py`
must regenerate and inspect a receipt bound to that exact revision and epoch.
No RPM was signed by this staging action.
