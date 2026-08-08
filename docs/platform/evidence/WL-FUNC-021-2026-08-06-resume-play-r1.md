# WL-FUNC-021 — authenticated Music Resume playback (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
live provider, engine, seat, DLNA, and full Bus-parity acceptance are still
open.

## Goal

Close the daemon-backed Resume path for typed Episode, Chapter, and Audiobook
bookmark rows without creating a second queue authority or allowing the
standalone Music binary to mint a mutation token.

## Implementation

- `crates/services/mde-musicd/src/queue.rs` adds the queue-authority
  `select_or_enqueue` seam. It selects an existing entry or appends and selects
  a new typed resume entry atomically, preserving the cursor/list invariant.
- `crates/services/mde-musicd/src/bus_responder.rs` now admits typed bookmark
  audio Resume requests through the existing selected-source candidate and
  engine path. The queue is cloned and committed only after engine admission;
  an unavailable engine or failed start cannot leave a partial queue mutation.
  Ordinary non-current Music rows retain the stricter `content_not_current`
  behavior.
- `crates/desktop/mde-music-egui/src/app.rs` emits a validated unsigned
  `MusicActionRequestV1` containing the bookmark's source-qualified content
  kind and millisecond position. The Resume affordance is disabled in the
  standalone app and reports that the authenticated Construct shell is
  required.
- `crates/desktop/mde-shell-egui/src/main.rs` installs the shell publisher.
  The shell signs the body with the existing root `music-workspace` authority
  and persists it on `action/music/workspace`; missing Bus state fails closed.

No armed token is retained in the Music UI, and no second executor, Bus writer,
bookmark store, or provider session owner was introduced.

## Farm verification

- `.90`, slot `music-resume-full-r1`: `cargo test -p mde-musicd` — `167 passed,
  0 failed`; doctests `0 passed, 0 failed`.
- `.50`, slot `music-resume-ui-r2`: `cargo test -p mde-music-egui` — `39 passed,
  0 failed`; this includes the typed Resume source-kind/position regression.
- `.90`, slot `music-resume-shell-r2`: the full `mde-shell-egui`
  `--no-default-features` build compiled and ran `1451` tests; `1439 passed`
  and `12 failed` in pre-existing Car pixel, shell catalog, IAC fixture,
  navigation, surface-taxonomy, and switcher tests outside this Music slice.
  The focused Music mount test
  `shell_mounts_and_renders_the_media_surface` passed `1/1`.
- `.50`, slot `music-resume-format-r1`: the repository-wide `cargo fmt --all
  -- --check` reports broad pre-existing drift across unrelated dirty files;
  it also reported existing file-wide drift in the touched large modules. No
  bulk formatter rewrite was applied. `git diff --check` passed locally.
- BigBoy `.130` was down/unreachable for this slice; no BigBoy result is
  claimed.

## Remaining proof

This is fixture-backed farm evidence, not live playback. Live provider decode,
network-loss cache playback, audible engine proof, live seat handoff, DLNA
session/control, UI removal of the legacy worker after full Bus parity, Dell
seat acceptance, and release/RPM gates remain required before WL-FUNC-021 or
the broader drain goal can close.
