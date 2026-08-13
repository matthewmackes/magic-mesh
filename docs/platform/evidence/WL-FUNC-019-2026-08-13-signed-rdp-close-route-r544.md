# WL-FUNC-019 — signed Windows RDP close route (r544)

Date: 2026-08-13

## Executable gap

The universal-resource router's receipt-bound external RDP close path rejected
any invocation carrying `armed_token`. That field is mandatory on the signed
`action/resources/invoke` ingress and is consumed before route planning, so the
planner-only close fixture hid a production contradiction: a locally approved
Windows RDP Open could produce a signed authority receipt, but its authenticated
Close could never reach session authority.

## Result

`plan_receipt_bound_vdi_close` now accepts the already-verified outer ingress
token. It still requires the exact signed Open receipt, original resource/action
binding, session identity, catalog digest and generation, explicit cancellation
target, bounded deadline, and fresh one-use ingress authorization. The router
re-arms a separate closed `SessionRequest::Close` for the downstream session
authority; no caller token or command is forwarded.

The new persisted-Bus regression performs the complete route: signed and locally
approved external RDP Open, typed signed Open completion, then a separately
signed receipt-bound Close. It proves one Close reaches only
`action/vdi/session` and cancels the exact Open request.

## Farm gates

- `.90` slot 2: exact persisted signed-ingress regression passed 1/1:
  `workers::service_aggregator::resource_actions::tests::signed_approval_gated_rdp_open_can_route_its_receipt_bound_close`.
- `.170` slot 2: strict `cargo clippy -p mackesd --all-targets --all-features -- -D warnings` passed.
- `.170` slot 2: `cargo build -p mackesd --all-targets --all-features` passed.
- `.170` slot 1: crate-wide `cargo fmt -p mackesd -- --check` ran once and
  remained red on pre-existing formatting drift outside this slice; the owned
  file matches Rustfmt.
- Scoped `git diff --check` passed.

No live Windows login, rendering, clipboard, or recovery claim is made. Those
remain deferred non-blocking post-release acceptance together with installed
publisher credentials and the signed runtime artifact.
