# MUSIC-DOCK + MUSIC-HOME — docked always-open Music + a server-stats Home (design)

**Status:** locked via a 10-Q operator survey, 2026-06-18.
**Trigger:** Operator — "The Music App should always open Maximized; slide in from
the bottom like a docked window, always open. Change the Home page to show stats
about the music on the server (poll Airsonic for what's available)."

## Locked decisions

### Feature 1 — MUSIC-DOCK (docked, always-open window)
| # | Question | Lock |
|---|----------|------|
| 1 | Window model | **Layer-shell dock** — a persistent bottom-anchored surface (the `mde-notify-center` layer-shell pattern), slides up from the bottom edge. |
| 2 | "Always open" | **Autostart at login + minimize-to-handle** — launches at login, stays running; "closing" slides it DOWN to a handle, never quits. |
| 3 | Screen space | **Overlay on top** — slides over the desktop (no exclusive zone; doesn't resize other windows). |
| 4 | Height when open | **Full height (maximized)**. |
| 5 | The handle | **Bottom-center tab** — a small always-visible tab at the bottom-center edge (`♪ Music` + the now-playing title); click slides the dock up. |

### Feature 2 — MUSIC-HOME (server-stats Home page)
| # | Question | Lock |
|---|----------|------|
| 6 | Core counts | **Songs / Artists / Albums + Playlists + Radio/Podcasts** (genres skipped — 926 is noise). |
| 7 | Discovery sections | **Most Played / Frequent + Starred/Favorites + Now Playing across the mesh** (Recently-Added skipped). |
| 8 | Server card | **Host + version · last scan + library size · live scan progress · connection health** (all four). |
| 9 | Refresh | **On open + periodic poll (~30–60 s)** — counts / now-playing / scan stay live. |
| 10 | Layout | **Hero numbers + sections below** — big Songs/Artists/Albums up top, then the server card + Most-Played/Starred/Now-Playing strips. |

## Live Airsonic facts (polled 2026-06-18, `172.20.0.2:4040`)
`getScanStatus` → 23,126 songs, not scanning · `getArtists` → 240 · `getGenres`
→ 926 · `getPlaylists` → 1 · `getInternetRadioStations` → 4 · `getPodcasts` → 0.
Album total via `getAlbumList2` (count). Most-played = `getAlbumList2 type=frequent`;
starred = `getStarred2`; now-playing = the existing mesh now-playing data.

## Architecture
- **MUSIC-DOCK** rearchitects `mde-music`'s top-level surface from a normal iced
  window to a **layer-shell** surface (Overlay layer, `Anchor::BOTTOM|LEFT|RIGHT`,
  full height, `KeyboardInteractivity::OnDemand`) — reuse the `mde-notify-center`
  boot/single-instance/Esc pattern. A separate tiny **bottom-center handle**
  surface (Top layer, non-exclusive) stays mapped when the dock is hidden;
  pressing it re-maps the dock. Slide-in = an iced `time`-driven offset/opacity
  animation on map. Autostart `.desktop` (Workstation-gated, like the wallpaper).
- **MUSIC-HOME** adds daemon stats verbs to `mde-musicd`
  (`action/music/library-stats` → counts + scanStatus + server host/version +
  reachability; reuse `getAlbumList2 frequent` / `getStarred2` for the strips),
  and rebuilds the `Route::Hub` Home view in `mde-music` into the hero-numbers +
  sections dashboard, polled on a timer.

## Acceptance (high level; per-task bullets in the worklist)
- Music launches at login as a bottom-docked, full-height, always-running surface
  that slides up from the bottom; "closing" slides it to a bottom-center `♪ Music`
  tab; clicking the tab slides it back up. Overlay (doesn't reshape other windows).
- The Home page shows live server stats (songs/artists/albums/playlists/radio/
  podcasts), a server card (host/version/scan/library/health), and Most-Played /
  Starred / mesh Now-Playing strips; refreshes on a poll.
- All Carbon tokens (§4); no stubs (§7) — every stat is real Airsonic data.

## Risks / out of scope
- Layer-shell rearchitecture is the riskiest piece (surface lifecycle, focus,
  the second handle surface) — stage it behind MUSIC-HOME, which is lower-risk.
- Reduce-motion: the slide animation must honor the adaptive-motion budget.
- Not in scope: multi-monitor dock placement (primary output first).
