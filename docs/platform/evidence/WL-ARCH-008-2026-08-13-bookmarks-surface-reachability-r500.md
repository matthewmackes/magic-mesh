# WL-ARCH-008 — Browser bookmarks surface reachability (r500)

Date: 2026-08-13

## Finding

The full static RPM payload diagnostic reported `mde-bookmarks-egui` as
`built-but-dead`: the crate was a workspace member and a direct dependency of
the shipped `mde-shell-egui` binary, but it was absent from the shell's
authoritative `EMBEDDED_SURFACE_CRATES` catalog.

The crate is not dead code. `mde-shell-egui/src/web/mod.rs` owns a
`BookmarksManager` and `BookmarksBus`; the production Browser render path pumps
the Bus and renders `bookmarks_panel` under the `browser-bookmarks` identity.
Removing it would delete reachable Browser behavior. The correction records
that existing production mount in the authoritative embedded-surface catalog
and adds a regression naming the live `web::web_panel` boundary.

## Farm evidence

- `.90`, `bookmarks-repro-r500`: before the change,
  `install-helpers/verify-rpm-payload.sh all` failed with two surface errors,
  including `mde-bookmarks-egui built-but-dead: NOT-mounted`.
- `.90`, `bookmarks-all-r500`: after the change, the same complete diagnostic
  reports `mde-bookmarks-egui mounted in surfaces.rs AND compiled into the
  shipped shell`. That intermediate run exited non-zero solely for
  `mde-panel-egui`; the failure count fell from two to one.
- The `mde-panel-egui` path was then inspected directly. It contained only
  untracked empty `src/` and parent directories: no `Cargo.toml`, source,
  workspace member, shell dependency, Git history entry, or executable surface.
  The directory-name scanner had classified rsynced local residue as a crate.
  The empty directories were removed rather than adding a false dependency,
  catalog entry, or policy exemption.
- `.90`, `surface-all-final-r500`: the exact complete
  `install-helpers/verify-rpm-payload.sh all` diagnostic passed all payload and
  surface checks. Bookmarks remains explicitly mounted and no Panel crate is
  enumerated.
- `.130`, `bookmarks-catalog-test-r500`: the fully qualified catalog regression
  passed 1/1 (`1,577` filtered).
- `.50`, `bookmarks-browser-render-r500`: the fully qualified production
  Browser render-path regression passed 1/1 (`1,577` filtered), proving the
  Browser frame renders the bookmark manager at the guest boundary.
- `.90`, `bookmarks-crate-tests-r500`: the complete `mde-bookmarks-egui` suite
  passed 41/41; doc tests passed.
- `.196`, `bookmarks-crate-clippy-r500`: strict all-target Bookmarks clippy
  passed.
- `.170`, `bookmarks-shell-clippy-r500`: shell-wide strict all-target clippy
  reached the changed catalog and then failed on three pre-existing out-of-scope
  test lints in `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`; no
  Bookmarks diagnostic was emitted.
- `.196`, `bookmarks-shell-fmt-r500`: `cargo fmt -p mde-shell-egui -- --check`
  passed.

No worker or packaging-helper source was changed. The remaining panel failure
is not masked or allowlisted by this slice.
