# WL-FUNC-017 Maps hash parsing gate — 2026-08-11

## Scope

Commit `abc79e1b` hardens the two production SHA-256 chunk parsers in
`mde-maps-location-egui`. Fixed-size chunk bytes are copied into `[u8; 4]`
before conversion, removing the two `unwrap()` panic paths reported by
clippy. No unrelated files were included.

## Farm evidence

- BigBoy `.130`, slot `full-workspace`: `cargo build --workspace` — passed,
  `Finished dev profile` in 5m08s.
- `.170`, slot `func017-verify`: `cargo test -p mde-maps-location-egui --lib
  -- --nocapture` — 315 passed, 0 failed.
- `.170`, slot `func017-clippy-final`: `cargo clippy -p
  mde-maps-location-egui --lib` — passed with existing warnings only.
- `.90`, slot `crit007-clippy`: `cargo clippy -p mackesd --lib` — passed with
  existing warnings only.

The first pre-fix Maps clippy run identified exactly two `unwrap()` errors at
the two repaired production conversions; the post-fix run completed
successfully.

## Remaining acceptance

Live Maps/weather/MG90 proof remains deferred under the operator instruction
to postpone acceptance testing until after the first full build. This record
proves code-level build, test, and clippy gates only.
