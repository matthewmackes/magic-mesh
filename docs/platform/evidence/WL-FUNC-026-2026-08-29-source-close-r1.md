# WL-FUNC-026 source close — 2026-08-29

Classification: source/cargo close. Operator FileBrowser leftover remains
`WL-TEST-003` after a testing Beta.

Source revision: `5f9685408` on `agent/drain-worklist-20260725`.
`production_admitted: false`. No dest invented. No seat mutation.

## Why this closes

S1 is in-tree: `FolderPrefs` serialize to
`<config>/mcnf/files-folder-prefs.json`, hydrate at `FileBrowser`
construction, debounce writes, LRU-cap at 256, and degrade corrupt,
oversize, and symlink stores to defaults with an honest note. Restart
hydrate is covered by
`folder_prefs_survive_restart_and_hostile_files_degrade`.

## Farm (current HEAD, not re-run)

| command | job | ended | result |
|---|---|---|---|
| `cargo test -p mde-files-egui` | `143b09b89c4d` / `e3b570130ec2` | 2026-08-29T00:52:09Z | 223 passed, 0 failed |

Live leftover: `WL-FUNC-026-2026-08-26-dell-folder-prefs-r1.md`.
