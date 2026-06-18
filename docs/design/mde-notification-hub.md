# MDE-Notification-Hub — design

**Epic prefix:** `NOTIFY`
**Status:** locked 2026-06-14 (operator survey, 4 forks)
**Surface crate:** new `crates/services/mde-notify`
**Depends on:** `mde-bus` (alert lane + DND), `mde-theme` (Carbon tokens), libcosmic layer-shell (the `mde-mesh-wallpaper` pattern)

---

## 1. Why

Today a "notification" in MCNF is a forward-only path: mackesd workers
(`alert_relay`, `presence_watch`, `firewall_monitor`, `compute_event_toast`,
`metrics_exporter`, the `events.rs` hooks) drop alert JSON / publish to the bus,
and the `bus_bridge` mirrors every `org.freedesktop.Notifications` call into
`fdo/*` topics. Cosmic's daemon renders the transient toast and forgets it.
There is **no operator-facing place that shows the alert history**, groups it,
or lets the operator triage it — the Workbench `notifications.rs` panel is a
*settings* page (DND / placement / expire-ms), not a viewer. The retired
`toast_chip` was deleted (GUI-5) on the assumption "Cosmic owns rendering."

The operator wants a **professional notification center** — a desktop-wide,
themed, table-driven hub that shows the full mesh + desktop alert stream,
grouped and color-coded, with sound + motion. This is the **MDE-Notification-Hub**.

## 2. Locked decisions (operator survey 2026-06-14)

| Fork | Decision | Consequence |
|------|----------|-------------|
| **Surface** | Standalone center **+** toasts (new binary) | A layer-shell slide-out center panel opened from a panel/applet entry, **plus** transient toast popups. True replacement for the Cosmic tray. Links `mde-theme` for the look. Not gated on the Workbench window being open. |
| **FDO scope** | **Render the bus, don't own FDO** | The Hub is a *reader* of the `mde-bus` alert lane. The existing `bus_bridge` already intercepts every `org.freedesktop.Notifications.Notify` into `fdo/*` topics, so the Hub sees desktop-app notifications **and** mesh alerts without seizing the FDO D-Bus name from Cosmic. Non-invasive, reversible, and compliant with §2 (FDO interop only, no MDE-private name takeover). Cosmic's daemon keeps rendering live toasts; the Hub adds its own themed toast layer + the persistent center. |
| **Grouping** | **Group by source, color by severity** | Top-level collapsible groups per source lane (Security, Presence, Firewall, Compute, Desktop apps, Per-peer). Row accent + severity glyph encode severity via `mde-theme` status tokens. Two signals at once. |
| **Effects** | **Configurable per-group sound packs** | A settings surface: per-group sound-pack picker + per-group sound enable/mute + animation style (slide / fade / none). Bundled OGG sound packs. **All effects DND-aware** (gated by `mde_bus::dnd`). |

## 3. Architecture

```
                    org.freedesktop.Notifications (Cosmic owns)
 desktop app ─────────────────┬───────────────────────────────► Cosmic toast
                              ▼
                 mackesd bus_bridge  → bus topic  fdo/<app>
 mackesd workers ───────────────────► bus topics  peer/<h>/alerts
   alert_relay / presence_watch /                  fleet/sec
   firewall_monitor / compute_event_toast /        event/firewall/<h>
   metrics_exporter / events.rs hooks              compute/event/<h>
                              │
                              ▼  (read-only tail, cursor per topic)
        ┌───────────────────────────────────────────────┐
        │  crates/services/mde-notify  (shared lib)       │
        │  • AlertItem model + severity/source classifier │
        │  • bus tail (Persist::list_since per lane)       │
        │  • grouping + dedup + retention                  │
        │  • mde-theme severity→token map                  │
        │  • sound-pack player (cpal+symphonia, DND-gated) │
        └───────────────┬───────────────────┬─────────────┘
                        ▼                   ▼
            mde-notify-center          (toast layer)
        layer-shell Overlay panel   layer-shell Top, transient
        (slide-out, table view)     slide/fade, auto-expire
```

- **One new binary** `mde-notify-center` (auto-discovered bin in `mde-notify` or
  its own `[[bin]]`). It owns **two** layer-shell surfaces: the **center** (an
  Overlay-layer slide-out anchored right, opened on demand) and the **toast
  stack** (a Top-layer, non-interactive, auto-expiring surface). Mirrors the
  `mde-mesh-wallpaper` layer-shell pattern (`get_layer_surface`, libcosmic fork).
- **Data**: the lib tails the bus lanes with `Persist::list_since(topic, cursor)`
  on a cadence (reusing the worker poll idiom) — no new bus responder needed; it
  is a pure consumer. Bus root resolves via `mde_bus::client_data_dir()` (the
  SUBAUDIT fix — survives session-env staleness so the center reaches the live
  system bus).
- **Opening the center**: the existing `mde-cosmic-applet` gains a bell/quick
  action that publishes `action/notify/toggle` (or launches the center binary);
  the center also self-registers a small bus toggle topic so the applet pip can
  reflect unread count.

## 4. Data model

