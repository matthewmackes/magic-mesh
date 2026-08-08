# WL-FUNC-021 — media roaming session admission

Status: bounded source/fixture checkpoint complete; live two-seat handoff,
mesh propagation, and installed-seat proof remain `Remaining`.

## Change

`mde-media-core` now admits replicated roaming rows only when they:

- belong to the requested mesh identity and the canonical single-writer seat
  filename;
- carry a positive, non-saturated lease generation and timestamp;
- contain finite, non-negative playback positions within any declared duration;
- stay within bounded queue, track, and display/media text limits; and
- use valid playlist cursor and positive decoder-track identities.

Rows that fail these checks are ignored before lease-owner selection or resume.

## Verification

BigBoy `.130`, slot `media-roaming-admission-20260806-r3`:

```text
11 passed, 0 failed, 230 filtered out
```

The new hostile cross-identity, claimed-seat, and saturated-lease fixture is
included. Farm `.50`, slot `media-roaming-format-20260806-r4`, passed the crate
format check. No live seat or mesh state was changed.

## Source hash at capture

```text
beb7465a84fdbcfab7171617f27961b1ccf8032e0b08ee86ff2a7d2e8da2cb08  crates/desktop/mde-media-core/src/roaming.rs
```
