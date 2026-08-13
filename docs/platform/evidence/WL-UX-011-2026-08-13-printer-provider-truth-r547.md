# WL-UX-011 — truthful printer-provider readiness (r547)

## Result

The existing `cups_sync` worker now publishes a bounded
`printer-provider/<node>.json` readiness projection on its established cadence.
It cross-checks the exact `cups.service` load, enablement, active, and substate
facts with the CUPS scheduler, configured-queue count, and kernel USB printer
class inventory.

The projection contains only schema version, node identity, observation time,
typed `ready` / `disconnected` / `disabled` / `unknown` readiness, bounded
queue and kernel-printer counts, and a fixed reason. Queue names, job metadata,
device labels, command output, credentials, and secrets never cross the
projection boundary. No mutation authority was added.

Missing, malformed, oversized, incomplete, duplicate, contradictory, or
substituted systemd, CUPS, queue, and kernel observations fail to `unknown` and
zero the counts. A valid absent/disabled CUPS service is `disabled`; an enabled
but stopped service or a running scheduler without a queue is `disconnected`;
only mutually consistent running service, scheduler, and queue facts are
`ready`.

## Farm gates

- BigBoy `.130`, `ux011-printer-test-r547`: focused hostile regression
  `workers::cups_sync::tests::hostile_printer_provider_facts_fail_unknown_without_exposing_identifiers`
  passed 1/1, with 4,990 filtered out.
- BigBoy `.130`, `ux011-printer-clippy-r547`: strict
  `cargo clippy -p mackesd --features async-services --all-targets -- -D warnings`
  passed against the final production source.
- BigBoy `.130`, `ux011-printer-build-r547`:
  `cargo build -p mackesd --features async-services --lib` passed against the
  final production source.
- Scoped `git diff --check` passed before evidence recording.
- The requested combined build-plus-Rustfmt helper invocation was rejected by
  `xcp-build.sh` before execution because the helper accepts only direct Cargo
  subcommands. Per the one-run cadence, no replacement or rerun was added;
  strict Clippy and the production build are the valid formatting-sensitive
  compiler gates retained above.

## Remaining boundary

WL-UX-011 remains open for display, privacy, and further capability-gated
safe-control coverage. Physical CUPS/printer transitions and installed one-node
acceptance remain deferred, non-blocking post-release proof.
