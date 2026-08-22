# WL-FUNC-023 leftover — mint helper refuses production dests — r1

Date: 2026-08-22  
Classification: helper refuse; **not** a production mint, **not** live
enroll, and **not** dest replace  
Source revision: after `c4b39161c` (this change)  
`production_admitted: false`

`install-helpers/mint-enroll-bearer.py` could write a 43-char bearer
under `/root/mcnf-private` if a caller pointed `--mackesd` at a live
`enroll-token`. Operator 2026-08-22: seat/ledger mutation waits on an
unpublished signed candidate. None exists.

## Act

Dest or sidecar under `/root/mcnf-private` now refuses with
`unpublished signed candidate is absent`. The helper still never prints
bearer or token bytes. Existing bootstrap dests were not replaced. Seat
15 `mackesd enroll-token` was not invoked.

## Verification

Local (tiny helper, no cargo):

```text
python3 install-helpers/test-mint-enroll-bearer.py
```

Leftover freeze bar is still a live mint through lifecycle authority and
live enroll/offboard+reenroll under red `AI-GENERATED-ALERT` + 5s after
that candidate exists.
