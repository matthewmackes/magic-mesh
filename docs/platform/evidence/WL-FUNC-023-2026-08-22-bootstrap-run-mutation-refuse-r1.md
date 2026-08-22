# WL-FUNC-023 leftover — bootstrap runner refuses lifecycle mutation argv — r1

Date: 2026-08-22  
Classification: helper refuse; **not** live enroll and **not** dest replace  
Source revision: after `a2f4c3985` (this change)  
`production_admitted: false`

`run-with-bootstrap-ssh-env.py` could still exec `mackesd enroll-token`,
`join`, `offboard`, or `mint-enroll-bearer.py` with dest identity env.
Operator 2026-08-22: seat mutation waits on an unpublished signed
candidate. None exists.

## Act

Those argv names now refuse with `unpublished signed candidate is absent`.
`/usr/bin/true` and the existing child-env fixture still run. Login env
stays unset. Bootstrap dests were not replaced. Seat 15 was not invoked.

## Verification

Local (tiny helper, no cargo):

```text
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
under red `AI-GENERATED-ALERT` + 5s after that candidate exists.
