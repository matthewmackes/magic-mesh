# WL-REL-006 leftover — six-role inventory names Browser VM dest

Date: 2026-08-22  
Classification: inventory identity update; **not** a new digest produce,
**not** dest replace, and **not** `production_admitted`  
Source revision: after `83d1e9af5` (this change)  
`production_admitted: false`

The six-role inventory still said Browser VM had `image_digest:
leftover: no current-revision-bound digest` after the dest receipt and
Containerfile pin already existed.

## Act

`install-helpers/produce-open-source-input-inventory.py` now records the
already-produced Browser VM identity. Private dest
`/root/mcnf-private/browser-vm-base-digest.json` was **not** replaced
(mode `0400`, 563 bytes, sha256
`ac9755db790445048eb621542b69ec24220b58ecec3e056a9e570309b7c100a9`,
bound to `b30954e31` / `:44`). Containerfile pin
`quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
is named separately from the dest's `:44` reference.

| field | value |
|---|---|
| `receipt_revision` | `b30954e31` |
| `resolved_digest` | `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357` |
| dest `image_reference` | `quay.io/fedora/fedora-bootc:44` |
| leftover | dest not rebound to a later HEAD |

Fixture catalog refuse now matches the producer: any `org.example.*` app
id is a fixture. No curated refs were invented. Maps
`production_admitted` stays false.

## Verification

Local (tiny helper, no cargo):

```text
python3 install-helpers/test-produce-open-source-input-inventory.py
open-source input inventory hostile self-test: PASS
```

Leftover remains Maps `production_admitted`, real curated catalog refs,
RPM signer after freeze, S7 `REPLACE_*`, and live-seat dest
(WL-TEST-002).
