# WL-FUNC-016 S4 bounded VDI clipboard transport — 2026-08-08

Clipboard V2 now has a typed VDI lease, message, and payload-free receipt
boundary. Admission binds exact session/generation/lease identity and expiry,
negotiates only mutually supported MIME, rejects secret-bearing or unsupported
representations, and keeps cross-lease replay high-water marks without storing
clipboard bytes or host paths.

The live-VNC shell adapter publishes and rotates leases, uses typed host/guest
lanes, persists delivery receipts, and materializes only truthful text support
for the current VNC protocol. Rich MIME remains negotiated and explicitly
unsupported where the concrete guest protocol cannot carry it; reconnect does
not silently duplicate a delivered payload.

The live RDP adapter now carries bounded Unicode text in both directions over
CLIPRDR. The live SPICE adapter does the same through a demand-driven vdagent
`GRAB -> REQUEST -> CLIPBOARD` exchange after negotiating both clipboard
capabilities. Host-to-guest and guest-to-host traffic consume one-use permission
tickets and fail closed on missing controller, stale lease, excess size, replay,
disconnect, or absent protocol capability. SPICE agent messages are fragmented
at 2 KiB and reassembled under a 1 MiB payload limit. Unsupported MIME remains
explicit rather than advertising false capability.

## Verification

BigBoy `.130`, slot `func016-vdi-rich-r1`:

- `cargo test --locked -p mackes-mesh-types vdi_transport -- --nocapture`:
  4/4 passed.
- `cargo test --locked -p mde-shell-egui --features live-vdi vnc_host_clipboard -- --nocapture`:
  2/2 passed.
- `cargo test --locked -p mde-shell-egui --features live-vdi vnc_clipboard_event_is_canonical_and_session_attributed -- --nocapture`:
  1/1 passed.
- Scoped rustfmt and `git diff --check` passed.
- `.170`: shell clipboard/permission passed 21/21 and RDP CLIPRDR passed 3/3.
- `.170`, slot `func016-spice-clipboard-s4-r1`: SPICE vdagent
  protocol/reconnect passed 2/2, fragmentation passed 1/1, capability status
  passed 1/1, and the shipped `live-vdi` binary check passed.
- `.170`, slot `func016-spice-package-r1`: complete `mde-vdi-spice` unit,
  parser, and loopback suites passed 51 tests; the hardware-only live guest case
  remained explicitly ignored because no SPICE fixture was configured.
- `.170`, slot `func016-rdp-package-r1`: complete `mde-vdi-rdp` unit and
  loopback suites passed 78/78.

## Remaining acceptance gap

No live guest hardware fixture was exercised. Non-text materialization, CAS
cleanup, package policy, and five-seat proof remain; FUNC-016 stays `Remaining`.
