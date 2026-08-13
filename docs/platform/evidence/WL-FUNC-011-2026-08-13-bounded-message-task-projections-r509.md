# WL-FUNC-011 bounded message and task projections — r509

Date: 2026-08-13

Scope: `crates/desktop/mde-collab-egui/src/messages.rs`

## Result

The Collaboration surface now searches and lays out finite newest-last windows:
512 channel messages, 256 thread replies, and 256 channel tasks. Older rows stay
in the durable collaboration history and the surface reports the exact omitted
count instead of implying that history was lost. Task-to-source lookup uses the
same bounded message window, preventing a hostile task/message projection from
creating an unbounded quadratic frame cost.

The command/event authority, durable read model, and other Collaboration
modules are unchanged.

## Farm evidence

- `172.20.0.130`, slot `func011-bounded`:
  `cargo test -p mde-collab-egui hostile_projection_searches_only_the_newest_bounded_window -- --nocapture`
  passed 1/1 with 135 filtered tests.
- `172.20.0.130`, slot `func011-bounded-clippy`:
  `cargo clippy -p mde-collab-egui --all-targets -- -D warnings` passed.
- `172.20.0.170`, slot `func011-bounded-fmt`:
  `cargo fmt -p mde-collab-egui -- --check` passed after the exact reported
  line-wrap correction.
- `git diff --check` passed.

The original Clippy attempt on `172.20.0.90` stopped making progress after its
remote process disappeared while the local SSH wrapper remained. That stale
wrapper was terminated and the same gate—not a duplicate—was rerouted to
BigBoy, where it passed.

## Remaining epic acceptance

WL-FUNC-011 still requires first-release integration and the deferred,
non-blocking post-release provider/seat proof for Calls media, cross-node
executors, native office sessions, recovery, and the six-section hard cut.
