# MUSIC-RFX — music client/interface refactor (sonixd-inspired)

Design survey locked 2026-06-17 (3-round `/plan`), prompted by "investigate using
[sonixd](https://github.com/jeffvli/sonixd) to refactor the Music Client and
Interface."

## Investigation finding (why this is a reference, not a port)

**sonixd is TypeScript + React + rsuite + Electron** (archived Aug 2024; author
moved to "Feishin"). Its **code cannot be reused** in Magic Mesh — `AI_GOVERNANCE.md`
§2/§4/§6 lock the stack to **Rust + iced/libcosmic + cosmic-text**, strictly **IBM
Carbon** theming, Cosmic owns the shell, no web/Electron shells. So sonixd is a
**UX/interaction reference**; we reimplement its strong patterns in the existing
`mde-music` (iced GUI) + `mde-musicd` (daemon). We already ship ~80% of sonixd's
surface (7 library hub cards, album pages w/ dominant-colour art, search,
now-playing, MPRIS, **gapless** playback) **plus** features sonixd lacks (mesh
playback hand-off, mesh-shared artwork + audio cache). This refactor closes the
gaps that are sonixd's strengths.

## Locks (10)

| # | Decision | Lock |
|---|----------|------|
| Q1 | Approach | **UX reference — reimplement in iced** (reuse our Subsonic client/engine/mesh features) |
| Q2 | Scope | **Full overhaul** — GUI (`mde-music`) + daemon (`mde-musicd` queue/engine) + interactions |
| Q3 | Queue mgmt | **Full** — drag-reorder + multi-select + remove(one/many) + play-next |
| Q4 | Navigation | **Keep the Library hub-cards + breadcrumb**, polish density/rows in place (no sidebar paradigm change) |
| Q5 | Now-playing | **Full maxi view** — large art + seek scrubber + prev/next + up-next peek |
| Q6 | Playback | **Gapless-only** — no crossfade / ReplayGain this round (engine already gapless) |
| Q7 | Playlists | **Full editing** — create/rename/delete/reorder + **Add-to-playlist everywhere** |
| Q8 | Performance | **Virtualize long lists** (album grid / artists / queue render only visible rows) |
| Q9 | Interactions | **Right-click context menus** for track/album/playlist actions (pairs with multi-select) |
| Q10 | Delivery | **One full release** — build the whole refactor, ship together (not phased) |

**Standing constraints (not surveyed — governance-forced):** strictly IBM Carbon
tokens (`mde-theme`, no sonixd multi-theme); iced 0.14/libcosmic; mesh features
(peer hand-off `take-over`, shared artwork/audio cache, MPRIS) preserved.

## Architecture (where each piece lands)

```
  mde-music (iced GUI)                         mde-musicd (daemon)
  ───────────────────                          ───────────────────
  Library hub (polished, virtualized)  ──bus──▶ browse verbs (have)
  Queue panel: drag-reorder, multi-                queue.rs: + move / remove /
    select, remove, context menu       ──bus──▶     remove_many / move_to_next
  Maxi now-playing: art + SCRUBBER ────bus──▶ transport: + seek  → engine.rs seek
    + prev/next + up-next peek
  Playlist editor: create/rename/      ──bus──▶ airsonic.rs: create_playlist /
    delete/reorder + Add-to-playlist                update_playlist / delete_playlist
  Right-click context menus everywhere ──bus──▶  (new write verbs)
```

- **New daemon bus verbs:** queue — `queue-move`, `queue-remove`, `queue-remove-many`,
  `queue-move-to-next`; transport — `seek`; playlist — `playlist-create`,
  `playlist-update`, `playlist-delete`. Each replies on `reply/<ulid>` like the rest.
- **Engine seek:** `engine.rs` gains a seek (reposition the symphonia decode) so the
  scrubber can scrub; `get-state` already returns `position_ms`.
- **Virtualization:** iced lazy/`scrollable` rendering only visible rows — applied to
  the album grid, artist list, and the queue.
- **Context menus:** an iced overlay/menu on right-click; the action set routes to the
  existing + new bus verbs; multi-select drives the bulk actions.
- **Add-to-playlist:** a chooser (existing playlists + "new") reachable from every
  track row's context menu (album, search, queue, now-playing).

## Tasks → MUSIC-RFX-1..11 (see WORKLIST)

Daemon: queue model + verbs (1), engine seek + verb (2), playlist write verbs (3).
GUI: maxi now-playing + scrubber (4), queue panel reorder/multi-select/remove (5),
playlist editor (6), add-to-playlist-everywhere (7), right-click context menus (8),
list virtualization (9), Carbon density/row polish (10). Cross-cutting: keep the
mesh hand-off + shared caches working through the refactor (11).

## Acceptance (runtime-observable, per §7)
- Drag a track in the queue → its order persists + playback follows; select 3 tracks
  → remove → all 3 gone; "play next" inserts after the current track.
- Maxi now-playing shows live art + a scrubber that **seeks** audio (position jumps),
  prev/next work, and the up-next list reflects the real queue.
- Create a playlist, rename it, add tracks via a track's right-click "Add to playlist",
  reorder + delete it — all reflected on the server (re-query confirms).
- A right-click on any track/album/playlist row opens a context menu whose actions
  work (play, play-next, add-to-playlist, go-to-album, remove).
- A library with thousands of albums scrolls smoothly (only visible rows rendered).
- Mesh playback hand-off (`take-over`) + shared artwork/audio cache still work after
  the refactor.

## Risks
- **iced context-menu / drag-drop maturity** — right-click menus + drag-reorder in
  iced 0.14/libcosmic may need custom widgets; spike early, fall back to inline
  buttons for any action the menu can't host.
- **Engine seek correctness** — seeking a streamed (radio) source vs a finite track
  differs; seek only finite tracks, disable the scrubber for live streams.
- **Scope** — "one full release" is a large surface; keep each MUSIC-RFX-N
  individually runtime-reachable so the release isn't blocked on the weakest item.
- **Carbon fidelity** — denser sonixd-style rows must still come from `mde-theme`
  tokens (no raw hex/metrics), §4.

## Out of scope (v1)
- Crossfade / ReplayGain (Q6 = gapless-only).
- Sidebar navigation paradigm (Q4 = keep hub-cards).
- sonixd's multi-theme system (governance = IBM Carbon only).
- Jellyfin backend (we target Subsonic/Airsonic; Jellyfin is a later add).
