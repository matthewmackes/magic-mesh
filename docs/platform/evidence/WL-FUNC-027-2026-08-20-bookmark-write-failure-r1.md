# WL-FUNC-027 evidence — bookmark persistence write failure

Date: 2026-08-20

## Real remaining gap

`FileBrowser::write_bookmarks` discarded the atomic store writer's `io::Error`.
When the configured bookmark-store parent could not be created, pinning
remained visible in memory but the operator received no warning and the pin
would disappear after restart.

## Implemented behavior

- Bookmark write failures are surfaced through `FileBrowser::last_note()`.
- A failed write keeps the bookmark dirty, allowing a later retry instead of
  falsely treating the mutation as durable.
- Successful writes clear the dirty marker as before.
- The new regression uses a regular file as the configured parent, proving the
  failure is user-visible and no store is claimed to have been created.

## Validation

- Focused regression:
  `bookmarks_report_persistence_failure_and_keep_dirty_state`
- Farm gate: passed
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3
  ./install-helpers/xcp-build.sh cargo test -p mde-files-egui --locked`
  Result: 198 passed, 0 failed, 0 ignored (2026-08-20).

