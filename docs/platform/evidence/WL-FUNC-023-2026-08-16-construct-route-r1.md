# WL-FUNC-023 Construct lifecycle route correction

- Date: 2026-08-16 UTC
- Code source revision: `c7cbadfb5e67584c68c1bc4de8694ad0758c4dfa`
- Commit epoch: `1786913786`
- Scope: Construct legacy lifecycle entry point

## Change

The Construct Power Cycle panel's “Open Onboarding & Offboarding” action now
defaults to the shipped `/usr/bin/magic-setup` binary. The previous default,
`/usr/bin/mde-onboarding-offboarding`, was not installed by the RPM and made
the lifecycle route unavailable on a normal packaged node. The
`MDE_ONBOARDING_OFFBOARDING_BIN` override remains available for an explicitly
installed renderer-specific launcher.

## Farm verification

BigBoy (`172.20.0.130`), slot `2`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  --bin mde-shell-egui power_cycle --locked -- --nocapture

test result: ok. 3 passed; 0 failed; 0 ignored; 1625 filtered out
```

The initial `--lib` probe was correctly refused because this package exposes a
binary target rather than a library target; it did not mutate the source or
artifacts.

## Boundary

This correction makes the packaged legacy route reach the shared lifecycle TUI;
it does not claim that full installed/live lifecycle acceptance, fleet
replacement, or release promotion is complete. Those remain governed by
WL-TEST-002 and the release evidence chain.
