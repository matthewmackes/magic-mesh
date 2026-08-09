# WL-UX-013 S4 — redacted health support export (r11)

Date: 2026-08-09

Base commit: `961589a0452c8c7e80fae5f54c331bdad8f526b6`

Source SHA-256: `120ef924b9e2d2d87635300a44a365c2be5e5e12d1968aba72518e9015cd1c9e`

## Delivered behavior

- The Health modal exposes a visible **Export redacted support bundle** action when a current snapshot exists and preserves an honest success path or bounded failure message in modal-local state.
- The explicit click handler is the only render path that performs filesystem I/O.
- `mde.health.support-bundle.v1` JSON deterministically includes snapshot generation/time, bounded node grades and factors, mesh summary, active conditions, and resolved 24-hour history.
- Output is capped at 64 KiB; node, active, resolved, fact, filename, and text collections/fields have independent ceilings. Bounded top-N selection never clones an entire hostile snapshot collection. Credential-shaped fields/material, raw authorization, private-key markers, and path-shaped values are omitted or replaced.
- Production resolves one fixed `<account-home>/.local/share/mde/health-support` directory from the current UID's bounded `/etc/passwd` record. It does not trust `XDG_DATA_HOME`, `HOME`, or a temporary-directory fallback.
- Every directory component is walked relative to a directory descriptor with `O_DIRECTORY|O_NOFOLLOW`; missing components are mode 0700 and their parent entry is synced. The final directory must be owned by the current UID and have no group/other permissions.
- Each write uses a random mode-0600 `O_EXCL|O_NOFOLLOW` temporary, writes and syncs the file, renames within the anchored parent, then syncs the parent directory. Post-create failures remove only the temporary created by that attempt. Existing destination symlinks, directory symlinks, unsafe filenames, and preplanted temporary collisions fail closed.

## Focused farm verification

Host: machine 196 (`172.20.0.196`)

Slot: `health-support-export-r11`

Exact format command:

```text
rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/health_modal.rs
```

Result: passed.

Exact test commands:

```text
cargo test -p mde-shell-egui health_modal::tests::support_bundle_is_deterministic_byte_bounded_and_redacts_hostile_material -- --exact
cargo test -p mde-shell-egui health_modal::tests::support_bundle_writer_rejects_escape_and_filename_is_sanitized -- --exact
cargo test -p mde-shell-egui health_modal::tests::support_bundle_rejects_symlinked_directory_and_destination -- --exact
cargo test -p mde-shell-egui health_modal::tests::support_bundle_exclusive_temp_collision_is_not_followed_or_removed -- --exact
cargo test -p mde-shell-egui health_modal::tests::support_bundle_export_writes_atomic_round_trip_json -- --exact
cargo test -p mde-shell-egui health_modal::tests::support_bundle_write_failure_is_preserved_in_modal_state -- --exact
cargo test -p mde-shell-egui health_modal::tests::zero_state_and_escape_are_rendered_and_functional -- --exact
```

Results: each command ran one exact test and passed (`1 passed; 0 failed; 1,513 filtered out`). No broad test or workspace gate ran.

After review replaced whole-collection cloning with bounded top-N selection,
BigBoy (`172.20.0.130`) slot `health-topn-r17` reran the hostile deterministic,
byte-bound, and redaction test: `1 passed; 0 failed; 1,513 filtered out`. The
single-file format check passed again.

Scoped `git diff --check` passed. No commit was created.
