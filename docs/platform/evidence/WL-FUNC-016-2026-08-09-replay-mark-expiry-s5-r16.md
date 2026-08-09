# WL-FUNC-016 S5 replay-mark expiry checkpoint

Date: 2026-08-09

Base revision: `40a383a72c15721db1b652262d308613a1fb32f0` plus the scoped worktree patch described here.

## Production boundary

`ClipboardPermissionModel` previously bounded replay high-water marks only by a
128-source row cap. A terminal transfer could therefore leave a quiet
source/seat/session blocked indefinitely after the envelope or VDI lease that
admitted its sequence had expired.

Each replay mark now retains the effective admitting authority expiry (the
envelope/lease minimum already calculated by the permission model). Admission
prunes marks at that exact boundary before applying the source sequence
high-water check. A newer terminal sequence extends both the sequence and
expiry monotonically. This remains metadata-only state in the existing
clipboard permission model; it adds no clipboard store, transport, or authority.

## Hostile and boundary proof

One focused farm slot was used on farm machine `.50` (`172.20.0.50`):

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=func016-replay-expiry \
install-helpers/xcp-build.sh \
  cargo test -p mde-shell-egui replay_mark_ -- --nocapture
```

Result: PASS, 2 passed, 0 failed, 0 ignored, 1504 filtered out.

- `replay_mark_expires_at_its_authority_boundary` proves that an identical
  source/session sequence is refused one millisecond before expiry and admitted
  at the exact expiry boundary under renewed authority.
- `replay_mark_newer_terminal_extends_retention_without_sequence_rewind` proves
  that transport failure records a mark and a later terminal sequence extends
  retention without allowing an older sequence back through early.

The build emitted pre-existing unrelated warning classes; there were no test
failures or blockers in this scoped boundary.
