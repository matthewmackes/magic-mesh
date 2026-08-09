# WL-ARCH-009 / WL-UX-012 — Copilot Bus startup recovery (r16)

Date: 2026-08-09

Base commit: `c5f4f232d5973c9244ea09d46ef4d3ed13bf0d47`

Production source: `crates/mesh/mackesd/src/workers/copilot.rs`

Source SHA-256:
`3a73ff11368c8b99af05453ce6d31c6f80dc41897d525151d162f39fe464a6c4`

## Correction

`CopilotWorker` no longer exits permanently when the shared Bus is unavailable
or `Persist::open` fails during startup. An explicit override remains exact;
otherwise the documented mde-bus resolver is used, with
`mde_bus::SYSTEM_BUS_ROOT` as the fixed service-context fallback. The worker
retries at its poll cadence clamped to 10 ms–2 s, and shutdown interrupts every
retry wait. It does not search arbitrary roots or materialize other state.

Opening and reading the `action/copilot/ask` tail now form one activation
transition. A tail-read failure retries instead of activating with a `None`
cursor, which could replay retained asks. After the first successful open and
tail read, the cursor is primed once and the existing request, status,
suggestion, and alert-triage timers retain their prior startup and recurring
cadences. Startup history is skipped; a fresh signed ask is answered exactly
once, and recovery does not fabricate a proposal or duplicate reply work.

## Focused farm proof

Host: machine 196 (`172.20.0.196`)

Slot: `copilot-bus-recovery-r16`

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=copilot-bus-recovery-r16 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::copilot::tests::copilot_bus_root_preserves_override_and_has_system_fallback \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,423 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=copilot-bus-recovery-r16 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::copilot::tests::unavailable_bus_wait_is_alive_and_shutdown_prompt \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,423 filtered out`.

```text
ssh mm@172.20.0.196 \
'cd /home/mm/magic-mesh-farm-copilot-bus-recovery-r16 && \
cargo test -p mackesd --features async-services --lib \
workers::copilot::tests::bus_open_retry_recovers_without_replaying_or_duplicating_asks \
-- --exact --nocapture'
```

Result: `1 passed; 0 failed; 4,423 filtered out`.

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/copilot.rs
```

Result: passed on machine 196 after syncing the exact final source. Scoped
`git diff --check` also passed. No broad suite, package build, installed-seat
proof, or unrelated test was run.
