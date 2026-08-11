# WL-FUNC-018 Flatpak catalog replacement authority — 2026-08-11

- Scope: the App-VM front door binds launches to the currently validated Flatpak catalog generation.
- Hostile boundary: a replaced catalog cannot retain prior launch authority.
- Focused gate: `cargo test -p mackesd ipc::apps::tests::replaced_flatpak_catalog_cannot_retain_prior_launch_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live first-launch and presentation proof.
