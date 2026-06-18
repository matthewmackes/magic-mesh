# MUSIC-ALBUMS — Music app redesign (Claude Design import: Albums.dc.html)

Source: claude.ai/design project `6cf3ae71-13c5-4fb6-8019-38eb5c9cf8d5`,
file `Albums.dc.html`, imported via the Claude Design connector 2026-06-18.
Target: `crates/services/mde-music` (the `mde-music` GUI). Carbon Gray-100 dark.

## Locked layout (from the .dc.html)
A full-window CSS grid: rows `48px / 1fr / 80px`, cols `256px / 1fr`,
areas `header header / sidebar content / player player`.

- **Header (48px):** hamburger glyph · "**MCNF** Music" wordmark · centered
  search input ("Search artists, albums, songs", max 480px, magnifier icon,
  `#262626` fill) · account/avatar icon. Bottom border `#393939`.
- **Sidebar (256px):** vertical nav, `#161616`, right border `#393939`.
  - Top: **Home**, **Internet Radio**.
  - **LIBRARY** section header (uppercase, `#8d8d8d`): **Albums** (active —
    `#262626` fill + `inset 3px 0 0 #4589ff` accent rail + `#f4f4f4` bold),
    **Artists**, **Songs**, **Playlists**.
  - divider, then **Recently Added**, **Settings**.
  - rows: 40px, gap 12px icon→label, 14px label, `#c6c6c6` idle /
    `#f4f4f4` on hover (`#262626` hover fill). Carbon line icons.
- **Content (1fr, scroll):** breadcrumb "Library / Albums"; title row —
  `Albums` (28px/600) + "`N albums`" subcount (`#8d8d8d`) left, **Sort: Name
  A–Z** button (`#262626`, bottom border `#8d8d8d`, caret) right; then the
  **album grid**: `repeat(auto-fill, minmax(168px,1fr))`, gap 24px. Each card:
  square art (`aspect-ratio:1`, `#262626`, `1px #393939` outline → `2px #4589ff`
  on hover) + 2-line clamped title (14px `#f4f4f4`) + 1-line artist
  (12px `#8d8d8d`, ellipsis).
- **Player (80px):** 3 cols `1fr / auto / 1fr`, `#262626`, top border `#393939`.
  - left: 48px art tile + track title/artist (ellipsis).
  - center (520px): transport row — shuffle · prev · **play** (40px white
    circle, dark glyph) · next · repeat (`#c6c6c6`, hover `#f4f4f4`); below it
    a scrubber — `mm:ss` · 4px track (`#525252`) with `#f4f4f4` fill + 12px
    knob · `mm:ss` (tabular-nums).
  - right: volume icon + 96px volume bar (same track/knob style).

## Carbon tokens (map to mde-theme, no raw hex)
`#161616`=background, `#262626`=surface/raised, `#393939`=border,
`#f4f4f4`=text, `#c6c6c6`=text (idle nav), `#8d8d8d`=text_muted,
`#525252`=track, `#4589ff`=accent (Carbon Blue-50). All via `mde-theme`
tokens — add a Blue-50 accent token if absent (§4, with a palette test).

## Worklist (incremental per /design-sync)
- MUSIC-ALBUMS-1: shell — the 48/1fr/80 + 256/1fr Carbon grid (header + sidebar
  + content + player regions), dark + light via mde-theme.
- MUSIC-ALBUMS-2: sidebar nav (Home/Internet Radio/LIBRARY{Albums,Artists,Songs,
  Playlists}/Recently Added/Settings) with the active accent rail; wire each to
  the existing mde-music views.
- MUSIC-ALBUMS-3: Albums grid (auto-fill 168px cards, square art + 2-line title +
  artist, hover accent outline) bound to the real library (reuse the windowed art
  loader from the MUSIC-LOCK-FIX so large libraries don't stall).
- MUSIC-ALBUMS-4: header search (filter artists/albums/songs) + Sort control.
- MUSIC-ALBUMS-5: the 80px persistent player (reuse the existing playback bar
  state — mini-art, shuffle/prev/play/next/repeat, scrubber, volume).
- MUSIC-ALBUMS-6: account/avatar + Settings routing (mesh routing prefs).

## Acceptance (runtime-observable)
- mde-music renders the Carbon grid (header/sidebar/content/player) in dark +
  light; Albums is the active nav with the blue accent rail.
- the album grid populates from the real library, cards show art + 2-line title +
  artist, hover shows the accent outline; clicking a card opens the album.
- the persistent 80px player works (transport + scrubber + volume) from any view.
- no raw hex outside mde-theme (§4); large libraries don't lock the UI.

## Notes
- Reuses the existing mde-music playback/library backend — this is a **view
  rebuild** (the §6 glue-not-reimpl rule), not a new daemon.
- The `support.js` / `x-dc` / `sc-for` are the design-canvas runtime; the real
  data binding is mde-music's library model, not the demo `albums` array.
