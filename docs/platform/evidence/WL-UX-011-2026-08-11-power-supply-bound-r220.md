# WL-UX-011 evidence — power-supply inventory bound (r220)

- Scope: Device Inventory provider.
- Change: power-supply entities are admitted in deterministic lexical order
  with a hard cap of 64 published records.
- Farm host: `172.20.0.50`.
- Farm slot: `ux011-power-supply-bound-r220`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=ux011-power-supply-bound-r220 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::device_inventory::tests::power_supplies_are_bounded_and_deterministically_admitted -- --exact --nocapture`
- Result: `1 passed; 0 failed`.
