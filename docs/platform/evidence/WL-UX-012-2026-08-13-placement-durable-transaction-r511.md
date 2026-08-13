# WL-UX-012 taskbar placement durable transaction — r511

Date: 2026-08-13

Commit: pending at gate time

## Result

The Construct taskbar now persists the candidate Bottom/Left placement before
publishing that placement to layout or starting its transition. A failed
preference write leaves both the current display ownership and transition
unchanged, preventing the running shell from advertising a content edge that a
replacement shell cannot restore. The successful path durably retains the
exact ordered pin projection alongside placement and restores it without
restoring transient animation state.

## Farm evidence

- `.130` BigBoy, slot `ux012-placement-test`: focused hostile regression
  `nav_bar::tests::placement_publishes_only_after_exact_preferences_are_durable`
  passed 1/1, with 1,595 tests filtered out.
- `.196`, slot `ux012-placement-clippy2`: strict production-binary Clippy
  (`cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`) passed.
- `git diff --check`: passed.

An initial Clippy command requested a nonexistent `production` Cargo feature and
exited before compilation; the established production-binary command above is
the accepted gate. An initial exact test filter selected zero tests because it
omitted the module path; the accepted focused invocation above executed 1/1.
A package formatting probe reported unrelated pre-existing workspace drift;
only the formatter's changes for `nav_bar.rs` were applied and concurrent files
were left untouched.

## Remaining acceptance

First-release package verification remains. Per operator direction, installed
responsive proof is deferred and non-blocking until after that release: one
available seat must cover Bottom/Left, Dark/Light, large text, lock,
multi-display/session switching, upgrade restoration, and direct visual review
without clipping, overlap, duplicate launcher, or focus loss.
