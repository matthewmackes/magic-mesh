# The Desktop Chooser — a picker for every discovered desktop

*Operator directive 2026-07-02: "When remote Desktops, Local KVM Desktops, Spice, VNC,
RDP are discovered, they must be presented via a Chooser." Locked via a 15-question
survey.*

A shell surface that aggregates **every** discovered desktop source — remote mesh-peer
desktops, local KVM (libvirt/QEMU-KVM) VM consoles, and RDP/VNC/Spice endpoints — into
one live card grid and lets the operator pick + connect. It composes with the BRAND-1
empty-desktop backdrop (cards float over the centered logo) and drives the existing
VDI attach path (`vdi.rs` `ConnectRequest`/`Session`, `mde-vdi-rdp`/`-vnc` + a new
`mde-vdi-spice`).

## Locks (15-Q survey)

| # | Question | Decision |
|---|---|---|
| 1 | Placement | **Auto-popup on new discovery** — the Chooser surfaces itself when a new desktop source appears (also the default over the empty-desktop backdrop + a dock/Workbench action). |
| 2 | Layout | **Grid of live-thumbnail cards** — name, node, protocol badge, status pip, click-to-connect. |
| 3 | Grouping | **One unified view, grouped by node/host**, protocol as a per-card badge. |
| 4 | Spice | **New `mde-vdi-spice` crate** (Spice→egui), matching `mde-vdi-rdp`/`-vnc`. |
| 5 | Discovery | **All** — mesh registry (peers advertise desktops) + mDNS (LAN RDP/VNC/Spice) + local KVM enumeration (libvirt/QEMU-KVM) + manual host:port add. |
| 6 | Protocol | **Always ask which protocol** when a source offers several. |
| 7 | Thumbnails | **Periodic preview thumbnails** (peer-published / local-VM framebuffer snapshot / cheap probe) with a graceful protocol/OS-icon fallback. |
| 8 | Auth | **Mesh-identity SSO for peers** + **saved creds sealed in the secret store** (reuse FILEMGR-6 sealing) for external RDP/VNC/Spice. |
| 9 | Connect UX | **Choose fullscreen or windowed per connection** (fullscreen under the thin chrome bar = the E12 VDI idiom). |
| 10 | KVM lifecycle | **Full** — start/stop/pause a local VM + open its console, from the card (drives the mackesd `vm_lifecycle` worker). |
| 11 | Card actions | **Full** — connect, favorite/pin, edit, remove (manual sources), (KVM) power. |
| 12 | Multi-monitor | **Per-connection choice** — span all displays or a single one. |
| 13 | Find | **Full** — search + filter (node/protocol/status/OS) + sort. |
| 14 | Offline | **Shown greyed with a reason, never blocking** — reachability from roster/state, no hanging probe; retry. |
| 15 | Persistence | **Synced across the mesh** — favorites/recents/manual sources bind to the mesh identity and follow the operator between seats. |

## Architecture

- **`mde-shell-egui` Chooser surface** (`chooser.rs`) — the card grid, grouped by
  node/host, over the BRAND-1 backdrop when empty; auto-popups on a new-source event;
  status pips + protocol badges + periodic thumbnails; search/filter/sort; per-card
  context menu. Emits `vdi.rs` `ConnectRequest`s (protocol chosen, fullscreen/windowed,
  monitor span) and `vm_lifecycle` actions.
- **mackesd desktop-source discovery aggregator** — collects sources from the mesh
  registry (peers advertise their desktops), mDNS (`mdns_relay` — RDP 3389 / VNC 5900 /
  Spice), and local KVM enumeration (`kvm.rs`/`vm_lifecycle`), plus manually-added
  sources; publishes `state/desktops/sources` with per-source protocol(s), reachability,
  node, OS, and thumbnail refs. §6 mesh-side.
- **Protocol clients** — `mde-vdi-rdp` (ironrdp, exists), `mde-vdi-vnc` (exists), + a
  new **`mde-vdi-spice`** (Spice→egui; airgap-verify the Spice crate, honest VNC
  fallback if unfetchable).
- **Auth** — mesh-peer desktops use the node's mesh identity (no separate login);
  external endpoints' creds sealed in the secret store (reuse FILEMGR-6's seal/derive).
- **Thumbnails** — mesh peers publish a small periodic snapshot; local VMs snapshot the
  framebuffer; external endpoints get a cheap one-frame probe; fallback to a protocol/OS
  icon.
