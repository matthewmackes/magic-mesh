# WL-ARCH-010 Display1 relay-loss reset — r166

- Revision: `89c05974` (`clear Display1 input after relay loss`).
- Scope: when a QEMU relay disappears, focus, held key/button edges, and the input sequence are cleared even when best-effort release cannot reach the old endpoint; the next relay starts from a clean state.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-relay-reset-r166b install-helpers/xcp-build.sh cargo test -p mackesd --lib display1_broker::tests::relay_loss_reset_clears_stale_focus_edges_and_sequence -- --nocapture`
- Result: `1 passed; 0 failed; 4706 filtered out` on seat 50.
