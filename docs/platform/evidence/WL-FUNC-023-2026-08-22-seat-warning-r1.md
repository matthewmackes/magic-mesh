# WL-FUNC-023 leftover — production mutation runs seat-update-warning — r1

Date: 2026-08-22  
Classification: leftover (3) prelude; **not** live enroll and **not** dest replace  
Source revision: after `69b86bbae` (this change)  
`production_admitted: false`

After a dest-backed unpublished signed candidate admits, mint to
`/root/mcnf-private` and dest-env lifecycle mutation argv would have
run immediately. Leftover (3) requires red `AI-GENERATED-ALERT` + 5s.

## Act

`require-seat-mutation-warning.py` admits `seat-update-warning.sh`
(regular, executable, contains `AI-GENERATED-ALERT` and `WAIT_SECONDS=5`)
and runs it. Mint and dest-env mutation call it only after production
candidate dest admit succeeds. Tests admit the live helper and execute
fixture helpers only; the live toast was not published.

No dest was written under `/root/mcnf-private`. Seat 15 was not invoked.

## Verification

```text
install-helpers/seat-update-warning.sh --self-test
seat-update-warning: self-test passed
python3 install-helpers/test-require-seat-mutation-warning.py
require seat mutation warning hostile suite passed
python3 install-helpers/test-mint-enroll-bearer.py
PASS
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after a real dest-backed unpublished signed 13.0.0 three-RPM set exists.
