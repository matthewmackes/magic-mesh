# WL-CRIT-007 host snapshot restart freshness — 2026-08-11

- Scope: Host State activation tail-primes both the remote-action and local
  shell-snapshot lanes. A retained pre-restart seat snapshot cannot be
  republished as fresh or authorize a host mutation.
- Boundary: forward actions remain parked until a valid post-activation snapshot
  is successfully mirrored. Empty/malformed rows advance fail-closed; a valid
  row advances only after mirror publication, so transient Bus write failure
  retries the same observation without releasing parked actions.
- Hostile regression:
  `restart_requires_a_post_activation_snapshot_before_mirroring_or_authorizing`
  retains a stale two-display observation, injects a failed mirror write, then
  recovers and proves that stale state cannot authorize disabling the current
  last console.
- Intended farm command: `cargo test -p mackesd --features async-services --lib workers::host_state::tests::restart_requires_a_post_activation_snapshot_before_mirroring_or_authorizing -- --exact --nocapture`.
- Result: **PASS** on `.50`, slot 1 — 1 passed, 0 failed, 4,822 filtered. The
  earlier `.90` attempt rebooted during final link and produced no result;
  capacity recovery enabled this clean rerun. Targeted `git diff --check`
  passed.
- Remaining proof: restart the installed daemon with a retained seat mirror and
  perform one real post-snapshot display action.