- **Persistence** — favorites/recents/manual sources bound to the mesh identity + synced
  via Syncthing (like bookmarks / MEDIA session-roaming).

## Worklist (CHOOSER-1..9)

- [ ] **CHOOSER-1: mackesd desktop-source discovery aggregator.** Collect desktop sources from the mesh registry (peer-advertised), mDNS (RDP/VNC/Spice on the LAN, reuse `mdns_relay`), and local KVM enumeration (`kvm.rs`/`vm_lifecycle`), + manual sources; publish `state/desktops/sources` (per-source: node, protocols, reachability, OS, thumbnail ref). Unit-tested discovery/merge folds; §6 mesh-side.
- [ ] **CHOOSER-2: the Chooser surface (`mde-shell-egui`, Carbon §4).** A live card grid grouped by node/host, protocol badges + status pips, over the BRAND-1 backdrop when empty; auto-popups on a new-source event; renders `state/desktops/sources`. §4 tokens (no raw hex); egui snapshot + headless mount tests (empty, populated, offline-greyed).
- [ ] **CHOOSER-3: live thumbnails.** Periodic preview thumbnails — mesh peers publish a small snapshot, local VMs snapshot the framebuffer, external endpoints a cheap probe — with a graceful protocol/OS-icon fallback; bounded refresh + cache. Tested source→thumbnail plumbing + fallback.
- [ ] **CHOOSER-4: connect flow (protocol picker + display options).** On connect: **always-ask protocol** picker when several are offered; fullscreen-or-windowed choice; multi-monitor span/single choice; route to `mde-vdi-rdp`/`-vnc`/`-spice` via a `vdi.rs` `ConnectRequest`. Tested request construction folds.
- [ ] **CHOOSER-5: `mde-vdi-spice` client crate (Spice→egui).** A first-class Spice client rendered egui-native like the RDP/VNC crates; airgap-verify the Spice crate (add+vendor, honest VNC-fallback note if unfetchable); connects to a KVM VM's Spice console. Headless: connect→a frame arrives. New crate, workspace member.
- [ ] **CHOOSER-6: auth.** Mesh-peer desktops authenticate with the node's mesh identity (SSO, no prompt); external RDP/VNC/Spice creds sealed in the secret store (reuse FILEMGR-6 seal/derive), prompted once; never a plaintext credential on disk/logs. Tested seal/read + the SSO path.
- [ ] **CHOOSER-7: local-KVM lifecycle from cards.** Start/stop/pause a discovered local VM + open its console, driving the mackesd `vm_lifecycle` worker; card reflects live power state. Tested action dispatch + state reflection; honest-gated where no local hypervisor.
- [ ] **CHOOSER-8: card actions + find + states.** Per-card context menu (connect, favorite/pin, edit, remove manual, KVM power); search + filter (node/protocol/status/OS) + sort; offline/unreachable sources shown greyed with a reason + retry, never a blocking probe. Tested filter/sort + the non-blocking offline model.
- [ ] **CHOOSER-9: mesh-synced favorites/recents/manual sources.** Favorites, recents, and manual sources bound to the mesh identity + synced via Syncthing (like bookmarks); they follow the operator between seats. Two-seat test: pin at one seat → visible at another. §6 mesh-side.

## Acceptance (top-level, §7 runtime-observable)

- With no desktop attached, the Chooser card grid shows over the logo backdrop; a newly
  discovered source pops the Chooser and adds its card (live thumbnail, node group,
  protocol badge, status pip).
- Clicking a card asks the protocol (when several), then fullscreen/windowed + monitor
  span, then connects via the right VDI client; a mesh peer connects with no credential
  prompt; an external endpoint prompts once then remembers (sealed).
- A stopped local VM starts from its card and connects; an offline peer's card is greyed
  with a reason and never hangs the UI.
- A pinned favorite at one seat appears at another seat the operator logs into.
- Per unit: `build/test/clippy -p <crate>` green, Carbon check (no raw hex),
  `lint-layered-tiers.sh` clean.

## Out of scope

- A general remote-desktop *server* (this is the client/chooser; serving desktops is the
  VDI-broker's job).
- Session recording of remote desktops.

## Risks

- **Spice crate airgap add** (Q4/CHOOSER-5): verify early; honest VNC fallback rather
  than silently dropping Spice.
- **Thumbnail cost** (Q7): periodic previews must be cheap + bounded — never a full
  decode per card per frame; cache + throttle.
- **Non-blocking discovery** (Q14): reachability must come from roster/state, never a
  synchronous probe that stalls the grid.
