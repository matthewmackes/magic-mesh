# WL-REL-001 version metadata farm evidence — 2026-08-16

- Source revision: `6633efb6`
- Farm host: `172.20.0.50`
- Farm slot: `wl-rel001-version-metadata-20260816`
- Command: `cargo metadata --no-deps --format-version 1`
- Result: `PASS`

The metadata contained the complete workspace and resolved shipped crates to
`13.0.0`; the documented internal packaging/test boundaries remain `0.0.0`.
The metadata gate does not freeze the source or admit release inputs; those
remain dependent on WL-FUNC-023 and WL-REL-006.
