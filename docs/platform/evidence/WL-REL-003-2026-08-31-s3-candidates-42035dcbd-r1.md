# WL-REL-003 S3 freeze-SHA RPM candidate manifests — r1

Date: 2026-08-31  
Classification: unpublished dest manifests; **not** six-role plan, derivatives,
publication, or live enroll  
`published: false`  
`production_admitted: false`

## RPM candidates

Produced on `172.20.0.130` from dest-cut `42035dcbd` /
`1788153988` against `/home/mm/mcnf-signed-rpms-42035dcbd` and the governed
public key whose sha256 matches
`rpm-signing-identity-42035dcbd.json`
(`39c4f65d7c7a44a8ab64e234dfa9989d1fb3f335f7e5221f619679aeb59183c9`).
Signer `06B1C27EA0E08A225155EB3314018AA1497DDC7C`. Payload digests match
the unsigned freeze handoff.

Fedora 42 host `rpm --initdb --dbpath` refuses user lock files; produce
used a PATH wrapper that calls `rpm.TransactionSet.initDB()` then
`/usr/bin/rpm` for import/checksig. Dest-cut helpers were not edited.

| Role | Kind | NEVRA | Payload SHA-256 |
|---|---|---|---|
| workstation | `mcnf-app-vm-rpm-candidate-manifest` | `magic-mesh-13.0.0-35.x86_64` | `a20ea60c…9b27` |
| server | `mcnf-server-rpm-candidate-manifest` | `magic-mesh-server-13.0.0-35.x86_64` | `75752a5e…2b28` |
| lighthouse | `mcnf-browser-vm-lighthouse-rpm-candidate-manifest` | `magic-mesh-lighthouse-13.0.0-11.x86_64` | `feceac5c…19f7` |

Canonical dest directories:
`/home/mm/mcnf-s3-42035dcbd/{workstation,server,lighthouse}-candidate.json/`
(mode `700`, `candidate-manifest.json` `400`).

Server `reverify` and lighthouse `verify` accepted the matching signed
RPM. Cross-role substitution was not used as a pass.

## Base receipts

App VM dest receipt inspect PASS against
`quay.io/fedora/fedora@sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`
at dest-cut identity.

Browser VM base produce leftover: dest-cut bootc digest
`sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
is not a live pullable quay manifest (`manifest unknown`). Live
`quay.io/fedora/fedora-bootc:44` has moved (index sha256 prefix
`e91da1af`). Do not invent a new dest. S4 derivatives wait on that
receipt leftover.

## Leftover

S4 Browser/App VM image build, S5 bootc plan fields, S6 six-role plan
input. Native F44 `.131` remains down.
