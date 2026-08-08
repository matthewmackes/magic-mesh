# WL-FUNC-021 — Live-seat package payload guard (2026-08-06)

`verify-music-live-seat.sh` now checks, without secrets, that the installed
`magic-mesh` RPM owns both `/usr/bin/mde-musicd` and `/usr/bin/mde-shell-egui`.
Self-test includes positive and missing-payload fixtures. Source SHA-256:

```text
288fada42e5fa55e9b89024989f59d1e0b2e28465148a7b9d458d99eb9cfcc5f  install-helpers/verify-music-live-seat.sh
```

Validation:

```text
bash -n install-helpers/verify-music-live-seat.sh
./install-helpers/verify-music-live-seat.sh --self-test
```

Result: **self-test passed**; the bounded read-only default seat probe also
passed. This does not claim current-source package installation or mutation
authorization.
