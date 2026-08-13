# WL-UX-011 service-provider truth checkpoint (2026-08-13, r546)

## Result

The running service aggregator now publishes a bounded, credential-free
`state/service-provider/<node>` snapshot from systemd's own `Id`, `LoadState`,
`ActiveState`, and `UnitFileState` facts for six fixed platform units. Each unit
is projected as `Ready`, `Disconnected`, `Disabled`, or `Unknown`. The provider
does not read command lines, environments, status text, journals, or secrets and
adds no service lifecycle or mutation authority.

Incomplete, duplicate, substituted, oversized, malformed, and contradictory
observations fail closed. The publication is folded into the existing
service-aggregator Bus cycle rather than introducing another worker registry.

## Farm gates

- BigBoy `.130`, slot 1: focused hostile regression passed 1/1:
  `workers::service_aggregator::service_provider::tests::hostile_service_facts_fail_closed`.
- BigBoy `.130`, slot 1: strict `mackesd` async-services all-target Clippy passed.
- `.196`, slot 1: production `mackesd` async-services build passed.
- BigBoy `.130`, slot 2: exact owned-file Rustfmt passed with child traversal
  disabled; crate-wide formatting remains independently red in unrelated files.
- Scoped `git diff --check` passed.

No live systemd or Workers UI proof is claimed; acceptance remains deferred
until after the first full release.
