# WL-FUNC-019 — stale desktop-roster evidence

- Date: 2026-08-10
- Farm host: `172.20.0.90` (seat `.90`)
- Farm slot: `func019-stale-roster-r153`
- Gate: `cargo test -p mackesd --lib workers::service_catalog::tests::stale_or_future_desktop_roster_cannot_revive_rdp_cards -- --nocapture`
- Result: 1 passed, 0 failed

The service catalog now withholds future-dated or older-than-five-minute retained
desktop rosters before projecting approval-gated RDP cards. Authenticated Windows
login/render remains an external deployment proof.
