# WL-FUNC-016 — rich-session replay capacity cleanup (r121)

Date: 2026-08-10

## Correction

Expired signed collaboration sessions retained entries in the bounded
256-identity rich-clipboard replay ledger indefinitely. A long-running seat
could therefore reject a valid new session as over capacity even though every
retained authority had expired.

The runtime now removes expired markers before each intake pass and ignores
expired retained envelopes while reconstructing replay state. A newer sequence
keeps replay expiry monotonic, so cleanup does not weaken still-live replay
protection.

## Focused farm proof

Machine 9 (`172.20.0.50`) passed the exact library regression:

```text
cargo test -p mackesd --lib \
  workers::clipboard_sync::tests::expired_rich_sessions_release_replay_capacity_before_fresh_admission \
  -- --exact --nocapture

test result: ok. 1 passed; 0 failed; 4671 filtered out
```

The regression fills the ledger with expired session identities, admits a fresh
authenticated rich envelope, and proves only the fresh replay marker remains.

## Remaining boundary

Live deployment must still prove repeated expired rich sessions release
capacity while an authenticated HTML, image, or File-backed session reaches the
supported live adapters. This checkpoint does not close WL-FUNC-016.
