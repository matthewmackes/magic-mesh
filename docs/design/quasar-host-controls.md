# Quasar Host Controls — Bluetooth · Mixer · Displays · Power · Hotkeys (E12-15..E12-19)

> **Status: LOCKED 2026-07-01** — 14-question operator survey (this session), one
> question at a time per `/plan`. The host shell owns the DRM seat with no
> compositor and no settings daemon, so every "system control" a desktop OS takes
> for granted has no owner in Quasar until this epic. These are **seat services**:
> they belong to the one egui shell (§4/§5) and to `mackesd` (§9), never to a
> per-app or per-desktop layer.

## The locks

| # | Fork | Lock |
|---|------|------|
| 1 | State ownership | **Split**: the shell acts on hardware **directly, in-process** (PipeWire/BlueZ/DRM/backlight — zero-latency sliders + keys); a thin `mackesd` **`host_state` worker mirrors** status to Bus topics for Workbench planes + remote peers. Typed Bus verbs carry every *remote* control action. |
| 2 | Audio stack | **PipeWire + WirePlumber** baked into the image; existing ALSA users (`mde-voice-hud` SIP, `mde-musicd`) ride the `pipewire-alsa` shim unchanged. |
| 3 | Placement | **`Surface::System`** (dock) owns ALL interaction; the **chrome bar gets read-only iconic status: Signal · Bluetooth · Volume**. No chrome-embedded controls. |
| 4 | Mixer | **DAW-authentic full mixer**: channel strips (fader + VU meter + mute + solo) + a master strip. Strips cover **the local host session (musicd/voice/apps) · every local VM session · mesh-remote volume streams** (other peers' strips rendered from their `host_state` mirror, controlled via typed verbs). |
| 5 | Bluetooth | **Full pairing manager** (adapter on/off, scan, pair w/ PIN-passkey dialogs, trust, connect, forget, auto-reconnect at seat start) **+ Android-style proximity announce**: a nearby pairable device raises a one-tap "Found X — Pair?" popup. *Honest scope: BlueZ discoverable/pairable advertisement, not Google Fast Pair (proprietary).* |
| 6 | Displays + Power | **Full multi-head**: enumerate all connectors, enable/disable, mode (res/refresh) per output, relative arrangement — plus a **Power & Battery** section with **multi-battery support** (UPower: internal batteries, UPSes, **BT peripheral batteries**). |
| 7 | Persistence | **Hybrid**: hardware-bound state stays node-local (BT link keys, per-connector mode); **preferences roam per-peer** in mesh state (mixer levels, display *arrangement intent* keyed by the replug-stable EDID `MonitorId` from `session_roaming`). |
| 8 | Key policy | **XF86 keys are host-first, always** (XF86Audio*/MonBrightness*/Bluetooth/power); everything else reaches the focused guest unless prefixed by the **leader chord** (the Esc-chord pattern generalized). |
| 9 | Hotkeys | **Fixed compiled-in table.** No bindings UI, no hotkey roaming. Actions are **typed verbs only** (§9 — no shell-exec): volume/mute, brightness, BT toggle, session switch, monitor-focus switch, return-to-chrome, lock, surface launch, per-strip mute. |
| 10 | Mesh reach | **Full remote control**: every peer's BT/display/power/audio is visible in the Workbench (free via the `host_state` mirror) AND controllable via typed verbs — with the safety interlocks below. |
| 11 | OSD | **Rich OSD**: hotkey actions flash an egui overlay over anything fullscreen — icon + level bar + device/strip name + context (which strip moved), Motion-table fade. |
| 12 | Power actions | **Everything**: local lock/suspend/reboot/poweroff (logind D-Bus, confirm-gated) + the same as remote typed verbs (confirm handshake) + **per-VM power rows** that **reuse the Instances panel's broker verbs** (§6 glue — one implementation, two views). |
| 13 | Brightness | **sysfs backlight + DDC/CI** (i2c-dev) per-output sliders; a DDC-refusing monitor shows an honest "not controllable" state, never a dead slider. |
| 14 | Epic identity | **Folded into E12 as E12-15..E12-19**, drained in the current loop alongside the E12 remainder. |

## Architecture

```
mde-shell-egui                             mackesd
┌──────────────────────────────┐           ┌──────────────────────────────┐
│ chrome icons (RO): 📶 ᛒ 🔊    │           │ host_state worker            │
│ Surface::System              │  publish  │  · mirrors seat snapshot →   │
│  ├ Mixer (DAW strips)        │──────────▶│    state/host/<node>/*       │
│  ├ Bluetooth                 │   Bus     │  · executes remote verbs:    │
│  ├ Displays (+arrange)       │◀──────────│    action/host/<node>/*      │
│  ├ Power & Battery           │  remote   │    (allowlist + interlocks)  │
│  └ (hotkey table, read-only) │  verbs    └──────────────────────────────┘
│ rich OSD overlay             │
│ hotkey dispatch (libinput)   │           crates/desktop/mde-seat (new)
└──────────────┬───────────────┘           · PipeWire graph client (mixer)
               │ direct, in-process        · BlueZ zbus client (+agent)
               ▼                           · UPower zbus client
   PipeWire · BlueZ · logind ·             · logind client
   UPower · DRM/KMS · backlight ·          · DRM modeset + backlight + DDC/CI
   DDC/CI (i2c-dev)                        · injectable transports, typed errors
```

- **`mde-seat`** (new, `crates/desktop/`) is the hardware-access library — every
  client injectable + typed-error'd so the whole epic is testable headless
  (the mde-kvm `ChTransport` pattern). The shell consumes it directly (Lock 1);
  the `host_state` worker consumes the *same crate* for its mirror — one
  implementation of each protocol client.
- **§2 compliance**: BlueZ/UPower/logind are D-Bus — the **FDO-interop exception**
  covers all three (they are FDO/standard system services; no MDE-private bus
  names). PipeWire uses its own native protocol (not D-Bus).
- **§6 tiers**: `mde-seat` + the System surface live in desktop-shell;
  `host_state` lives in platform-services; nothing in mesh-substrate grows a
  desktop dep. Dependencies point inward only.
- **§9**: remote actions are **typed verbs + signed job bundles only** — the verb
  set IS the allowlist; there is no generic "run this on the peer".

## Safety interlocks (Lock 10's blast radius, extends §8 doc duty)

1. **Never black the last console**: a display verb that would disable a peer's
   only active connector is refused with a typed error (local AND remote).
2. **Leader-aware power**: reboot/poweroff on the current etcd leader warns and
   requires the explicit confirm flag on the verb.
3. **Remote confirm handshake**: destructive remote verbs (poweroff, reboot,
   forget-BT-device) are two-phase: propose → typed confirm within a TTL.
4. **Honest gating**: every backend probe failure (no PipeWire, no adapter, DDC
   refused, no backlight) renders as a typed "not available/controllable" state —
   never a fake control (§7).

## The units (lifted to WORKLIST as E12-15..E12-19)

- **E12-15 — `mde-seat` foundation + `Surface::System` + chrome status icons.**
  The crate skeleton (typed clients, injectable transports), the System surface
  mounted in the dock, the three read-only chrome icons fed by a seat snapshot.
- **E12-16 — the DAW mixer.** PipeWire graph client; strip model
  (host/VM/mesh-remote); fader/meter/mute/solo/master; `host_state` audio mirror
  + `action/host/audio` verbs so remote strips are live; volume OSD.
- **E12-17 — Bluetooth manager.** BlueZ client + pairing agent (PIN/passkey
  dialogs), scan/pair/trust/connect/forget, proximity-announce popups,
  auto-reconnect at seat start, BT chrome icon, remote BT verbs.
- **E12-18 — Displays + Power.** Multi-CRTC drive in the `mde-egui` DRM runner
  (the deepest engineering item — unlocks E12-10's per-monitor-VM demo);
  enable/mode/arrange UI; backlight + DDC/CI brightness; EDID-keyed roaming
  arrangement prefs; UPower battery telemetry (multi + peripherals); power
  actions local + remote; per-VM power rows reusing Instances verbs.
- **E12-19 — Hotkeys + OSD + `host_state` worker + packaging.** The fixed key
  table (XF86 host-first + leader chord) dispatching typed actions; the rich OSD
  overlay; the `host_state` mirror worker + remote-verb allowlist/interlocks;
  pipewire/wireplumber/bluez/upower (+i2c tooling) into the RPM requires and the
  bootc Containerfile.

**Serialization note**: E12-15 lands first (it owns the dock/`main.rs` wiring —
the same-file trap); E12-16/17/18 then parallelize on its base (each owns its own
panel module); E12-19 last (it touches the shell input path + packaging).

## Acceptance (epic-level, each runtime-observable)

1. A BT keyboard pairs from the System panel (PIN dialog on the DRM seat), types
   into a VM session, and reconnects by itself after a reboot.
2. A nearby pairable speaker raises the announce popup; one tap pairs + routes
   audio to it; its battery % appears in Power.
3. The mixer shows live strips for musicd + a running VM session + a remote
   peer's stream; each fader/mute/solo acts; the master meter moves; a remote
   strip's fader controls the other node (verified on the second machine).
4. Two monitors: arranged in the panel, different VMs on each (with E12-10),
   per-output brightness works (backlight on the panel, DDC on the external),
   arrangement re-applies EDID-keyed after replug and roams to a second
   workstation.
5. XF86 volume/brightness keys act with the rich OSD over a fullscreen guest;
   every other key reaches the guest; the leader chord switches sessions/locks.
6. Disabling a peer's only console is refused typed; rebooting the etcd leader
   demands the confirm flag; all remote verbs refuse outside the allowlist.

## Risks / out of scope

- **Risks**: DDC/CI flakiness (mitigated by honest not-controllable states);
  PipeWire on the bootc image (+~15 MB, two more services in the preset);
  multi-CRTC DRM complexity (atomic modeset across CRTCs; single-GPU assumption
  for v1); BT agent UX on a bare seat (dialogs must render over any surface);
  pipewire-rs / zbus version pins vs the 1.94 toolchain.
- **Out of scope (explicit)**: configurable hotkeys (Lock 9), Google Fast Pair
  proper, per-app EQ/effects, a11y (deferred per §4), Wayland anything, network
  config (CONNECT owns the boundary; the Signal icon is read-only status).
