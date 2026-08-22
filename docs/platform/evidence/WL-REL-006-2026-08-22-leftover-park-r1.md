# WL-REL-006 leftover park — remaining inputs cannot proceed — r1

Date: 2026-08-22  
Classification: leftover-honesty / park; **not** production admission,
preflight close, or dest replace  
Source revision: after `a48b00be0` (this change)  
`production_admitted: false`

In-tree REL-006 compile leftovers that can be recorded without freeze,
invented Flatpak refs, or dest replace are done. Remaining demand is
blocked on operator/freeze secrets.

## Inventory pin (this change)

`produce-open-source-input-inventory.py` now names the already-admitted
Containerfile pin
`quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
on App VM and bootc, matching Browser VM. Dest receipts were **not**
replaced.

Surface `packaging/surface/surface-stack.f44.json` `bootc_base` stays
`null`. `verify-surface-stack.sh` refuses a guessed base on a blocked
manifest (`blocked manifest must not guess a base digest or signing key`).

Local: `python3 install-helpers/test-produce-open-source-input-inventory.py`
→ PASS.

## Why the leftover is parked

| Leftover | Why parked |
|---|---|
| Maps `production_admitted` | `bind_receipt` forces false until freeze / real candidate-bound provider object |
| Real curated Flatpak refs | No operator-approved digest-pinned refs; `org.example.*` is refused |
| RPM signer receipt | Governed secret `06B1C27EA0E08A225155EB3314018AA1497DDC7C` is not on the control host; waits on WL-REL-001 freeze |
| S7 `REPLACE_*` | Combining existing dests would be cross-revision; template not overwritten |
| Live-seat dest | WL-TEST-002 Blocked |
| Surface `bootc_base` | Must stay null while the stack manifest is blocked |

Unblock: FUNC-023 live enroll so freeze can proceed, then REL-001 / REL-002,
operator-approved catalog refs, and the RPM signer secret. Do not invent
refs. Do not flip `production_admitted`. Do not guess Surface `bootc_base`.
