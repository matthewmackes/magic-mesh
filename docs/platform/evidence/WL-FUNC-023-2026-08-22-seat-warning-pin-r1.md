# WL-FUNC-023 leftover — production mutation pins seat-update-warning — r1

Date: 2026-08-22  
Classification: leftover (3) prelude; **not** live enroll and **not** dest replace  
Source revision: after `b6e464269` (this change)  
`production_admitted: false`

`MCNF_SEAT_MUTATION_WARNING` could point production mint/dest-env mutation
at a fixture helper that exits immediately, skipping the red
`AI-GENERATED-ALERT` + 5s hold after dest admit.

## Act

`require_seat_mutation_warning(for_production_mutation=True)` now resolves
only `install-helpers/seat-update-warning.sh` and ignores the env
override. Mint and dest-env mutation use that pin. Tests resolve and
admit the live helper; the live toast was not published.

No dest was written under `/root/mcnf-private`. Seat 15 was not invoked.

## Verification

```text
python3 install-helpers/test-require-seat-mutation-warning.py
require seat mutation warning hostile suite passed
python3 install-helpers/test-mint-enroll-bearer.py
PASS
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after a real dest-backed unpublished signed 13.0.0 three-RPM set exists.
