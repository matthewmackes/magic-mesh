# WL-FUNC-028 leftover — Surface live Construct Transfers — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:55Z`–`2026-08-25T10:59:30Z`  
Classification: leftover-honesty / live-seat CLI+store+keystroke; **not**
readable Transfers labels, **not** Save/Remove, **not** a pair add, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`)  
`production_admitted: false`

SSH as `root@` with `/root/.ssh/mackes_mesh_ed25519`. No `seat-update-warning`.
No Sunshine (binary absent). No `seat-remote-input` uinput node (Construct
libinput does not hotplug those; Health Fix r1). No pair add. No inbox
verb. Did not SSH Seat 15 or Dell. Dell leftover-028 was not copied here.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-028` leftover (live
  Construct Transfers paint/click).
- Prior Seat 15 empty-store probe:
  `WL-FUNC-028-2026-08-24-live-transfers-r1.md`.
- In-tree CLI-parity editor (now installed on this dest-cut):
  `WL-FUNC-028-2026-08-23-transfers-editor-cli-parity-r1.md`.
- Sibling Surface kmsgrab (no PNG, no keystroke):
  `WL-FUNC-025-2026-08-25-surface-files-posix-r1.md`.

## Seat identity

| Field | Value |
|---|---|
| RPM | `magic-mesh-13.0.0-35.x86_64` (buildtime 2026-08-24 15:44:23 EDT) |
| `mackesd --version` | `13.0.0 "Construct" · 4071ed295e18a8bd117cea5ee639eb5cafab3485 · 2026-08-24 · dev` |
| sha256 shell | `2658ea3f142750646b98798e18bb35dad6e35b3eed8c3a5cf592193e32d2fe91` |
| sha256 mackesd | `43cec1fb621e0e9fb9e0015ac82f1341d15ab0a1a2f0e0ebaf0987ebf4cbea54` |
| Construct | pid `1290917` `root`, active since 2026-08-24 20:25:57 EDT |
| DRM | pid holds `/dev/dri/card1`; eDP-1 `2736x1824` connected/enabled |

This dest-cut includes `43459f809`. Packed shell literals: `No sync pairs
saved.` 1, `invalid sync pair id` 1, `malformed pair id` 0,
`transfer sync-pair add: queued` 1, `New transfer` 3.

## CLI / store

`mackesd transfer sync-pair list` printed `no sync pairs saved`. `--json`
printed `[]`. Inbox exists (`root:root` `1777`, empty). `sync-pairs/` empty.
Worker `state/mackesd/data/workers/transfers` `running` on `peer:SURFACE`
(generation advanced through the probe; pair ids none).
`event/notify/transfers` absent.

Refuses (no inbox write):

| Command | Result |
|---|---|
| `--interval bogus` | `malformed interval \`bogus\`` rc 1 |
| `--id '../etc'` | `invalid sync pair id \`../etc\`` rc 1 |
| `--interval 0` | `malformed interval \`0\`` rc 1 |
| `remove leftover-028` | `no sync pair \`leftover-028\` in the store` rc 1 |

## Paint / keystroke

`ffmpeg -f kmsgrab -device /dev/dri/card1 -crtc_id 56` downloaded the live
primary plane as XR30 (`30335258`) Intel X-tiled modifier
`0x100000000000001`, 2736×1824, 19961856 bytes. Labels are not readable
from that dump (same tiled-XR30 gap as FUNC-025). Sunshine / grim /
Moonlight absent.

Construct already holds SONiX keyboard `/dev/input/event3`. Ctrl+J then
Ctrl+N were written to that fd (not a new uinput node). Plane SHA-256:

| Frame | SHA-256 | After |
|---|---|---|
| 0 | `f23043240d439f8d94713edd19cf6f674f8dbf774f96b20c3e7b8ce657961664` | before keys |
| 1 | `02356002647972adf3a3a6703a95d3636ad67cd106075660dfdc076fdba8c28a` | Ctrl+J |
| 2 | `eecca43818d549d3a9f8a9deec00dfda350d1c54712749bb7c55f76a59f7febd` | Ctrl+N |

Store stayed `[]`. `action/voip/get-gateway` count stayed 1 (Activity
auto-get did not fire). No `OpenTransfers` journal line.

## Non-claims

- Transfers copy (`No sync pairs saved.`, New transfer editor) was not
  read from pixels.
- Save / Remove were not clicked. No pair was added.
- Dell leftover-028 is not on this seat.

## Leftover / blocker

Still open. Closing S2 needs readable Communications Transfers paint
(detiled XR30 or a capture dest) plus a Save/Remove that does not invent
a dest. CLI-parity is packed and the empty-store CLI refuses. Ctrl+J/N
reached Construct (plane hashes moved) but that is not labeled paint.
Do not flip `production_admitted`.
