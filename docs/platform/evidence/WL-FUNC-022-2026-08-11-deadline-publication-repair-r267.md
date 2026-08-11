# WL-FUNC-022 deadline publication repair evidence — 2026-08-11

- Scope: Clock deadline advancement now recovers in-process when its atomic
  authority/audio-outbox commit succeeds but subsequent Bus publication fails.
- Boundary: the failure path reloads the durable snapshot and action cursor,
  then marks publication pending. The next sweep repairs the state publication
  and drains the already-durable audio effect without creating another
  occurrence or duplicate effect.
- Regression: `deadline_publish_failure_reloads_durable_occurrence_before_replay`
  blocks the Bus state directory after a timer becomes due, verifies revision 3,
  one ringing occurrence, and one pending audio outbox row are already durable,
  then restores Bus storage and verifies exactly one publication/effect.
- Intended farm command: `cargo test -p mackesd --features async-services workers::clock::tests::deadline_publish_failure_reloads_durable_occurrence_before_replay -- --exact --nocapture`.
- Result: **NOT RUN**. `.90` had adequate storage but both slots were occupied;
  all other free slots were below the governed 8 GiB reserve. No reserve bypass
  was attempted. Formatting and targeted `git diff --check` passed.
- Remaining proof: run the exact gate once a safe warmed slot opens, then retain
  the epic's multi-peer/live restart acceptance.
