# WL-UX-012 dynamic taskbar action refusal — r545

Date: 2026-08-13

## Result

Taskbar action routing now fails closed when a dynamic remote-session or pinned-
desktop control no longer resolves to its bounded projection entry. Malformed
static controls that carry a substituted surface also emit no action. These
states previously fell through to `Home`, allowing a stale or malformed target
to trigger an unrelated shell route.

The route boundary now returns `Option<Action>` and publishes an action only for
an exact typed static control, a valid surface launcher, or a currently resolved
dynamic entry. The same refusal applies to direct taskbar controls and the More
overflow surface. No raw command path or second launcher was added.

## Farm gates

- `.50`, slot 2: `cargo build -p mde-shell-egui --all-targets` passed. The cold
  build completed in 14m03s; its only warning was pre-existing concurrent
  `mde-vdi-rdp/src/session.rs:354` dead code.
- `.50`, slot 1: the requested focused selector compiled the full test binary,
  but selected 0 tests because `--exact` was paired with the unqualified test
  name. This is recorded as insufficient test evidence and was not rerun under
  the stop cadence.
- BigBoy, slot 1: strict all-target `mde-shell-egui` Clippy reached current
  source and stopped only on the same out-of-scope concurrent
  `mde-vdi-rdp/src/session.rs:354` dead-code warning. No `nav_bar.rs` warning was
  emitted before the stop; the gate is red, not claimed as passing.
- BigBoy, slot 2: package formatting reported unrelated concurrent
  `health_modal.rs` drift and one `nav_bar.rs` line wrap. The owned wrap was
  applied exactly; the command was not rerun under the stop cadence.
- Final scoped `git diff --check` passed.

## Residual UX-012 acceptance

The pre-release code path still needs final audit of responsive Bottom/Left,
large-text, lock, multi-display, and session-switching behavior. First-release
package integration remains required. Direct-seat captures and upgrade proof
remain deferred post-release acceptance.
