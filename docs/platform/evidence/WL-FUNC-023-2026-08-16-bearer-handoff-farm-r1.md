# WL-FUNC-023 bootstrap bearer-handoff evidence — 2026-08-16

- Source revision: `35c29d75b3d7432c0170917d87b3d15112cc73a8`
- Farm host: `172.20.0.170`
- Farm slot: `wl-func023-bearer-handoff-20260816`
- Commands:
  - `cargo test -p mackesd ssh_bootstrap --locked -- --nocapture`
  - `cargo test -p mackesd push_enroll_drives_the_bootstrap --locked -- --nocapture`
- Results: `3 passed, 0 failed`; then `1 passed, 0 failed`.

The endpoint now carries the minted bearer separately from the rendered
`mackesd join --role lighthouse {{JOIN_TOKEN}}` command template. Push-enroll
refuses when the bearer is absent and the focused wiring test proves the typed
`RunEnroll` action receives the bearer value rather than the template. Actual
token minting, live SSH execution, and live target acceptance remain open.
