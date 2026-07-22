# Decision Log (ADR-style, append-only)

Every change to a governance lock (`AI_GOVERNANCE.md` §0–§10) — and any other reopened
design lock — requires an entry here: the **symptom** that justified reopening, the
**superseding decision**, and the **date**. Newest wins (§10). Append only — never edit or
delete a prior entry; supersede it with a newer one.

---

## ADR-0005 — compute inventory bus publish: on-change + slow heartbeat (2026-06-25)

- **Supersedes:** the VIRT-1 (`v5.0.0-compute.md` §1/§3) lock that `compute_registry`
  **publishes the inventory snapshot to `compute/inventory/<peer>` every 10 s tick**.
- **Symptom:** That every-tick publish predates the cross-node transport split. Today the
  `compute/inventory/<peer>` bus topic has exactly **one** consumer — *this* node's own
  Workloads source (`ipc::apps::read_local_inventory`, wired in `mackesd.rs`). Peers read
  the fleet-wide view from the **replicated `compute-inventory.json`** file on the
  QNM-Shared plane (`mde-workbench` compute panel, `probe_nmap`, `ipc::apps`), *not* from
  the bus — there is no federation subscriber, so the bus topic is per-node by design. The
  consumer only ever wants the **latest** doc, yet the worker republished a byte-identical
  body every 10 s, appending **~8 640 redundant messages per peer per day at idle** to the
  append-only Persist log (BUS-1.9 retention then has to prune them).
- **Decision:** Keep the **10 s poll** cadence — VIRT-21 state-transition events and the
  replicated `compute-inventory.json` file must stay timely, and those are cheap local
  ops. **Change only the bus publish** to **publish-on-change plus a 60 s heartbeat**: the
  body is serialized once per tick and published when it differs from the last published
  body, or when ≥60 s have elapsed since the last publish (so a freshly-pruned topic or a
  late subscriber still finds a recent doc). First tick always publishes. A running VM's
  live `cpu_pct` delta naturally keeps an *active* node publishing each tick, while an
  *idle* fleet (the common case) goes quiet between heartbeats.
- **Scope:** `crates/mesh/mackesd` `compute_registry` worker only. The bus body shape and
  the QNM-Shared transport are unchanged, so every consumer is untouched. Cadence policy is
  a pure, unit-tested helper (`should_publish`) so it stays verifiable.

## ADR-0006 — Apple HIG (as principles) is the platform design standard; two interfaces: Construct + Car (2026-07-22)

- **Supersedes:** the WIN10-HYBRID chrome lock (`win10-taskbar.md`, 2026-07-12 — itself a
  reversal of VDOCK), the SYNC 3 Car look as design authority (`auto-mode-sync3.md`,
  2026-07-20; its dark palette tokens survive as the Car appearance), the front-door
  iPadOS-home locks Q86/89 (revived and re-scoped, not resurrected as written), and the
  NAVBAR-W10 "no top status bar" decision (reversed). §4's design-language wording is
  amended in lockstep; the Browser Material-3 carve-out is deliberately **kept**.
- **Symptom:** The chrome direction had reversed twice in ten days (taskbar → vertical
  dock → Win10 taskbar) while the launcher, home, and Car surfaces each carried their own
  paradigm (Win10 Start / Spotlight panel / SYNC 3). No single standard governed the full
  platform; ~48 of 105 design docs carried competing paradigm framing (Win10/Win7/Cosmic/
  Material), and the operator's product intent — one coherent, Apple-grade experience
  across exactly two interfaces — had no doc of record.
- **Decision (operator, 50-question survey 2026-07-22):** Apple's Human Interface
  Guidelines are the design standard for the full platform, **applied as principles, not
  pixels**. Exactly two interfaces: **Construct** (iPadOS structure + macOS pointer
  manners: persistent springboard home with pages = the 8 launcher groups, no dock, no
  widgets, slim top status bar, Control Center, Notification Center, Spotlight, card
  app-switcher, full-screen-only apps, shared NavigationBar/Toolbar/Sidebar/Sheet/Popover
  components, scrim materials, dark-only, Inter type ramp) and **Car** (CarPlay-principled:
  SYNC3 dark palette kept, Dashboard-cards home, six apps, always-visible left instrument
  strip with per-frame telemetry fold, glance rules + soft in-motion limits, one-tap
  toggle). Mackes-Carbon icons kept platform-wide. Authority doc:
  `docs/design/platform-interfaces.md`. Interface-paradigm docs retired to
  `docs/design-archive/`. Win10 chrome code is deleted at cutover (no legacy flag).
  Epics: WL-UX-006 (Construct) + WL-UX-007 (Car); WL-UX-001 superseded-retired;
  WL-UX-005 folds into WL-UX-006.
- **Scope:** design authority + `mde-shell-egui` chrome + `mde-egui` shared components +
  the 17-surface adoption sweep + Car surfaces. Engine model unchanged (one surface per
  frame, DRM-native). Curtain lock security behavior and the VDI full-native-resolution
  guarantee are explicitly sacred.
