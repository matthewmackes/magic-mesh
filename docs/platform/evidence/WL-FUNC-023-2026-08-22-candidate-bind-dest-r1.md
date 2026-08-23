# WL-FUNC-023 leftover — bind unpublished signed candidate dest — r1

Date: 2026-08-22  
Classification: dest producer; **not** live mint, **not** dest replace,
and **not** a produced candidate  
Source revision: after `bb19f37ee` (this change)  
`production_admitted: false`

Admit could read a dest-backed unpublished signed candidate, but nothing
wrote that dest. Historical 12.1.6 RPMs on the control host could have
been planted as a false candidate.

## Act

`admit-unpublished-signed-candidate.py` now requires role-correct `13.0.0`
NEVRAs (`magic-mesh-13.0.0-`, `magic-mesh-server-13.0.0-`,
`magic-mesh-lighthouse-13.0.0-`). `bind-unpublished-signed-candidate.py`
is the no-replace producer. A dest under `/root/mcnf-private` also
requires `rpm -qp` identity so fixture bytes cannot unlock mint.

No dest was written under `/root/mcnf-private`. Seat 15 was not invoked.
Existing 12.1.6 artifacts were not bound. Control-host GPG
`E6C820DAFBD1B07A` is not the governed signer
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`.

## Verification

Local helper suites (no heavy cargo):

```text
python3 install-helpers/test-admit-unpublished-signed-candidate.py
admit unpublished signed candidate hostile suite passed
python3 install-helpers/test-bind-unpublished-signed-candidate.py
bind unpublished signed candidate hostile suite passed
python3 install-helpers/test-mint-enroll-bearer.py
PASS
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after a real dest-backed unpublished signed 13.0.0 three-RPM set exists.
