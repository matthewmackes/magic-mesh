# WL-FUNC-021 Music detail routes — 2026-08-07

## Result

Artist and Podcast rows now open typed daemon-owned detail surfaces. The daemon
accepts the exact `get-artist` alias, persists bounded `ContentKind::Episode`
rows, joins episodes to the retained podcast title, and projects them into the
workspace snapshot. Radio rows open an honest station surface and retain their
source-qualified stream URL; typed radio playback remains intentionally open.

This closes a contract gap, not a missing Subsonic documentation gap. The
Subsonic endpoints were already implemented, but the response-to-workspace
projection and UI detail joins were incomplete. Provider dialect differences
(for example `streamId` versus `id`) still require explicit normalization.

## Farm gates

- `mde-musicd`: 196/196 on BigBoy (`172.20.0.130`).
- `mde-music-egui`: 61/61 on `.50` (`172.20.0.50`).
- Focused detail tests: daemon 1/1 and UI 1/1.
- Music UI format check: pass on `.50`.
- Fedora 44 RPM payload gate: pass on BigBoy.

## Live package and browse proof

- RPM: `magic-mesh-12.1.6-5.x86_64`.
- Size: `87,560,815` bytes.
- SHA-256: `32c4d1c70382dfe9bb517859d6467257e42b973ba97836dfd83638a959e49e28`.
- Installed with `--replacepkgs` after the visible seat-update warning on Dell
  (`172.20.146.225`) and seat 15 (`172.20.0.15`).
- `verify-music-live-seat.sh` passed on both: active daemon, `NRestarts=0`,
  package identity/payload, Bus ping, and state/list-albums replies.
- Both live providers returned 240 artists, 31 podcasts, and 53 radio
  stations. Retained collections were albums 100, artists 239, podcasts 31,
  and radio 53.
- Live detail probes on both opened AC/DC with 23 albums and
  `Wait Wait... Don't Tell Me!` with 3 episodes. The retained episode rows
  carried the feed title as `parent_title`.
- Post-install CPU proof on the two authorized seats passed the `850‰` max /
  `500‰` mean thresholds with zero restarts: seat 15 max `186‰`, mean `163‰`;
  Dell max `440‰`, mean `292‰`.

## Still open

Provider-loss continuity, physical renderer/cast proof, cross-seat handoff,
typed radio playback, and the five-seat CPU/NWS acceptance boundary remain
open. The CPU/NWS audit found Dell and seat 15 within the current bounded CPU
gate, but Eagle is on an older release, T480/Surface SSH access is denied, and
no live NWS overlay snapshots were available on Dell or seat 15.
