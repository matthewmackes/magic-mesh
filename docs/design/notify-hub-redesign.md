# Notification Hub redesign + mesh Clipboard (NOTIFY-HUB / CLIP)

Goal: make the **Notification Hub** flow with the redesigned **Start Menu**
(APPS-STYLE-2), add a synced **Clipboard Viewer**, and animate the list.
Target: `mde-notify-center` (the Hub) + a clipboard mesh-sync path. Survey locks
(5 questions, 2026-06-18):

| # | Decision | Lock |
|---|----------|------|
| 1 | Clipboard sync | **Auto-sync every copy** — every local text clip broadcasts to all nodes (operator accepted: incl. anything that looks like a password). |
| 2 | Clipboard data | **Text only, rolling last 50, persisted on QNM-Shared** (`/mnt/mesh-storage/clipboard/`), visible mesh-wide, survives reboot. |
| 3 | Clipboard actions | **Click = copy to this node** + per-entry **delete** + **pin** (pinned survive the cap) + **Clear all**. |
| 4 | Hub style | **Full Start-Menu adoption** — Carbon header, sectioned zebra rows, same row/avatar/accent styling + sizing, light + dark via mde-theme. |
| 5 | Animation | **Full set** — a new item slides in from the right + blinks 2× in its severity colour; existing items slide down; same-source notifications **stack** (collapse to one card with a count, expandable). |

## Hub section order (top → bottom)
Notifications list (animated) → **Clipboard Viewer** → Music Player → SIP Phone
status. (Operator: clipboard is "the very last area above the Music Player and
SIP Phone status".)

## Architecture
- **CLIP-SYNC** — a clipboard mesh-sync path: watch local clipboard changes
  (wl-paste/clipd), publish each text clip on a bus topic + append to the
  QNM-Shared history (`clipboard/history.json`, last 50 + pinned). All nodes tail
  it. Click in the viewer → `wl-copy` to this node; delete/pin/clear edit the
  shared history (leader-safe write). **No secret filtering** (auto-sync-all lock).
- **NOTIFY-HUB** — rebuild `mde-notify-center`'s view with the Start-Menu idiom
  (header + zebra sectioned rows + Carbon tokens, light+dark) and a tick-driven
  animation layer (per-item slide-in offset + blink phase; insert pushes others
  down; same-source collapse with a count).

## Worklist
- NOTIFY-HUB-1: Start-Menu-idiom restyle (Carbon, zebra, sections, light+dark).
- NOTIFY-HUB-2: animations (slide-in + 2× severity blink + slide-down + same-source stack).
- CLIP-SYNC-1: mesh clipboard sync (auto-broadcast + QNM history, 50 + pinned).
- CLIP-VIEW-1: Clipboard Viewer section (click=copy / delete / pin / clear-all), above Music/SIP.

## Acceptance (runtime-observable)
- Hub renders the Start-Menu look (zebra rows, Carbon, light+dark); sections in the locked order.
- A copy on node A appears in the Clipboard Viewer on node B within the sync window; clicking it on B loads B's clipboard; delete/pin/clear reflect mesh-wide.
- A new notification slides in + blinks 2× in its severity colour; others slide down; repeated same-source notifications collapse to a count.
