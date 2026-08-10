# WL-FUNC-019 — Service launch route isolation (r142)

Date: 2026-08-10

Source revision: `dfbd3cfe`

## Result

A ready provider Service card with a typed `Launch` action cannot be
substituted onto the Workloads start-and-attach authority. The exact shipped
resource class, provider identity, action verb, catalog digest, and invocation
identity are admitted before the planner returns `TargetMismatch` for the
cross-authority request.

No production correction was required: the existing resource-class authority
validation already fails closed. The fixture preserves that behavior against
future universal-browser route expansion.

## Focused farm proof

Machine 193 build VM `.90`, isolated slot `func019-service-route-r142`:

```text
cargo test -p mackesd \
  workers::service_aggregator::resource_actions::tests::ready_service_launch_cannot_cross_route_to_workload_authority \
  -- --exact --nocapture
```

Result: 1 passed, 0 failed, 4,679 filtered. Focused rustfmt and
`git diff --check` passed. No physical seat was used.

## Remaining boundary

This checkpoint proves negative Service/Launch route isolation only. Positive
service execution routing, persisted-ingress coverage, additional resource
class fixtures, responsive captures, and live recovery proof remain.
