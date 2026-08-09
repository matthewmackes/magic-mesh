# WL-FUNC-016 VDI orphan-gate cleanup — r21

Date: 2026-08-09

## Production defect

The runtime-reachable clipboard permission controller drained transport updates
with `while let Ok(...)`. An empty channel and a disconnected channel were
therefore indistinguishable. If an RDP, VNC, or SPICE worker disappeared after
approval or during materialization, the receiver remained the active gate with
no sender able to finish it. Reconnect submissions were refused behind that
orphan until another context transition or expiry cleaned it up.

## Correction

`ClipboardPermissionController::poll_ingress` now detects the ticket channel's
terminal disconnect, converts any non-terminal transfer to the existing typed
`ClipboardFailure::Transport`, retains its replay high-water mark, and releases
the payload-free gate. This prevents the abandoned rich sequence from being
materialized twice while permitting a newer reconnect sequence immediately.

The hostile regression uses an HTML offer with a negotiated UTF-8 fallback. It
consumes the one-use approval, drops the only transport ticket before completion,
and proves all three boundaries:

1. the orphan becomes a visible transport failure and releases the gate;
2. the abandoned sequence is refused on reconnect;
3. a newer rich sequence reaches the approval state instead of remaining busy.

Source artifact:

```text
8d97390dfd55593b3d27de3e7ae4cb34110684ec1bb32b09253a655e02b4e32e  crates/desktop/mde-shell-egui/src/clipboard_permissions.rs
```

## Focused farm proof

Host: `172.20.0.130` (BigBoy)

Slot: `func016-vdi-orphan-cleanup-r21`

Command:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=func016-vdi-orphan-cleanup-r21 \
install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  clipboard_permissions::tests::rich_vdi_transport_drop_releases_gate_without_replaying_on_reconnect \
  --features live-vdi --locked -- --exact --nocapture
```

Result:

```text
running 1 test
test clipboard_permissions::tests::rich_vdi_transport_drop_releases_gate_without_replaying_on_reconnect ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1547 filtered out
```

`git diff --check` also passed locally. A package-wide `cargo fmt --check`
reported existing unrelated formatting drift across the shell; no unrelated
source was reformatted.

## Remaining acceptance gap

This is deterministic transport-controller proof, not installed hardware proof.
WL-FUNC-016 still needs live guest evidence that all supported MIME kinds cross
RDP/VNC/SPICE as advertised, that worker loss during a real payload transfer
surfaces the typed failure, and that five-seat local/mesh/VDI memory and payload
cleanup remain bounded.
