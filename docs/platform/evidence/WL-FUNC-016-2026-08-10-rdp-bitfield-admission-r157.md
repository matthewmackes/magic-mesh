# WL-FUNC-016 — RDP bitfield DIB admission (r157)

Date: 2026-08-10

The RDP clipboard decoder now rejects a 40-byte `BI_BITFIELDS` header without
the required channel-mask space, preventing malformed image materialization.
Farm proof on `.50` used the feature that exposes the live RDP clipboard path:

```text
MCNF_BUILD_HOST=172.20.0.50
MCNF_BUILD_SLOT=func016-rdp-bitfields-r157c
install-helpers/xcp-build.sh cargo test -p mde-vdi-rdp --features live-connect \
  --lib clipboard::tests::bounded_dibv5_negotiation_round_trips_and_rejects_hostile_geometry -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 97 filtered out
```

