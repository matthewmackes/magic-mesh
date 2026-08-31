# WL-REL-003 S1–S2 freeze-SHA RPM self-sign — r1

Date: 2026-08-31  
Classification: unpublished freeze-SHA signed RPMs; **not** six-role resume,
evidence envelope, publication, or live enroll  
Operator authority: 2026-08-31 generate-keys  
`published: false`  
`production_admitted: false`

## Signer

Governed fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C` (key id
`497ddc7c`) was imported from the already-present BigBoy secret into an
ephemeral mode-0700 keyring, used once, and destroyed. A second production
RPM GPG key was **not** generated. Control-host key `E6C820DAFBD1B07A`
was not used. `rpm-sign-4.20.1-1.fc42` was installed on `172.20.0.130`
before mutation.

Surface MOK: a new operator dest pair was generated under
`/root/mcnf-private/surface-mok-operator-20260831` (0400, not in Git). It
does **not** replace freeze-tree
`packaging/surface/mcnf-dev-snapshot-surface.cer`. Surface five-RPM stack
stays blocked.

## Inputs

Unsigned handoff `/home/mm/mcnf-unsigned-handoff-42035dcbd` was copied
(not mutated) into `/home/mm/mcnf-signed-rpms-42035dcbd`. Pre-sign NEVRA
and payload digests matched `handoff.json` exactly.

## Sign

`MAGIC_MESH_SIGN_KEY=06B1C27EA0E08A225155EB3314018AA1497DDC7C`
`install-helpers/sign-release.sh --prepare-rpms` signed all three RPMs in
one invocation from the dest-cut checkout. Payload digests were identical
after sign. `rpm --checksig -v` reported Header V4 EdDSA/SHA512 Signature,
key ID `497ddc7c`: OK on each role.

| Role | Signed file | NEVRA | Payload SHA-256 (unchanged) |
|---|---|---|---|
| workstation | `workstation.rpm` | `magic-mesh-13.0.0-35.x86_64` | `a20ea60cb5603600c8cce5264dfc6623af5eb426651f6a03a820c2e59a019b27` |
| server | `server.rpm` | `magic-mesh-server-13.0.0-35.x86_64` | `75752a5e1b94ce18f7da031c754f23f3503598e03d8b4a3a0a3b64b2c7d42b28` |
| lighthouse | `lighthouse.rpm` | `magic-mesh-lighthouse-13.0.0-11.x86_64` | `feceac5c59ca9297c75e41b21c8c3bcfec4180b598790eac50a14517219d19f7` |

Signed files mode `400`. Ephemeral `~/.gnupg-sign-*` directories are gone.

## Leftover

`WL-REL-003` S3–S6 (candidate manifests, Browser/App VM derivatives, bootc
plan input) remain. Do not tag, publish, or mark `production_admitted`.
