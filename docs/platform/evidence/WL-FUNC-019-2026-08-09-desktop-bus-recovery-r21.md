# WL-FUNC-019 / WL-ARCH-009 — desktop Bus startup recovery (r21)

Date: 2026-08-09

Base commit: `f14d3a0c08c8a05b17c04dc00d639f9e84d0b4e6`

Production source: `crates/mesh/mackesd/src/workers/desktop_sources.rs`
Source SHA-256: `2cf89e9d8571a110d395703abf56178f0ce8dc65427581a3b1b96988c70ac7cd`

## Correction

`DesktopSourcesWorker` no longer returns permanently when `Persist::open` fails
during a boot-order/storage race. A service context without a configured or
user data root selects the documented shared `/run/mde-bus` spool; an
unavailable spool is retried at the worker cadence clamped to 10 ms–2 s, with
every wait interrupted by shutdown. No desktop state, manual-store projection,
cursor state, or mDNS browse is materialized before Bus opens. After the first
successful open, the existing manual-store load, three action-tail cursor
primes, mDNS startup, and immediate first roster publication execute once; Bus
resolution/opening is not re-entered during the normal worker loop.

The test-only resolve/open seam drives root absence, an explicit open failure,
and later availability in one still-running worker. Hostile startup action
history is skipped by the one-time cursor prime, and a corrected-forward
`state/desktops/sources` roster is then published without process restart.

## Focused farm proof

Host: machine 9 (`172.20.0.50`)

Slot: `desktop-bus-recovery-r21`

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=desktop-bus-recovery-r21 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::desktop_sources::tests::bus_absence_wait_is_alive_and_shutdown_prompt \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,408 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=desktop-bus-recovery-r21 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::desktop_sources::tests::bus_open_retry_recovers_forward_without_worker_restart \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,408 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=desktop-bus-recovery-r21 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::desktop_sources::tests::desktop_bus_root_preserves_override_and_has_system_fallback \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,413 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=desktop-bus-recovery-r21 \
install-helpers/xcp-build.sh sync
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.50 \
'source $HOME/.cargo/env 2>/dev/null; \
cd magic-mesh-farm-desktop-bus-recovery-r21 && \
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/desktop_sources.rs'
```

Result: passed with no formatting diff. Scoped `git diff --check` also passed.
No broad suite, package build, installed-seat proof, or unrelated test was run.
