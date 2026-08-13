# WL-UX-011 — truthful input inhibition provider state (r538)

Date: 2026-08-13

## Production result

The node hardware provider no longer reports every named Linux input device as
healthy. It now reads the kernel-owned `inhibited` state for each admitted
`/sys/class/input/input*` identity and publishes:

- `Disabled` with an explicit reason when the kernel reports `inhibited=1`;
- `Ok` when a named device reports `inhibited=0`, or when the optional kernel
  attribute is absent;
- `Unknown` when an inhibition attribute exists but is malformed or unreadable;
- a bounded `inhibited: yes|no` event when the state is valid.

This is a provider-truth change only. It adds no input mutation, bypass, or
parallel control authority. Existing deterministic 256-device admission and
missing-name behavior remain intact.

## Farm evidence

- BigBoy `172.20.0.130`, slot 1 —
  `cargo clippy -p mackesd --all-targets --features async-services -- -D warnings`:
  passed in 3m47s.
- `172.20.0.90`, slot 1 —
  `cargo build -p mackesd --features async-services`: passed in 6m26s.
- `172.20.0.50`, slot 1 — focused exact library regression
  `workers::device_inventory::tests::input_provider_is_bounded_and_reports_unavailable_names_truthfully`:
  passed 1/1; 4,977 filtered out.
- `172.20.0.196`, slot 1 — Rust 1.94 exact-file `rustfmt --check` identified
  existing formatting drift elsewhere in `device_inventory.rs`; the owned input
  provider/test hunk was updated to the formatter's exact output. Package-wide
  formatting also remains red on unrelated concurrent files, which were
  preserved.
- Local scoped `git diff --check`: passed.

The first attempted exact test selector matched zero tests because it omitted
the module-qualified name. That run is not counted above; the corrected exact
selector is the recorded 1/1 result.

## Remaining UX-011 coding

- Continue production provider/control coverage for Wi-Fi, audio, display,
  printers, services, privacy, virtualization, and any still-unrepresented
  input/storage transitions.
- Continue capability/generation/audit/cancellation/recovery review for each
  concrete safe control; unresolved provider state must remain non-actionable.

## Deferred post-release proof

- Physical hardware control and stale/failure transition captures.
- Installed-package identity and credential-free fleet export inspection.
- Reduced one-node restart/rejoin/recovery acceptance.

Those live proofs remain deferred and non-blocking until after the first full
release; they are not represented as completed coding evidence here.
