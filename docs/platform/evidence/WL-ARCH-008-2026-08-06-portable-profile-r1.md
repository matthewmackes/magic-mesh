# WL-ARCH-008 portable Browser profile migration — 2026-08-06

## Boundary implemented

`install-helpers/migrate-browser-profile.py` stages a deterministic portable
bundle for the Browser VM. The profile allowlist carries bookmarks, history,
session files, and extension payloads; optional roots carry downloads and
managed policies. Symlinks, unknown profile entries, and credential-bearing
stores/names are reported as `skipped` and are never copied. The output is
private, contains relative paths only, and refuses to overwrite a different
existing bundle without explicit `--replace`.

## Verification

- Local `python3 -m py_compile install-helpers/migrate-browser-profile.py`:
  passed.
- Local `python3 install-helpers/migrate-browser-profile.py --self-test`:
  passed. The redacted fixture proves downloads survive, credential stores do
  not enter the bundle, and two consecutive migrations produce the same
  manifest.
- Farm sync and probe: BigBoy `172.20.0.130`, slot
  `browser-profile-migration-20260806-r1`; remote self-test and `py_compile`:
  passed.
- Source SHA-256:
  `f892d40c9dbe9999d3aae9f95330fefa3ca36d623ef20467df011bd6d244dbfd`.

## Remaining gap

This is source/fixture evidence only. A live legacy profile inventory and
guest import on Dell remain open; Dell was not modified.
