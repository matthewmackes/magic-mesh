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
- Result: **PASS**. Farm `.130`, slot `func022-deadline-r3`, ran the exact
  regression after a clean full test-profile compilation: 1 passed, 0 failed;
  all other test binaries were filtered. Warnings were emitted, but no build or
  test error occurred.
- Remaining proof: retain the epic's multi-peer/live restart acceptance.
