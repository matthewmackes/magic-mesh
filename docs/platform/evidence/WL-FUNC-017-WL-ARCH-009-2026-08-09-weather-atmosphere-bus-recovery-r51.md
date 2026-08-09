# WL-FUNC-017 / WL-ARCH-009 — Weather Atmosphere Bus recovery r51

Date: 2026-08-09

## Scope

- Production source: `crates/mesh/mackesd/src/workers/weather_atmosphere.rs`
- Verification host: machine9 (`172.20.0.50`)
- Isolated farm slot: `weather-atmosphere-bus-r51`
- Base revision: `75a128cd98db8f6359e84526d6688d86a3b69be0`

## Corrected behavior

- Construction no longer freezes a possibly missing default Bus root. An explicit worker override wins; otherwise every authority transaction resolves the current configured/user root and falls back to `mde_bus::SYSTEM_BUS_ROOT`.
- Every pre-provider authority read and post-provider authority recheck opens the resolved live Bus and calls `reopen_if_index_changed()` before reading. A late root or replaced `index.sqlite` is therefore observed by the same supervised worker, whose retry waits remain shutdown-aware.
- The exact post-provider effective-location plus admitted-viewport generation/identity recheck remains ahead of cache or map projection effects.
- A fresh provider snapshot is validated and atomically stored in the identity-bound cache before map publication. Cache failure returns an error with no fresh map write. Map publication failure also returns an error, so the refresh schedule cannot record fresh success and the same authority remains retryable; the durable cache is available after restart.

## Focused verification

The dirty working tree was synced with:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=weather-atmosphere-bus-r51 install-helpers/xcp-build.sh sync
```

The isolated farm copy was restored from the base archive and only the owned source was overlaid, preventing unrelated agents' dirty files from entering this proof.

Farm rustfmt:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/weather_atmosphere.rs
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/weather_atmosphere.rs
```

Result: PASS (no output from the check). A repository-wide `cargo fmt --all -- --check` was not used as acceptance evidence because it reports pre-existing formatting drift in unrelated files outside this slice.

Exact tests, both with `CARGO_TARGET_DIR=/home/mm/target-weather-atmosphere-bus-r51`:

```text
cargo test -p mackesd workers::weather_atmosphere::tests::late_and_replaced_bus_recovers_external_authority_and_shutdown -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4497 filtered out`; the worker survived an unopenable root, observed external generation 7 after the root appeared, observed external generation 8 after index replacement, and stopped on shutdown.

```text
cargo test -p mackesd workers::weather_atmosphere::tests::fresh_cache_precedes_publication_and_failures_remain_retryable -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4497 filtered out`; a forced cache failure produced no map row, while a forced map-topic write failure left a valid identity-bound cache and a subsequent retry published successfully.

Scoped checks:

```text
git diff --check -- crates/mesh/mackesd/src/workers/weather_atmosphere.rs
git diff --numstat -- crates/mesh/mackesd/src/workers/weather_atmosphere.rs
```

Result: PASS; source delta `146 insertions, 15 deletions`. No broad or unrelated tests were run.

## Hashes

- Weather Atmosphere source SHA-256: `996b30f83a5458b9adaed61c3d618cb36b2c69d795cc6dd2fc8c461ba4284841`
- Weather Atmosphere working Git blob: `c68d7504f8157af6cf51a3a02c05505597507d6a`

## Residual non-atomic caveats

- The cache filesystem and Bus store cannot commit as one transaction. A crash or Bus failure after cache fsync but before map publication can leave the cache newer than the visible projection; this ordering is deliberate because restart/retry can recover the validated fresh result instead of exposing an unrecoverable fresh projection.
- Authority can change after the exact post-provider recheck and before the separate cache and Bus writes. Snapshot identity remains bound to the checked location and viewport generations, and the next observed authority generation is immediately due, but there is no cross-store conditional commit.
- Viewport admission state and atmospheric map publication are separate Bus writes and are not jointly atomic.

No blockers. No commit or push was performed, and `docs/platform/WORKLIST.md` was not edited.
