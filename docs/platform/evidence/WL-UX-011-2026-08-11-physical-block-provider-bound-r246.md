# WL-UX-011 physical block provider bound — 2026-08-11

- Scope: the installed device-inventory worker admits at most 256 physical
  `/sys/block` rows per generation. Virtual `loop`, `dm`, RAM, zram, MD, and
  optical entries are filtered before the physical budget, so they cannot hide
  real disks from the replicated Workers projection.
- Farm: `172.20.0.50`, slot `1`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::device_inventory::tests::physical_block_provider_is_bounded_after_virtual_device_filtering -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,785 filtered out.
