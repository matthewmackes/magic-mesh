# WL-FUNC-021 credential file identity — 2026-08-11

- Scope: primary and additional credential files open with no-follow and
  close-on-exec semantics, require regular files, and cap input at 64 KiB.
- Hostile boundary: a final-path symlink cannot substitute provider credentials
  after restart.
- Focused gate: `cargo test -p mde-musicd creds::tests::symlink_cannot_substitute_credentials_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 10.5 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 256 filtered out.
- Remaining boundary: installed credential provisioning/rotation proof remains.
