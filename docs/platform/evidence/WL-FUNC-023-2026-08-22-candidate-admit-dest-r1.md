# WL-FUNC-023 leftover — dest-backed unpublished-candidate admit — r1

Date: 2026-08-22  
Classification: dest-backed refuse; **not** live mint, **not** dest replace,
and **not** a produced candidate  
Source revision: after `90f98dbf5` (this change)  
`production_admitted: false`

`mint-enroll-bearer.py` and `run-with-bootstrap-ssh-env.py` hardcoded
"unpublished signed candidate is absent". Leftover (1)/(3) would stay
refuse-closed even after a real dest appeared.

## Act

`admit-unpublished-signed-candidate.py` reads dest
`/root/mcnf-private/unpublished-signed-candidate.json` (override
`MCNF_UNPUBLISHED_SIGNED_CANDIDATE`). Missing dest still refuses with
that phrase. A present dest must name exactly three unpublished RPMs
(workstation/server/lighthouse), matching sha256, governed signer
fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`,
`published: false`, and `production_admitted: false`.

Mint production dests and dest-env mutation argv now call that admit.
No dest was written under `/root/mcnf-private`. Seat 15 was not invoked.
Fixture RPM bytes are not a production candidate.

## Verification

Local helper suites (no heavy cargo):

```text
python3 install-helpers/test-admit-unpublished-signed-candidate.py
admit unpublished signed candidate hostile suite passed
python3 install-helpers/test-mint-enroll-bearer.py
PASS
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after a real dest-backed unpublished signed candidate exists.
