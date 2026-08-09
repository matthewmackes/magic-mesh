# WL-FUNC-019 typed universal-resource actions S4 — 2026-08-08

The service aggregator now drains one bounded resource-action ingress and
revalidates the exact catalog revision/digest, publisher attestation, card,
action, target, generation, authorization, deadline, and cancellation nonce
before publishing. Only closed Workload, VDI, clipboard, and Android-provider
request types can cross the router. Downstream topics are fixed in code; the
wire contract contains no caller-selected command, executable, path, URL, or
topic. Existing actionless adapters remain actionless. Accepted actions issue
an exact receipt; cancellation must name that receipt and is routed only to the
same downstream authority, target, generation, and original request identity.

## Verification

Machine 194 (`172.20.0.170`), slot `func019-workload-cancel-card-s5-r1`:

```text
cargo test --locked -p mackesd --lib resource_actions \
  --features async-services -- --nocapture
22 passed; 0 failed
```

The integrated shell `vdi::resources` suite also passed 10/10. Together the
fixtures prove exact Workload, VDI, clipboard, and Android dispatch; distinct
authorization contexts; exact reply binding; accepted-receipt cancellation;
and refusal of stale/unavailable cards, catalog/action substitution, capability
mismatch, caller-supplied downstream tokens, raw locator/command fields, and
mismatched cancellation identities. Scoped formatting and diff checks passed.

The current integrated tree was reverified on machine 196
(`172.20.0.196`), slot `integrated-resource-actions-s6-r1`: all 22 focused
daemon tests passed, including persisted signed Bus ingress, attestation,
idempotent cursor/reply handling, and replay-safe downstream completions.

## Remaining acceptance gap

A generic standalone Cancel action in the closed schema and a provisioned live
round trip remain. FUNC-019 stays `Remaining`.
