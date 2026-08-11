# WL-UX-013 final-boundary recovery authority — 2026-08-11

- Scope: paint-time visibility is no longer treated as mutation authority. The
  production Health modal revalidates snapshot freshness, exact canonical
  active-condition identity, selected node or explicit mesh scope, one offered
  remediation descriptor, matching snapshot generation, and required
  confirmation immediately before writing an action. Stale, forged, resolved,
  duplicate, unoffered, cross-target, or unconfirmed actions produce no Bus
  side effect. Acknowledge/snooze remain available for current conditions, and
  mesh issue cards derive `HealthScope::Mesh` through the explicit mesh
  selection convention.
- Production path: Health modal → canonical condition/scope → generation-bound
  remediation validation → Mesh Bus action publication.
- Farm: BigBoy `172.20.0.130`, slot `3`.
- Focused gate:
  `health_modal::tests::governed_action_publication_requires_current_exact_generation_bound_authority`:
  PASS, 1 passed, 0 failed, 1,549 filtered out.
- Remaining epic boundary: action-result progress and partial-failure
  presentation, plus live physical-seat suspend/loss/return proof.
