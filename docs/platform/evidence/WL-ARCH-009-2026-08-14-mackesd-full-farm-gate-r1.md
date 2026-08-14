# WL-ARCH-009 — full mackesd farm gate

- Date: 2026-08-14
- Revision: `be5c3006`
- Farm: BigBoy `172.20.0.130`, slot `mackesd-full-audit`
- Command: `cargo test -p mackesd --lib`
- Result: 4,999 passed, 0 failed, 0 ignored

The complete mackesd library suite passed, including worker, Bus, authority,
Clock, peer, recovery, file, host-operation, mesh-transfer, and service
account tests. This is implementation evidence only; installed package and
operator-supplied release inputs remain under `WL-TEST-001`.
