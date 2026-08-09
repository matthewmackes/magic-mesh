# WL-FUNC-021 — durable provider selection across restart (2026-08-09 r4)

## Production gap and correction

A typed Music `play` request admitted an exact `ContentRef`, but the durable
queue retained only its provider-owned `remote_id`. After a daemon restart, two
providers offering that same logical track were reordered solely by catalog
policy, silently discarding the operator's selected source.

`Queue` now retains the admitted typed source for its current logical track.
The binding participates in the queue revision, survives atomic persistence and
restart deserialization, and is cleared when the cursor leaves that track. The
normal source resolver attempts the retained provider first after restart while
keeping other admitted variants as bounded failure fallback. Typed play persists
the binding through the existing daemon-owned queue mutation path; no GUI or
provider becomes a second authority.

## Farm proof

- Host: machine 193 (`172.20.0.90`)
- Slot: `func021-r4-20260809`
- Hostile restart/provider regression: 1 passed
- Queue persistence slice: 14 passed
- Adjacent source-aware resolver slice: 2 passed
- Exact-file Rustfmt and scoped `git diff --check`: passed
- `queue.rs` SHA-256: `b83ef73001807ace1250df68499af32fcf0cc2baca6d105c098c16e5e1b03892`
- `bus_responder.rs` SHA-256: `d0d5fcbbbc8fdfb81949103a056e322274cb67015e682cd11e4e5f55baae5694`
- `state.rs` SHA-256: `889cb3fe4efcae562cd60be097a4a1259f6527c926cccc1532e8e97d366625ad`

The hostile regression first proves catalog policy chooses provider two, then
persists an explicit provider-one selection, reloads the queue as a restarted
daemon would, and proves provider one is first while provider two remains the
fallback.

## Remaining live boundary

This is farm-backed queue/provider authority proof, not a live daemon restart
with two configured providers or physical audio continuity. Audible provider
loss/recovery, physical two-seat handoff, and live DLNA/Chromecast renderer
acceptance remain open under WL-FUNC-021.
