# WL-FUNC-011 collaboration clipboard source attribution — r490

Date: 2026-08-13

## Executable slice

The collaboration Clipboard surface now canonicalizes source attribution at
both command publication and projection rendering. It trims transport-edge
whitespace, rejects blank, control-bearing, and over-128-byte identities before
they can enter a `PublishClipboard` command, and presents invalid historical
projection sources as unavailable rather than as a spoofed local or remote
actor. This is the native Collaboration lane in
`mde-collab-egui`; it does not alter the separate VDI clipboard transport.

## Farm gates

- `.130`, slot `func011-clip-source-test-r490`: focused `source_attribution`
  regressions passed 2/2 with 132 tests filtered, covering both command
  publication and projected-row attribution. The committed baseline contains an
  unrelated test-only compile error in `activity.rs` (`impl ActivityRow<'_>`
  returns `&'a ActivityEntry` without declaring `'a`). To execute this scoped
  regression without editing outside the authorized slice, the disposable farm
  workspace alone used the compiler-suggested declaration
  `impl<'a> ActivityRow<'a>`; that correction is not part of this commit.
- `.170`, slot `func011-clip-source-clippy-r490`: strict relevant library clippy
  passed with `cargo clippy -p mde-collab-egui --lib -- -D warnings`. An initial
  all-target attempt reached the same unrelated `activity.rs` test-only lifetime
  error before this file produced any diagnostic.
- `.196`, slot `func011-clip-source-fmt-r490`: file-scoped Rust 2021 rustfmt
  passed after applying its one wrapping-only correction.

## Remaining epic acceptance

WL-FUNC-011 remains open. Calls still needs a production provider and live
media lifecycle proof; Files still needs all named executors and cross-node
acknowledgement; native office sessions and hard-cut migration remain; the
FUNC-016 rich clipboard mount and post-release collaboration acceptance remain
outstanding.
