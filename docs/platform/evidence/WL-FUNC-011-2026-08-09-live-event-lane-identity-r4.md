# WL-FUNC-011 live event lane identity binding — 2026-08-09

The collaboration daemon now requires every valid signed live event to match
the exact `collab/event/<space>/<actor>` Bus lane that carried it. A signature
alone can no longer admit an envelope routed under a different actor or space;
both forms fail closed before merge, while the same envelope remains admissible
on its canonical lane.

## Farm proof

- Host: machine 193 (`172.20.0.90`)
- Slot: `func011-r4-20260809`
- Focused hostile regression:
  `valid_signed_event_requires_exact_space_and_actor_lane_identity`
- Result: 1 passed, 0 failed, 4,362 filtered out.
- Exact-file `rustfmt --check`: passed.
- `collab.rs` SHA-256:
  `565b052a32420fb9cdd0f99e307a6aeeedf8e6707e80e5a03350d87d060f3195`

This closes one live-ingress identity/replay gap. Real cross-node Bus and
Syncthing replay, restart convergence, media providers, office transport,
migration, and five-seat release evidence remain open.
