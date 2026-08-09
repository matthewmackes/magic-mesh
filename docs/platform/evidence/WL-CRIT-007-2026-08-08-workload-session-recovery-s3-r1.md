# WL-CRIT-007 workload/session recovery S3 — 2026-08-08

The Workload reconciler now inspects every terminal attachment after daemon or
host recovery without creating a second lifecycle authority. Only the latest
valid, unexpired, exact-generation Display1 lease for a running workload may be
re-registered. Its existing identity is retained while first-frame readiness is
re-established; no new capability is minted.

Superseded, expired, mismatched, orphaned, invalid-phase, stopped-workload, and
registration-failed leases are revoked, removed from projection, and replaced
with bounded actionable status. Cleanup names the exact lease socket and cannot
remove a newer generation's active runtime. Recovery never invokes Workload
apply or cancel.

## Verification

- `.90`, slot `crit007-recovery-s3-r1`: focused `--lib` recovery gate passed
  3/3.
- Fixtures covered exact latest-generation reattachment, superseded-generation
  revocation/unpublication, mismatched refusal, and real temporary socket-file
  cleanup isolation.
- Scoped rustfmt and `git diff --check` passed.
- No operational tests were removed.

## Remaining acceptance gap

No physical reboot/suspend, real libvirt/QEMU Display1 registration, or live
first-frame seat reattachment was exercised. Fleet rollout and corrected-forward
recovery remain in S4, so CRIT-007 stays `Remaining`.