```rust
struct AlertItem {
    id: String,            // bus ULID (stable, dedup key)
    ts: i64,               // epoch seconds
    severity: Severity,    // Critical | Warning | Info | Success
    source: Source,        // Security | Presence | Firewall | Compute | DesktopApp | Peer(name) | System
    topic: String,         // raw bus topic (for drill / filter)
    host: Option<String>,  // originating mesh node, when present
    title: String,
    body: String,
    actions: Vec<AlertAction>, // BUS-2.7 action buttons (label + url), ≤5
    read: bool,
}
enum Severity { Critical, Warning, Info, Success }
```

- **Severity** derives from the alert JSON `severity` field
  (`crit`/`error` → Critical, `warn` → Warning, `info` → Info, else Info) and/or
  the bus message `Priority` (`urgent`→Critical, `high`→Warning, …) — one
  classifier function, unit-tested against both inputs.
- **Source** is classified from the topic prefix:
  `fleet/sec`→Security, `*/presence`→Presence, `event/firewall`→Firewall,
  `compute/event`→Compute, `fdo/*`→DesktopApp, `peer/<h>/alerts`→Peer(h),
  `mackesd::alert`/metrics→System.
- **Dedup** by bus ULID (`id`); the lib keeps a per-lane cursor so a restart
  replays from the retention horizon, not from zero.

## 5. Color + token mapping (§4 Carbon, no raw hex)

Severity → `mde_theme::Palette` token (single-sourced; the lib NEVER hardcodes a
hex/RGB — §4 lint-gated):

| Severity | Token | Carbon | Glyph |
|----------|-------|--------|-------|
| Critical | `palette.danger` | Red 60 | ● filled |
| Warning  | `palette.warning` | Yellow/amber | ◐ half |
| Info     | `palette.accent` | Blue 60 | ○ open |
| Success  | `palette.success` | Green | ✓ check |

Group rows use `palette.raised` backgrounds + `palette.border`; a group's accent
bar takes the token of its **highest open severity**. Spacing/typography come
from `mde_theme` density + `TypeRole` (same as the Workbench panels).

## 6. Effects (all DND-aware)

- **Sound**: bundled OGG packs under `/usr/share/mde/sounds/<pack>/<severity>.ogg`
  (≥2 packs: "Alert" prominent, "Subtle" soft). Played via a small
  `cpal`+`symphonia` helper (the AIR audio chain already links both). Per-group
  enable/mute + pack selection persisted to the settings sidecar. A `paplay`
  fallback if the device is busy. **Silent when `mde_bus::dnd` is active** unless
  the message carries `override=dnd`.
- **Visual**: toast slide-in + fade (or fade-only / none per the animation
  setting); unread badge pulse on the applet pip; center row highlight on new.
  Adaptive-budget like the wallpaper — no idle animation loop; motion only on
  events.
- **DND**: a single check against the replicated `dnd.yaml` gates BOTH sound and
  toast; the **center still fills** (history is never suppressed, only the
  interruptive surfaces are).

## 7. Settings

Fold the existing Workbench `notifications.rs` settings (DND / placement /
expire-ms) into the Hub's own settings page, and **add**: per-group sound-pack +
mute, animation style, retention window, and a "clear all / mark all read"
action. The Workbench panel becomes a thin deep-link into the Hub settings (or
is retired in favour of the Hub's gear) — decided at build time; no duplicate
state (one settings sidecar, single-sourced).

## 8. Acceptance criteria (runtime-observable, §7)

1. Launching `mde-notify-center` on a Cosmic session shows a themed slide-out
   center listing real bus alerts grouped by source, colored by severity — no
   `demo_data`, the rows come from `Persist::list_since` over the live bus.
2. A new bus alert (e.g. `mackesd publish peer/<h>/alerts …` or a real firewall
   denial) appears as a toast **and** a new center row within one poll cycle.
3. Toasts + sound respect DND: with DND active, no toast/sound fires but the
   center still records the alert.
4. Per-group sound pack + mute + animation settings persist across a restart and
   change the observed effect.
5. No raw hex / RGB literal in `mde-notify` (the §4 lint passes); every color is
   an `mde-theme` token.
6. The applet bell/pip reflects unread count and toggles the center.
7. `cargo test -p mde-notify` covers: severity+source classifier, dedup by ULID,
   DND gating, severity→token map, retention horizon.

## 9. Risks / open items

- **Two layer-shell surfaces from one app** — confirm the libcosmic fork
  supports an Overlay center + a Top toast surface concurrently (the wallpaper
  uses one Background surface; validate multi-surface early — RISK).
- **Sound asset licensing** — bundle CC0/self-authored OGG only; add to NOTICE.
- **Double-toast** — Cosmic's daemon ALSO toasts `fdo/*` app notifications. To
  avoid showing each desktop-app notification twice, the Hub toasts **mesh
  alerts only** by default and shows `fdo/*` app notifications in the center
  table (not as a second toast); operator can opt into Hub-toasting FDO too.
- **Retention** — cap the in-memory ring + honor the bus retention so a long
  uptime doesn't grow unbounded.

## 10. Out of scope (this epic)

- Owning `org.freedesktop.Notifications` (explicitly rejected — Q2).
- Cross-device (KDC) notification mirror integration — tracked separately
  (`mde-kdc-proto` notification plugin already queues; host integration is its
  own follow-on).
- Per-rule routing / external webhooks (operators wire `curl` via `events.rs`
  alert hooks; the Hub is a viewer, not an alert router).
