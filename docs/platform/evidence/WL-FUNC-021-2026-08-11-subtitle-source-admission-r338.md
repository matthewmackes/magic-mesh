# WL-FUNC-021 subtitle source admission — 2026-08-11

- Scope: subtitle sources admit local files, local `file:///` URLs, and
  credential-free HTTP(S) only.
- Hostile boundary: ambiguous credentials/authorities, remote file paths,
  controls, whitespace ambiguity, and mpv pseudo-protocols fail closed.
- Focused gate: `cargo test -p mde-media-core subtitle::tests::ambiguous_subtitle_source_cannot_substitute_loaded_content -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 12,348,144 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 274 filtered out.
- Remaining boundary: live subtitle loading and installed-player proof remain.
