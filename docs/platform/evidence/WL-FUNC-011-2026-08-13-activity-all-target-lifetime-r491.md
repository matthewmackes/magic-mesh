# WL-FUNC-011 Activity all-target lifetime repair — r491

Date: 2026-08-13

## Defect and repair

The committed Collaboration Activity test boundary did not compile under the
crate's all-target gate. `ActivityRow<'_>` exposed a test-only `entry` accessor
whose return type named an undeclared `'a` lifetime. The production library
therefore compiled while the library-test target failed with `E0261`.

The accessor now returns `&ActivityEntry` with lifetime elision tying the borrow
to `&self`, which in turn cannot outlive the row's borrowed projection entry.
Existing equality coverage remains intact. The neighboring test-only
`ActivityRows` impl also uses the same elided-lifetime form required by the
crate's strict `clippy::elidable_lifetime_names` policy; no lint was suppressed
and no test was removed.

## Farm evidence

- `.90`, slot `func011-activity-all-target-repro-r491`: unchanged baseline
  `cargo clippy -p mde-collab-egui --all-targets -- -D warnings` failed at
  `activity.rs:56` with `E0261`, proving the reported committed defect.
- `.90`, slot `func011-activity-all-target-r491`: the first explicit-lifetime
  repair advanced beyond `E0261` and was rejected by the strict
  `clippy::elidable_lifetime_names` policy. This established that suppressing
  the lint or retaining explicit impl lifetimes was not an acceptable repair.
- `.90`, slot `func011-activity-all-target-r491b`: the exact all-target clippy
  gate passed after the elided, self-borrowed boundary repair.
- BigBoy `.130`, slot `func011-activity-focused-r491b`:
  `cargo test -p mde-collab-egui activity::tests -- --nocapture` passed 3/3
  focused tests with 131 filtered out.
- `.50`, slot `func011-activity-fmt-r491b`:
  `cargo fmt -p mde-collab-egui -- --check` passed. A direct file-scoped
  `rustfmt --edition 2021 --check` also passed in the earlier synced `.50`
  workspace.

An earlier attempt to pass a source path through workspace `cargo fmt` checked
unrelated workspace files and reported existing drift. It made no changes and
is not counted as a gate.

## Remaining epic acceptance

This removes the real all-target compilation defect exposed by the clipboard
source-attribution slice. WL-FUNC-011 remains open for its substantive remaining
Calls provider/runtime work, named Files executors and cross-node
acknowledgement, native office-session hard cutover, and deferred post-release
live collaboration proof.
