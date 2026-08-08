# WL-UX-013 — admitted lifecycle publication sink (2026-08-05)

`node_availability` now has a closed lifecycle-evidence publication path for
sleep, planned shutdown/reboot, maintenance, adapter migration, and explicit
return. The ledger validates the shared device-aware transition before output
and commits only after the injected sink succeeds; stale, replayed, fabricated,
missing-return, and contradictory records remain rejected.

The production sink borrows an already-open Bus `Persist` handle and an exact
absolute durable path. It encodes one bounded body, rejects parent and final
symlinks, writes through an exclusive same-directory temporary file, syncs,
atomically renames, syncs the parent, and publishes the identical bytes on the
canonical per-node health topic. A Bus failure leaves the durable record for an
exact retry while the in-memory ledger remains unchanged.

## Verification

- Farm `.50`, slot `wl-ux013-lifecycle-publication-r1`:
  `cargo test -p mackesd production_lifecycle_sink_ -- --nocapture`.
- Result: `4 passed; 0 failed; 4448 filtered out`.
- The earlier closed-evidence admission slice in the same slot passed its three
  focused lifecycle-publication tests; final file-scoped `rustfmt --check`
  passed.

## Remaining acceptance edge

The sink is not yet called by logind sleep/resume, managed shutdown/reboot,
maintenance, or NetworkManager transition producers. Those callers must supply
real node/device identity, clock, generation/event IDs, expected-return data,
the existing Bus handle, and the durable path; no lifecycle event is inferred
from absence.
