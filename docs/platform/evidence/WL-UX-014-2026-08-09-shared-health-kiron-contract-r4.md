# WL-UX-014 shared health KIRON contract — 2026-08-09 r4

## Production correction

KIRON no longer needs a shell-private A-F vocabulary. The canonical health
module now owns a bounded `HealthKironAlert` that carries the existing
`GradeLetter`, snapshot generation, condition identity, node/device identity,
and authority timestamps. Validation rejects malformed lifecycle timing,
zero-generation records, oversized duration, and secret-shaped headline or
device metadata before the shell can render it.

The shell exclusively decodes bodies marked `health_kiron`, validates the
shared record, and maps its admitted attention/dwell into the one ToastHost.
Grade, authority-derived duration, optional device, and a fixed safe Workers
deep link remain visible without health recalculation or a second queue.

UX-013 currently defines only A/B/C/D/F: A-C are capability states, D is an
active warning, and F is active critical. Grade E is therefore deliberately
unrepresentable and fails closed at deserialization. UX-014's requested E scene
and 15-second policy remain unresolved until the health authority defines an E
production state; this slice does not invent one in presentation code.

## Farm proof

- Host: machine 9 `172.20.0.50`
- Slot: `ux014-r4-20260809`
- Shared contract round-trip and hostile validation: `2 passed; 0 failed`
- Shell shared-contract mapping: `1 passed; 0 failed`
- Shell unsupported-grade-E refusal: `1 passed; 0 failed`
- Shell headless lower-third tessellation with grade/device/duration metadata:
  `1 passed; 0 failed`
- Exact-file Rustfmt and scoped diff check: passed
- `health.rs` SHA-256:
  `4581351d56f2f55174ad041333426a3ef83b38750e337455d338e7ca3655e565`
- `toast_bridge.rs` SHA-256:
  `53be5918e554d21bfb0faa33273bb92d8b8cbe88d46a9f72ae348df357eafffa`

No installed direct-DRM seat capture was taken, so authored A-F scene artwork,
audio, fallback tiers, and live visual/performance acceptance remain open.
