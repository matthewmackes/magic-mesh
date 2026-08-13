# WL-UX-011 — truthful display-provider readiness (r548)

Date: 2026-08-13

Commit: recorded by the enclosing commit.

## Result

The existing `hardware_probe` inventory tick now also publishes a bounded
`display-provider/<node>.json` observation. The classifier cross-checks kernel
DRM connector status, enablement, and available modes with the configured
Construct seat service and the kernel DRM client/master table. It emits only
`Ready`, `Disconnected`, `Disabled`, or `Unknown`, connector counts, and a fixed
reason. Connector identities, EDID data, session IDs, PIDs, command output,
user content, and secrets are never projected. The provider is read-only and
adds no mutation authority.

Malformed, incomplete, duplicate, contradictory, ambiguous-master, and
substituted-seat observations fail to `Unknown`.

## Exact gate record

- BigBoy `.130`, slot 3: strict all-target `mackesd` Clippy compiled the new
  production path and stopped on one slice-local Rust borrow error in
  `gather_drm_connectors` (`E0505`, moving the connector identity while its card
  substring remained borrowed). The borrow was corrected exactly after the
  gate; cadence prohibited a rerun.
- `.196`, slot 1: package formatting ran once and failed on broad pre-existing
  crate formatting drift. The output included many files outside this slice;
  no broad rewrite was performed.
- `.50` focused-test and build commands were stopped during pre-build rsync
  after concurrent workers claimed the same workspaces. Neither compiled and
  neither is claimed as evidence.
- Scoped `git diff --check` is the final local syntax/whitespace check.

No focused test, successful build, successful Clippy, or clean package-format
claim is made. The hostile regression is present but remains to be executed by
a later permitted gate wave.

## Residual acceptance

- Execute the module-qualified hostile display-provider regression with
  nonzero discovery.
- Run strict relevant Clippy, production build, and owned formatting against
  the corrected commit.
- Include the provider in the first full signed release.
- Physical connector, DRM-master handoff, and one-node display acceptance stay
  deferred and non-blocking until after that release.
