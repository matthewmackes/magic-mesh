# WL-UX-011 Surface cancellation journal and acceptance seal (r18)

Date: 2026-08-10

Implementation commits:

- `dce742e8` — seal fresh Surface acceptance evidence;
- `5deafa79` — harden Surface action cancellation recovery.

Supersedes the cancellation architecture and claims in r17. This checkpoint is
runtime/tooling evidence, not physical Surface acceptance or release promotion.

## Corrected cancellation authority

- The daemon no longer treats Bus claim rows or cancellation result rows as
  mutation or recovery authority. The Bus remains transport/presentation only.
- One root-owned Surface action journal arbitrates enable/MOK and exact-device
  firmware apply. It retains a validated directory descriptor and performs
  lock, record, rename, unlink, and fsync operations relative to that descriptor.
- The journal is root-owned `0700`, uses `0600` bounded records, rejects
  symlinks, unsafe ownership/modes, duplicate or unknown JSON fields, foreign
  Pro 5/6 identities, substituted firmware targets, oversized rows, excess
  entries, and conflicting claims.
- A cancellation intent is durable before its nonce is consumed. An action
  claim is durable before effects. Whichever exact typed claim wins first is
  retained across restart; a cancellation never interrupts an already claimed
  MOK, service, fwupd, or firmware effect.
- Recovery runs before new Bus polling and does not depend on retained Bus
  requests. `CancelClaimed` closes as exact `Cancelled`; `ActionClaimedCancel`
  closes its cancellation as exact `TooLate`; orphan `ActionClaimed` closes the
  normal action as `Interrupted`. No effect is replayed.
- Every terminal body and SHA-256 is retained until exact publication succeeds.
  A crash between a late-cancel claim and its decision synthesizes and persists
  `TooLate` before the action is closed. Publication failures remain retryable.
- Garbage collection admits only terminal, published records after the
  conservative retention window. Journal capacity exhaustion refuses new work
  and never evicts a live or ambiguous claim.
- Firmware cancellation remains an exact fresh local presentation in the
  Surface card. Enable/MOK cancellation is reachable through the governed
  root-only `cancel-surface-mok-import` wrapper, which loads only the fixed
  encrypted action credential and accepts only an exact lowercase v4 request
  UUID.

## Race-free physical collection

- The canonical collector flow is now `prepare` → operator-authorized
  `PROVE CAMERA` → `seal`. Slow inventory cannot consume the camera result's
  90-second lifetime.
- Prepared inventory is root-owned/private, no-clobber, bounded, hash-bound to
  the exact collector, node, seat, generation, timestamps, identity, artifact
  set, sizes, statuses, and SHA-256 values. Seal revalidates current DMI and
  copies through no-follow descriptors before atomic publication.
- The manifest and recorder now share the canonical twelve physical checks.
  The obsolete nine-row checklist and camera-preview wording are deliberately
  recollection-only; the camera proof remains one-frame discard with no frame,
  device id, or request id retained.

## Verification

Named farm/current-tree gates completed with no failed final test:

- root journal hostile/recovery suite on `.90`: 10/10;
- shared Surface cancellation contracts on `.90`: 17/17;
- journal-only daemon recovery on `.170`: 2/2, including the
  `ActionClaimedCancel` crash edge;
- governed MOK cancellation CLI: 6/6;
- exact firmware-card cancellation presentation: 1/1;
- focused daemon compile with `async-services`: passed;
- governed wrapper hostile self-test: passed locally and on the farm;
- collector prepare/seal hostile self-test, physical recorder self-test, and
  promotion verifier self-test: passed;
- exact formatting and `git diff --check`: passed.

Independent adversarial review first rejected the Bus-authority implementation,
then rejected incomplete descriptor anchoring, unbounded cleanup, nonterminal
GC, untyped persisted identity, Bus-dependent recovery, and a missing late
`TooLate` crash decision. The final frozen tree was re-reviewed after each
correction; the last two residual checks were green.

## Remaining hard gates

The release and physical blockers are unchanged: the matching kernel signing
key and sufficient kernel build capacity, the complete release-signed five-RPM
Surface set and real pinned Fedora 44 bootc image, an approved governed SSH
recovery key artifact, canonical Pro 6 access/deployment/MOK/reboot, all twelve
direct Pro 6 observations and recovery proofs, cross-epic audio/visual proof,
and then the same revision/package/record path on the hash-bound Pro 5.
