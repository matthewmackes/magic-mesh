# WL-ARCH-010 live-proof typed operation migration — 2026-08-06

`verify-workloads-live-proof.py` now audits only the authoritative
`action/workload/operation` lane plus `state/workloads/<node>`. Retired VM
lifecycle and instance-roster topics, flags, fixtures, and correlation logic
were removed. Operation checks validate target node, bounded workload identity,
closed action vocabulary, deadline, freshness, and redacted capability tokens;
the existing typed projection validator remains the source of readiness truth.

Verification:

- `python3 install-helpers/verify-workloads-live-proof.py --self-test` passed.
- `git diff --check` passed; source SHA-256:
  `f2c79016fd1a5d2bcb3455d5a38af4adaacef9bfa6ddd10139d8d4d42470ad9a`.
- A live proof was not claimed: the local checkout has no live mackesd/Bus
  evidence. Dell runtime was not modified.

This removes the verifier's retired authority dependency; live Workload
restart/CAS, native attachment, package, and seat acceptance remain open.
