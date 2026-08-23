# WL-FUNC-023 leftover — production candidate dest requires governed GPG — r1

Date: 2026-08-22  
Classification: dest-backed refuse; **not** live mint, **not** dest replace,
and **not** a produced candidate  
Source revision: after `2e18d88dc` (this change)  
`production_admitted: false`

`rpm --checksig` exits 0 on unsigned 12.1.6 artifacts (payload digests
only). Admit treated `signer_fingerprint` as a JSON string. Mint and the
dest-env runner honored `MCNF_UNPUBLISHED_SIGNED_CANDIDATE`, so a fixture
dest could unlock production mutation.

## Act

Production mutation now admits only
`/root/mcnf-private/unpublished-signed-candidate.json` (env override
ignored). That dest, and any dest under `/root/mcnf-private`, requires
`rpm --checksig -v` plus a GPG `Signature` line naming governed
fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C` or key id
`497ddc7c`. Bind of a production dest uses the same verify.

No dest was written under `/root/mcnf-private`. Seat 15 was not invoked.
Unsigned `magic-mesh-12.1.6-35` refuses signature verify.

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
