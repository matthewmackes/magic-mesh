# WL-FUNC-021 DLNA control-host boundary — 2026-08-10 r197

## Correction

DLNA renderer admission now refuses an absolute `controlURL` whose host differs
from the host that served the renderer's device description. This prevents a
renderer-supplied description from turning the caster into a cross-host network
pivot. Ports may differ for devices that publish description and SOAP services
on separate listeners; DNS names compare case-insensitively and IP literals
compare by parsed address.

## Farm proof

- Host: `172.20.0.90`
- Slot: `func021-dlna-control-host-r197`
- Command: `cargo test -p mde-media-core --lib cast::tests::dlna_control_authority_cannot_pivot_to_another_renderer_host -- --exact --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 262 filtered out`
- Scope: `crates/desktop/mde-media-core/src/cast.rs` only

The regression covers same-host alternate ports, case-insensitive DNS names,
canonical IPv6 equality, and cross-host rejection. No live renderer,
Chromecast CASTV2 receiver, or physical-seat cast proof was available; those
live FUNC-021 limits remain open.

The farm formatting check also reported pre-existing unrelated drift in
`crates/desktop/mde-media-core/src/roaming.rs`; that file was not changed.
