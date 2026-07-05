# DEVMGR — a Device-Manager hardware inspector in the About surface

Operator-locked 2026-07-04 (25-Q survey). Turn the **About surface**
(`mde-shell-egui`, `Surface::About`, QBRAND-6) into a **Windows-Device-Manager-style
hardware inspector** — a faithful categorized device tree, rendered in Quasar dark,
with the ability to **switch which host you're inspecting** across the whole mesh
(nodes, cloud instances, paired phones, LAN devices, VyOS/routers). Each node
self-enumerates its hardware and publishes it to the substrate; the panel reads any
host's tree, shows rich per-device properties, flags problems MDM-style, and (in
Phase 2) drives real device actions over the overlay.

## Locked decisions (25)

| # | Area | Lock |
|---|------|------|
| 1 | Fidelity | **Faithful MDM layout, Quasar-skinned** — the exact Device-Manager tree/columns/toolbar/interaction, rendered in `mde_egui::Style` dark tokens (not literal Win chrome, not a loose homage). |
| 2 | Placement | **Device Manager fills the About body** — the brand/version shrinks to a compact title strip; the device tree is the surface body. |
| 3 | View modes | **By type + By connection + By node** — the two classic MDM modes plus a mesh-native "By node" mode that flattens every host's devices into one cross-fleet tree. |
| 4 | Categories | **Full Linux taxonomy** — CPU, Memory, Disk drives + storage controllers, Network adapters, Display/GPU, USB controllers, PCI/system devices, Audio, Input, Sensors/thermal, Bluetooth, Battery/power. |
| 5 | Host picker | **Left host rail + tree on the right** — a persistent left column lists all hosts (status dots); selecting one shows its device tree. Layout = rail │ tree │ (bottom drawer). |
| 6 | Host types | **Mesh peer nodes + Cloud/Nova instances + paired phones + LAN-discovered hosts + VyOS/router devices** (local always included). |
| 7 | Data path | **Hybrid: published snapshot + live refresh** — instant load from each node's published inventory, plus an on-demand "refresh live" per host. |
| 8 | Freshness | **Snapshot + Scan action + ~30s auto-refresh** — a "Scan for hardware changes" button (MDM's scan) + periodic republish; not a live stream. |
| 9 | Properties | **Bottom detail drawer** — selecting a device slides a detail drawer up from the bottom; the tree stays full-width above it. |
| 10 | Property tabs | **General + Driver + Details(sysfs/IDs) + Events + Resources** — the full MDM tab set, mapped to Linux. |
| 11 | Status | **Full MDM problem-code parity** — Windows-style problem codes (Code 10/28/43…) mapped from Linux states (no driver bound, disabled, dmesg errors, degraded), shown in the drawer + as tree badges. |
| 12 | Actions | **Rescan bus + Enable/Disable + Reload kernel module + Properties/Copy-info** — the Linux-real equivalents of MDM's Scan/Disable/Update/Properties. |
| 13 | Remote actions | **Any host, destructive armed** — actions fire on whichever host is selected (local or remote over the overlay); destructive ones sit behind typed-arming. |
| 14 | Arming | **Typed-arming** — reuse the platform's typed-arming confirm (type the device/host name) for disable/unbind/reload, matching EXPLORER-5 / Console power ops. |
| 15 | Backend | **Hybrid sysfs/udev + lshw** — /sys + libudev for the fast live tree (+ pci.ids/usb.ids names); lshw/dmidecode shelled out for deep DMI/firmware details on demand. |
| 16 | Worker | **Extend an existing inventory worker** — fold device enumeration + publish into `legacy_inventory`/`compute_registry` (not a brand-new worker), reusing what fits. |
| 17 | Search | **No search box** — browse via the category grouping + expand/collapse. |
| 18 | Expand default | **All collapsed** on open (most MDM-faithful). |
| 19 | Chrome | **Full menu bar (Action / View / Help) + toolbar** — the faithful MDM chrome. |
| 20 | Summary | **Rich per-host header card** — hostname, OS/kernel, uptime, CPU/RAM/disk totals, device count + problem badge, above the tree. |
| 21 | Notify | **Yes — fleet-wide fault notify** — a device faulting on ANY node posts to the mesh notify feed (→ Chat + phone), debounced (mirrors node_grade's D/F alert). |
| 22 | Non-PC hosts | **Same tree, applicable categories only** — a VyOS router shows Network/System/Firmware; a phone shows Power/Sensors/Radios; a Nova instance shows virtio devices; a LAN host shows what's remotely detectable. No empty categories. |
| 23 | Export | **JSON + Markdown report + clipboard** — export the current host's inventory as machine JSON and a human-readable report. |
| 24 | About info | **Title strip + ⓘ button** — a compact `◈ Magic-Mesh Quasar v<ver>` strip always visible; an ⓘ button opens license/credits/mesh-identity in a dialog. The tree fills the body. |
| 25 | Phasing | **Inspector first, actions + reach next** — P1 = a complete READ-ONLY inspector across local + mesh nodes; P2 = actions (armed, any-host), fleet notify, By-node view, non-PC host types, cross-fleet reach. |

## Architecture

### Producer — the device-inventory in an existing mackesd worker (DEVMGR-1, DEVMGR-9)
Extend an existing inventory worker (`legacy_inventory` / `compute_registry`) — do NOT
mint a new worker (#16):
- **Enumerate** the local host's hardware: sysfs/udev for the device graph
  (`/sys/bus/{pci,usb,…}/devices`, libudev props), the `pci.ids`/`usb.ids` databases
  for human names, and `lshw -json` / `dmidecode` shelled out for deep DMI/firmware
  details on demand (#15). Build the **full Linux taxonomy** (#4), each device carrying
  `{name, vendor, model, ids(vendor:product), sysfs_path, driver/module + version,
  status + problem_code, resources(irq/io/mem), recent events(dmesg/udev)}`.
- **Publish** the tree to the substrate at `<workgroup_root>/device-inventory/<hostname>.json`
  (the same SEC-5 mesh-shunt replication node_grade uses — every peer reads every host's
  inventory). A **smoothed/periodic republish** + an on-demand live re-query seam (#7/#8).
- **Fault notify** (#21, P2): on a device transition INTO a problem state (driver drops,
  disk I/O errors, NIC down), emit an `event/notify/<source>` alert (the CHAT-FIX-2
  producer) so it reaches Chat + the phone. Debounced against flapping (mirror node_grade).
- Runs on **every node** (rank-0 — each node enumerates its own hardware best).

### The shared inventory schema (§6 boundary)
The `device-inventory/<host>.json` **JSON is the contract**. mackesd is mesh-side and
`mde-shell-egui` is desktop-side — neither may depend on the other (§6). Put the serde
schema in a **mesh-neutral shared crate** both can depend on (a `crates/shared/*` types
crate — e.g. alongside the existing status/telemetry schema the dock's status inputs
already carry), or duplicate a small serde struct on each side **with a round-trip test**.
Reuse the substrate-read path the shell already uses for node-grade / status inputs.

### Consumer — the About → Device Manager shell (DEVMGR-2..6, 10, 11)
In `mde-shell-egui`, a new `device_manager.rs` module rendered as the body of
`Surface::About` (the brand → a title strip, #2/#24):
- **Chrome** (#19): a full menu bar (Action / View / Help) + a toolbar (Scan, the
  View-mode toggle Type/Connection/Node, Expand/Collapse-all).
- **Left host rail** (#5): lists every selectable host (#6) with a status dot + the
  "you are here" local marker; selecting one loads its inventory (hybrid snapshot + a
  live-refresh button, #7; Scan + ~30s auto-refresh, #8; honest dim/stale/offline states).
- **The tree** (#1/#3/#4/#18): the faithful MDM device tree in Quasar-dark tokens, three
  view modes (By type default, By connection = the PCI/USB topology, By node = cross-fleet),
  all-collapsed default, category grouping, per-device **problem badges** (#11 — Linux
  state → MDM problem code).
- **Rich header card** (#20): host · OS/kernel · uptime · CPU/RAM/disk totals · device
  count + problem badge.
- **Bottom detail drawer** (#9/#10): selecting a device slides up a drawer with the
  General / Driver / Details / Events / Resources tabs.
- **Export** (#23): the current host's inventory → JSON + a Markdown report + clipboard.
- **Non-PC hosts** (#22): one tree, only the categories that apply per host type.
- All colours/metrics/motion via `mde_egui::{Style,Motion,fonts}` (§4 — no raw hex /
  literal durations); glue over the existing About/status machinery (§6).

### Actions + reach (DEVMGR-7, 8) — P2
- **Local actions** (#12): Rescan bus (`echo 1 > /sys/.../rescan`), Enable/Disable
  (driver bind/unbind, `ip link up/down`, USB `authorized`), Reload module (rmmod +
  modprobe), Properties/Copy-info. Destructive ones behind **typed-arming** (#14).
- **Remote actions** (#13): the same actions on a selected remote host, dispatched over
  the Nebula overlay — reuse the existing remote-exec seam (the mackesd run-command / QC
  verb / TERM remote path), typed-armed, appended to the audit log.

## Acceptance (runtime-observable; per task, no stubs — §7)
- Opening **About** shows the device tree filling the body, brand as a title strip + an
  ⓘ dialog; the tree is the faithful MDM layout in Quasar-dark, all-collapsed, with the
  menu bar + toolbar + rich header card.
- A **device-inventory worker** on each node publishes the full-taxonomy device tree to
  `<root>/device-inventory/<host>.json`; the panel reads the local + any mesh node's tree
  (hybrid snapshot + live refresh; Scan + auto-refresh; honest stale/offline).
- The **host rail** switches hosts; **By type / By connection / By node** all render;
  selecting a device opens the **bottom drawer** with all five tabs; problems show as
  **MDM problem codes** + tree badges.
- **Export** produces JSON + a Markdown report. (P2) **Actions** fire on local + remote
  hosts, destructive ones typed-armed; a device fault on any node **posts to the notify
  feed**; **non-PC hosts** (Nova/phone/LAN/VyOS) render with only applicable categories.
- Everything through `mde_egui` `Style`/`Motion` tokens (§4); the worker is rank-0; the
  shell↔worker contract is the published JSON (§6, no cross-boundary crate dep).

## Risks
- **§6 boundary for the schema** — the producer (mackesd) and consumer (shell) can't share
  a desktop/mesh crate; keep the contract in a neutral shared crate or duplicate-with-a-test.
- **Remote actions are live-destructive** — disabling a NIC / unbinding a driver on a
  *remote* node over the overlay can strand it. Typed-arming + read-only-by-default posture
  + an audit-log append are the guardrails; consider a "you may lose reach to this host"
  warning for network devices.
- **Problem-code parity is synthetic** — Windows problem codes don't exist on Linux; the
  mapping (no-driver→Code 28, disabled→Code 22, error→Code 10) is a faithful *emulation*.
  Keep it honest (show the real Linux reason beside the code).
- **lshw/dmidecode dependency + latency** — deep details shell out; the RPM should
  `Requires: lshw` (+ pci/usb ids) and the tree must stay responsive (sysfs-first, lshw
  lazy). Cf. the browser's excluded-heavy-engine packaging discipline.
- **Non-PC inventory is shallow** — a VyOS router / LAN host exposes little; render the
  honest partial tree, never fabricate categories (§7).
- **Serialize the shell units** — DEVMGR-2..6/10/11 all touch the new `device_manager.rs`
  + the About mount; land them in sequence on the settled base (like the VDOCK churn),
  not in parallel.

## Out of scope (v1)
- Driver *installation* / firmware *flashing* (MDM's "Update driver") — inspect + bind/
  unbind/reload only; no package/firmware mutation.
- A live hotplug stream (snapshot + Scan + auto-refresh instead, #8).
- Historical inventory diffing / a hardware-change timeline (export is the only record).
- Configurable categories/thresholds (ship the locked taxonomy).

## Tasks → `docs/WORKLIST.md` DEVMGR-1..12 (P1: 1–6, P2: 7–12).
