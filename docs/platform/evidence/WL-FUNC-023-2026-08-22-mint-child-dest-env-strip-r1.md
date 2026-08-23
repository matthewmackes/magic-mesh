# WL-FUNC-023 leftover — mint enroll-token child strips dest env — r1

Date: 2026-08-22  
Classification: child-env refuse; **not** live enroll and **not** dest replace  
Source revision: after `7f8417f4d` (this change)  
`production_admitted: false`

`mint-enroll-bearer.py` copied the full process environment into
`mackesd enroll-token`. After dest-env admit, dest identity vars
(`MACKESD_BOOTSTRAP_SSH_KEY`, `MACKESD_BOOTSTRAP_KNOWN_HOSTS`) and
`JOIN_TOKEN` could leak into that mint child. Leftover (2) says only
the dest-env runner sources those vars.

## Act

`child_environment` now pops those three names before spawn. Workgroup
root is still passed as `MDE_WORKGROUP_ROOT`. Login env stays unset.
Bootstrap dests were not replaced. Seat 15 was not invoked.

## Verification

```text
python3 install-helpers/test-mint-enroll-bearer.py
PASS
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after a real dest-backed unpublished signed 13.0.0 three-RPM set exists.
