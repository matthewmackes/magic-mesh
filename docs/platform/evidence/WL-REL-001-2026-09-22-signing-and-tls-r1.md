# Signing and TLS leftovers — 2026-09-22

Classification: drain-branch solve of cert/signing leftovers that do not
require a new dest or a second RPM GPG key. Not freeze. Not publication.
Not `production_admitted`. Not live-seat.

## Governed RPM identity

In-tree `packaging/repo/RPM-GPG-KEY-magic-mesh` fingerprint
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`. SHA-256
`39c4f65d7c7a44a8ab64e234dfa9989d1fb3f335f7e5221f619679aeb59183c9`.

Private freeze-SHA receipt
`/root/mcnf-private/rpm-signing-identity-42035dcbd.json` matches that
fingerprint, digest, revision `42035dcbd`, and epoch `1788153988`.
`install-helpers/sign-release.sh --self-test` PASS.

Control-host `gpg --list-secret-keys` has no secret. Official
`produce-rpm-signing-identity-receipt.py inspect` still needs the BigBoy
secret. XEN-BIGBOY `172.20.145.165` and `mcnf-build-52` `172.20.0.130`
are unreachable from the control host and from XEN-HOME-SERVICES
`172.20.0.9`. Do not generate a second production RPM GPG key.

## ReleaseIntentV1

`automation/promotion/release-intent.py --self-test` PASS. Unadmitted
draft written privately at
`/root/mcnf-private/release-intent-42035dcbd.json` for `42035dcbd` /
`1788153988` with credential name `MAGIC_MESH_SIGN_KEY`.
`--require-admitted` correctly REFUSED. Do not invent a signature.

## TLS / PKCS#11 lock refresh

Drain-branch `Cargo.lock` now has patched crates for the
`github-required` cargo-deny advisories that are cert/signing adjacent:

| Crate | Was | Now | Advisory |
|---|---|---|---|
| rustls | 0.23.43 | 0.23.45 | RUSTSEC-2026-0285 |
| h2 | 0.4.15 | 0.4.16 | RUSTSEC-2026-0258 |
| cryptoki | 0.12.0 | 0.12.1 | RUSTSEC-2026-0286 |

Still present and not a cert/signing leftover: yanked `chacha20` 0.9.1
(via `chacha20poly1305` 0.10.1) and `quick-xml` 0.30.0 (via `zbus_xml`
4.0.0). Those stay on `WL-REL-001` as cargo-deny leftovers.

## Not solved here

- `WL-REL-004` evidence signing and `WL-REL-005` tag / `repo_gpgcheck`
  wait on `WL-REL-003` S6.
- Surface MOK / in-tree `surface-stack.f44.json` stay dest-operator.
- Live collab identity and SIP/Vitelity dests stay `WL-TEST-003`.
