# WL-CRIT-006 — unique release-evidence claims (r155)

Date: 2026-08-10

Release-gate verification now rejects reusing one evidence artifact for
independent gate claims. The canonical matrix uses gate-specific evidence
paths, preserving claim independence.

## Verifier proof

```text
install-helpers/release-evidence.sh --self-test — passed
python3 install-helpers/verify-release-gate-matrix.py --self-test — 15 hostile fixtures rejected
canonical matrix validation — passed, 17 required gates
```

The release remains blocked until the complete signed farm/package/live bundle
is available.
