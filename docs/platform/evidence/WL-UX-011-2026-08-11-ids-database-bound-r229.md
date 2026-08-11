# WL-UX-011 hardware ID database bound — 2026-08-11

- Scope: `pci.ids` and `usb.ids` naming inputs are restricted to bounded regular files, capped at 16 MiB before parsing.
- Farm: BigBoy `172.20.0.130`, slot `ux011-ids-db-bound-r229`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux011-ids-db-bound-r229 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::device_inventory::tests::oversized_ids_database_is_rejected_before_parse -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
