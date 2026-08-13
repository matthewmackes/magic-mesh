# WL-UX-011 — truthful audio-provider readiness (r541)

## Result

The existing rank-0 `hardware_probe` cadence now publishes a bounded
`audio-provider/<node>.json` projection. PipeWire graph evidence and
WirePlumber policy evidence are collected from the durable `mm` user session
through the established `runuser` plus `XDG_RUNTIME_DIR` seam, while
`/sys/class/sound` supplies the independent kernel inventory.

The projection exposes only schema version, node identity, observation time,
typed `ready` / `disconnected` / `disabled` / `unknown` readiness, bounded card
and audio-node counts, and a fixed reason. It publishes no command output,
device labels, user names, routes, profiles, credentials, or secrets and adds
no mutation authority.

Classification fails to `unknown` when PipeWire or WirePlumber is absent,
malformed, non-UTF-8, oversized, or contradictory; when kernel facts are
unavailable, oversized, malformed, duplicated, or substituted; and when a
PipeWire audio graph claims endpoints while the kernel exposes no audio card.
Hardware with a valid policy graph but no PipeWire audio node is
`disconnected`; mutually empty provider and kernel inventories are `disabled`.

## Farm gates

- `.170`, slot 1: focused hostile regression
  `workers::audio_provider::tests::hostile_audio_observations_fail_unknown_without_leaking_provider_data`
  passed 1/1.
- `.170`, slot 1: strict `cargo clippy -p mackesd --features async-services
  --all-targets -- -D warnings` passed against final source.
- `.170`, slot 2: `cargo build -p mackesd --features async-services --lib`
  passed against final source. An earlier all-target attempt reached linking but
  exhausted that lane's owned target volume; the failed target was removed and
  is not counted as evidence.
- `.170`, slot 2: exact owned-file Rustfmt checks passed for
  `audio_provider.rs` and `hardware_probe.rs`. The package-wide check was run
  once and remained red on extensive pre-existing formatting drift outside
  this slice; no unrelated rewrite was made.
- Scoped `git diff --check` passed.

## Remaining boundary

WL-UX-011 remains open for display, printers, services, privacy,
virtualization, and additional capability-gated safe-control coverage.
Physical audio transitions and installed one-node acceptance remain deferred,
non-blocking post-release proof.
