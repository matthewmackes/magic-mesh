# WL-FUNC-020 Cuttlefish contract Clippy gate — 2026-08-13

The shared Cuttlefish protocol schema-version constant was imported at module
scope even though only the existing wire-contract tests use it. Strict
all-target Clippy therefore rejected the production library target. Moving that
alias into the test module preserves the existing compatibility coverage and
removes no production behavior; no allow attribute or duplicate test was added.

## Farm evidence

- `.170`, slot `func020-clippy-repro`, before correction:
  `install-helpers/xcp-build.sh cargo clippy -p mackesd --all-targets -- -D warnings`
  failed with the line-23 unused import for
  `CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION` (exit 101).
- `.50`, slot `func020-file-fmt`:
  after `install-helpers/xcp-build.sh sync`,
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish_guest.rs`
  passed (exit 0).
- `.196`, slot `func020-compat`:
  `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::cloud::verbs::android::cuttlefish_guest::tests::observe_constructs_closed_exact_contract_and_admits_ready_vdi -- --exact --nocapture`
  passed 1/1.
- `.170`, warmed slot `func020-clippy-repro`, after correction:
  `install-helpers/xcp-build.sh cargo clippy -p mackesd --all-targets -- -D warnings`
  passed (exit 0, 1m 26s).

This restores the strict `mackesd` all-target gate needed by the substantive
WL-FUNC-020 guest packaging and release work. Real first-release artifact
production and the deferred post-release one-node acceptance remain outside
this slice.
