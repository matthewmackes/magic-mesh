# WL-REL-002 / WL-REL-003 unpublished 13.0.0 cut-and-sign — r1

Date: 2026-08-22  
Classification: unpublished dest cut-and-sign; **not** final freeze,
REL-002 prepare close, REL-003 six-role resume, publication, or live enroll  
Operator authority: 2026-08-22 unpublished cut-and-sign against the S1
candidate  
`published: false`  
`production_admitted: false`  
`final_freeze: false`

## Why this SHA

`2872293b1393fdb6d645170cea30fc7d1682569d` / `1787447942` was the recorded
S1 input-generation candidate. Its isolated Maps verifier lock still pinned
path crates at `12.1.6`, so a `--locked` Fedora 44 container cut refused.

Lock refresh: commit `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac` /
epoch `1787450205` (`packaging/maps/verifier/Cargo.lock` five path-crate
versions only). Dest RPMs bind to that descendant. This is not freeze.

## Cut

Native F44 builder `172.20.0.131` had no route. Official
`run-first-full-release.sh prepare` still refuses: Maps/catalog/signer
`REPLACE_*` remain. Operator dest cut used the same container-F44 farm
lane the prepare driver calls after preflight.

| Lane | Host | Slot | Receipt |
|---|---|---|---|
| `container-rpm --full 44` | `172.20.0.130` | `unpub13-full` | `7e3474eeb` / `1787450205` |
| `container-rpm --server 44` | `172.20.0.90` | `unpub13-server` | same |

Observed image: `registry.fedoraproject.org/fedora:44` config
`sha256:6c301be52aee4facc137ea68299cc3746f2ac5c40b705d113224d3e3e22f6331`
(tag-pin warning; not digest-pinned).

| Role | NEVRA | Payload SHA-256 (unchanged by sign) |
|---|---|---|
| workstation | `magic-mesh-13.0.0-35.x86_64` | `58cba25ba57a1b2c4058882237b98f02c9122bb88ab30c2fc248bfd102f935d9` |
| server | `magic-mesh-server-13.0.0-35.x86_64` | `1d7399206b37ffebc49a5218812951d2c38104f19ee316776669512f679309ea` |
| lighthouse | `magic-mesh-lighthouse-13.0.0-11.x86_64` | `54c88f464daf4f795c07d289e52055eed9ebce2826348b7fb694071bb1919a8e` |

Workstation and lighthouse came from the full lane. Server came from the
server lane. Server-lane lighthouse was not bound.

## Sign

Governed fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C` (key id
`497ddc7c`) was imported into an ephemeral mode-0700 keyring from the
already-staged Spaces object, used once, and destroyed. The control-host
key `E6C820DAFBD1B07A` was not used. `sign-release.sh --prepare-rpms`
signed all three RPMs in one invocation. Payload digests were identical
after sign. `rpm --checksig -v` reported Header V4 EdDSA/SHA512 Signature,
key ID `497ddc7c`: OK on each role.

## Dest

`bind-unpublished-signed-candidate.py` wrote
`/root/mcnf-private/unpublished-signed-candidate.json` (0400, no-replace).
`admit-unpublished-signed-candidate.py` admitted it.
`production_admitted` remains false. No enroll, offboard, or seat mutation
ran.

## Leftover

Live FUNC-023 mint + enroll/offboard+reenroll under red
`AI-GENERATED-ALERT` + 5s. Official REL-002 prepare still needs a complete
preflight. Native F44 `.131` is still down. This dest is unpublished and
must not be published or marked production-admitted.
