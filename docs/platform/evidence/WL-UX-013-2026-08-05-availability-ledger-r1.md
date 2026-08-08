# WL-UX-013 — daemon availability-intent ledger (2026-08-05)

The daemon now has a bounded admission seam for
`NodeAvailabilityIntent`. It keeps one latest intent per node in deterministic
order, retains a bounded event-id replay window, validates currentness and
lifecycle transitions through the shared health contract, rejects stale,
replayed, contradictory, and over-capacity records, and preserves explicit
`Unknown` rather than inferring absence. It performs no sleep, reboot, network,
or other live side effect.

## Verification

- BigBoy `.130`: focused daemon availability module gate passed,
  `5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- File-scoped Rust formatting passed.
- The full daemon gate currently stops at the pre-existing/unrelated
  `TransferPolicy` error in `workers/transfers/v2.rs:222`; this slice does not
  claim lifecycle publication, escalation policy, or live fleet proof.
