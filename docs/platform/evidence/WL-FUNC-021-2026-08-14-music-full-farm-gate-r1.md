# WL-FUNC-021 — full Music daemon farm gate

- Date: 2026-08-14
- Revision: `6cf57331`
- Farm: BigBoy `172.20.0.130`, slot `music-full-audit`
- Command: `cargo test -p mde-musicd --lib`
- Result: 274 passed, 0 failed, 0 ignored

The full Music daemon suite passed across Airsonic/provider admission,
catalog and source selection, typed mutations, Clock audio, engine fallback,
renderer generation, handoff, queue durability, MPRIS, cache, credentials,
and seat audio. The run also exposed a parallel-test race: the cover-art test
mutated process `HOME`, causing provider mutation tests to observe a different
cache root. The test now injects its artwork root without changing `HOME`.

Installed renderer/provider/package evidence remains under `WL-TEST-001` and
does not impose a second-seat requirement on the Music epic.
