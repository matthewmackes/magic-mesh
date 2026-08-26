# WL-FUNC-027 source — dedicated Places bookmarks store — r1

Date: 2026-08-26  
Observed: `2026-08-26T12:05:00Z`–`2026-08-26T12:08:00Z`  
Classification: source unit; **not** operator pin/rename/reorder/remove/navigate,
**not** dest hydrate, **not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725`  
Control host: `rocky9-kvm2`  
Farm: `172.20.0.50` slot `2` (`magic-mesh-farm-2`)  
`production_admitted: false`

No seat click. No dest JSON. No Sunshine. Did not SSH Surface or Dell.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-027`.
- Unit: source-only dedicated bookmarks module; Places user section in
  `mde-files-egui` view. FolderPrefs (`model/mod.rs`), POSIX menus,
  dialogs, and menubar left to peer lanes.
- Leftover: operator pin then restart/navigate on a current-revision seat.

## Source

`crates/desktop/mde-files-egui/src/bookmarks.rs` owns the bounded store
`<config>/mcnf/files-bookmarks.json` (`XDG_CONFIG_HOME`, else
`$HOME/.config`): path validation, count cap 48, 64 KiB, pin / unpin /
rename / reorder / remove. Hostile paths, duplicates, corrupt JSON,
oversize files, and a symlinked store refuse or degrade in memory; on-disk
bytes stay until a deliberate mutation. Write failures stay dirty and
visible.

The Places sidebar user section maps those intents onto the existing
session `FileBrowser` apply path so rename dialogs and listing pin share
one list. Immediate flush after pin / unpin / reorder.

## Farm

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2
./install-helpers/xcp-build.sh cargo test -p mde-files-egui
```

Result: **221 passed**, 0 failed, 0 ignored (lib tests 2.74s after a 2m 05s
compile of `mde-files-egui`). Includes dedicated `bookmarks::tests::*`
hostile/duplicate/corrupt/cap/symlink/write-failure cases and
`view::tests::places_user_section_actions_reach_bookmark_methods` loading
the session JSON through `BookmarkStore`.

## What this does not prove

- Operator pin / rename / reorder / remove in Places on a live seat.
- Navigate-on-activation of a hydrated pin.
- GUI paint of the user Places section on current-revision DRM.

## Leftover

Still open. Closing the production leftover needs operator pin then
restart and activate on a current-revision seat. Do not invent a dest.
Do not flip `production_admitted`.
