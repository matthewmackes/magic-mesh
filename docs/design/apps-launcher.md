# APPS — Re-envisioned Applications Panel (Magic-Mesh launcher on Cosmic)

> **HISTORICAL / SUPERSEDED (2026-07-19):** describes the retired `libcosmic`/Cosmic-applet launcher (`mde-cosmic-applet`). The live desktop is the egui-native, DRM-native shell `mde-shell-egui` — see [`quasar-vdi-desktop.md`](quasar-vdi-desktop.md); the launcher is now folded into the Front Door ([`app-launcher-rethink.md`](app-launcher-rethink.md)). Retained as a historical design record.

Design survey locked 2026-06-17 (25-question `/plan`). The Applications Panel is
a **Magic-Mesh app launcher that replaces Cosmic's app library** — a Start-menu-
style **panel dropdown** that launches apps **anywhere in the mesh**: local apps,
peers' published apps, containers/VMs, and mesh services. Hosted in
`mde-cosmic-applet`; the heavy lifting is a `mackesd` aggregator worker.

## Locks (25)

| # | Decision | Lock |
|---|----------|------|
| Q1 | Identity | **Replace** cosmic-app-library — the default launcher |
| Q2 | Purpose | **Mesh-wide** app launcher |
| Q3 | Form | **Panel dropdown** menu (from a cosmic-panel applet) |
| Q4 | Scope | Local apps + peer/mesh apps + containers/VMs + mesh services **+ header** (QNM disk space + links to Workbench, MDE Files, Cosmic Settings) |
| Q5 | Local sources | **XDG .desktop (all) + Flatpak** |
| Q6 | Landing | Opens to **Favorites** |
| Q7 | Layout | **Tabbed** (Favorites / Apps / Mesh / Workloads / Services) + persistent header |
| Q8 | Header disk stat | **Mesh Sync usage** (used/total of `/mnt/mesh-storage`) |
| Q9 | Peer-app launch | **Remote-desktop session** (stream the peer's app here) |
| Q10 | Favorites scope | **Synced mesh-wide** (Mesh Sync), per desktop user |
| Q11 | Context actions | pin/unpin · **launch on a chosen peer** · run containerized · details/uninstall |
| Q12 | Recents | **Favorites only** — no usage tracking (privacy) |
| Q13 | Density | **Carbon design standard** (mde-theme tokens, §4) |
| Q14 | Icons | **XDG icon theme** (Carbon-brand set), app fallback |
| Q15 | Mesh marking | Mesh entries live in the **separate Mesh tab** (no per-item badge) |
| Q16 | Trigger | **Grid/apps glyph** button in the cosmic-panel |
| Q17 | Mesh source | **PD-2 service descriptors** from the replicated directory (no new transport) |
| Q18 | Health | **Show health tier, allow launch anyway** |
| Q19 | Workloads | **Inline start/stop/attach** (containers + VMs) |
| Q20 | Services | **Published mesh services**; click opens the endpoint over the overlay |
| Q21 | Crate | **Extend `mde-cosmic-applet`** |
| Q22 | Replace mech | **Baked-layout swap + Super-key** (drop cosmic-app-library; mirrors BRAND-8) |
| Q23 | Launch path | **Local exec + action-bus verbs** for mesh/workloads |
| Q24 | Boundary | **Thin applet + mackesd aggregator** worker serving `action/apps/list` |
| Q25 | MVP | **Full mesh from day one** — all tabs + remote launch + start/stop/attach together |

## Architecture

```
mde-cosmic-applet (libcosmic, panel)            mackesd (aggregator + verbs)
  ┌───────────────────────────────┐               ┌────────────────────────────┐
  │ grid glyph ▸ dropdown (Super)  │  action/apps/list │ apps_aggregator worker  │
  │  Header: QNM disk + quick links│◄──────────────────│  • local XDG + flatpak  │
  │  Tabs: Favorites│Apps│Mesh│    │                   │  • mesh peers (PD-2 dir)│
  │        Workloads│Services      │  action/apps/launch│  • workloads (podman/   │
  │  Carbon density · XDG icons    │──────────────────►│    libvirt, local+peer) │
  │  search (fuzzy, all tabs)      │  action/compute/* │  • services (PD-2)      │
  └───────────────────────────────┘  action/provision/*└────────────────────────┘
  Favorites ↔ QNM-Shared/<user>/apps-favorites.json (mesh-synced)
```

- **Thin applet:** renders + launches only. The `apps_aggregator` mackesd worker
  builds the unified entry list (local XDG+flatpak scan, mesh peers + workloads +
  services from the PD-2 directory descriptors) and serves it on
  `action/apps/list`. One source of truth, no per-tab queries from the GUI.
- **Launch:** local apps exec directly (`.desktop`/gio). Peer apps →
  `action/apps/launch` → mackesd opens a remote-desktop session to the peer
  (reusing the descriptors' advertised remote-access path). Workloads →
  `action/compute/*` + `action/provision/*` (start/stop/attach). Services → open
  the published endpoint over the overlay.
- **Favorites:** per desktop user, stored on Mesh Sync so they follow the user
  to any node. No recents/usage tracking (Q12).
- **Replace Cosmic's launcher:** the Magic-on-Cosmic baked layout drops
  `cosmic-app-library` from the panel, adds our applet's grid glyph, and binds
  Super to toggle it (mirrors BRAND-8 dropping CosmicAppletNotifications).

## Acceptance (runtime-observable)
- Pressing Super (or the panel grid glyph) drops the launcher; Cosmic's stock
  app-library is gone from the baked layout.
- Header shows live Mesh Sync used/total + working links to Workbench, MDE
  Files, Cosmic Settings.
- Apps tab lists local XDG + flatpak apps; clicking one launches it.
- Mesh tab lists peers' published apps (from the directory) with presence/health
  badges; clicking opens a remote-desktop session to that peer.
- Workloads tab lists containers + VMs and can start/stop/attach them.
- Services tab lists published mesh services; clicking opens the endpoint over
  the overlay.
- Favorites tab is the default view; pin/unpin persists and is visible on a
  second node logged in as the same user.
- Renders through `mde-theme` Carbon tokens (no raw hex/§4 lint clean).

## Risks
- **libcosmic launcher replacement** — dropping cosmic-app-library from the baked
  layout + Super-key rebind must not break the panel; verify on a Cosmic session.
- **Remote-desktop launch** depends on the peer advertising a reachable
  remote-access descriptor; degrade honestly when absent.
- **Aggregator cost** — scanning XDG+flatpak + the directory each open; cache +
  refresh-on-open, don't block the dropdown.

## Out of scope (v1)
- Usage tracking / recents (Q12 = favorites only).
- Snap enumeration (Q5).
- Per-item mesh badging in local tabs (Q15 — Mesh tab segregates them).
