# App VM target-identity admission checkpoint

Date: 2026-08-10  
Epic: `WL-FUNC-018` S3

## Defect and correction

`SessionRequest::OpenApp` validated the signed App VM application payload but
accepted empty, control-bearing, or path-like `serving_peer`, `vm_id`, and
`client_peer` values into the session roster. Those identities cross the
session, Workloads, libvirt, and replicated-state boundaries. The broker now
rejects them before `open_app_session` and before any roster mutation.

## Focused farm proof

Host `.50`, slot `func018-appvm-target-admission-r186`:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=func018-appvm-target-admission-r186 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  workers::session_broker::tests::app_vm_open_rejects_path_like_or_empty_target_identities \
  -- --nocapture

1 passed; 0 failed; 0 ignored; 0 measured; 4721 filtered out
```

The hostile regression covers slash, traversal, control-character, and empty
target identities and proves each refusal leaves the roster empty. This is
source/farm admission evidence; image supply, live App-VM boot, VDI
presentation, and physical-seat proof remain open.
