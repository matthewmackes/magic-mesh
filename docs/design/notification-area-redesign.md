# Notification & Status Area — re-envisioned (design)

*Operator survey 2026-07-05 (10 ideas → 50-Q `/plan`, session 759c1f91). Carbon design
language throughout. **Chat owns all notifications** — the dock conveys STATUS, not a
second notification channel.*

## The problem (grounded in the live code)

Today's dock status area tells the mesh story **four times** (Status dot · Signal dot ·
Peers dot · the NODE-GRADE A–F list), every cell carries a redundant **glyph + colored
dot**, notifications leak across **three channels** (the Chat feed, transient toasts/OSD,
and the tray badge), and there are **two code paths** (`tray.rs` flyouts vs `dock.rs`
VDOCK quads). The information set is duplicated and unclear.

## The re-design in one line

An always-calm **4-pip health bar** (Device · Mesh · Power · Alerts) that **expands** to
the detail, with **Chat as the single notification home** and a reserved **critical edge
light-show** as the only push.

## The 50 locks

### Structure & spine
| # | Lock |
|---|---|
| Q1 | **Spine = one health tile that expands** (idea #6): compact = the bar, click a chevron for detail |
| Q6 | The bar **replaces BOTH VDOCK status quads** (the 8-cell grid → 4 pips) |
| Q13 | In the ~48px vertical dock it's a **vertical stack of 4 segment pips** |
| Q5/Q9/Q11 | The four segments: **Device · Mesh · Power · Alerts** (multi-segment health bar) |
| Q49 | Dock bottom zone top→bottom: **[local A–F grade] → [4-pip bar] → [system quad]**; the system quad is **restyled to match** the new pips (actions unchanged) |
| Q43/Q20 | Code home = **one dock `status` module + the edge-cue renderer + a Chat lane**; `tray.rs` is **retired** into it |

### The pips
| # | Lock |
|---|---|
| Q24 | Each pip = **tiny Carbon glyph + color tint** (identifiable by shape, colorblind-safe) |
| Q15/Q45 | A healthy segment **dims to a faint baseline** (stays present); resting = **four faint dim glyphs**, stable geometry |
| Q14 | **Segment click routes** (Mesh→Mesh view, Alerts→Chat, Device→sliders, Power→System); a **separate chevron expands** |
| Q21 | **Mesh** tint = worst peer grade + own reachability |
| Q23 | **Device** tint = worst device issue; click → the expansion sliders |
| Q22 | **Power** tint = battery status; the VDOCK-4 Power **action menu stays separate** |
| Q11/Q19 | **Alerts** tint = Chat's highest waiting severity; **returns to dim baseline on all-read** |
| Q12 | The **Sessions** indicator is **dropped** (a connected desktop is navigation, not status) |
| Q46 | **Volume** = media keys (→ OSD pill) + the expansion slider; **no always-visible volume cell** |

### The expansion
| # | Lock |
|---|---|
| Q7/Q17 | A **right slide-out panel** from the dock edge: **local grade + peer grades + device sliders** |
| Q18 | Only **this node's single A–F grade** is always-on; **peer grades live in the expansion** |
| Q27 | Motion = **slide + fade** from the dock edge (Carbon fast) |

### Color & motion
| # | Lock |
|---|---|
| Q25 | **Carbon semantic status tokens** (support-error / support-warning / support-success / support-info) — added to `mde-egui::Style` once |
| Q26 | Segment transition = **smooth fade + ONE attention pulse on worsening** (improving just fades) |
| Q28 | Severity taxonomy = **Red = alert (action needed) · Amber = warning · Blue = info** |

### Chat owns notifications
| # | Lock |
|---|---|
| Q3/Q16 | **Notification toasts are killed** (→ Chat); the hardware **OSD stays** as a **centered Carbon pill** (vol/brightness), never a notification |
| Q4/Q50 | The dock is **count-free** — Alerts is a **severity pip, no number**; the **unread COUNT lives only inside Chat** (its icon/lane header) |
| Q29 | Notifications appear as messages **from the originating host contact** (NOTIFY-CHAT host-as-contact) |
| Q30 | Chat gains a **dedicated Notifications lane** (severity-colored, newest-first, filterable) beside the roster |
| Q31/Q35 | Notifications carry **typed action buttons** — a **fully-configurable action set**; destructive actions **typed-armed** in the message |
| Q32 | **Debounce + coalesce** a flapping source into one updating item |
| Q36 | **Ack clears** it; a **live condition re-raises** if still true; snooze = quiet N min |
| Q38 | History = **session-only** (cleared on restart; distinct from durable peer chat messages) |
| Q39 | Config = **inline in the Chat workspace** (per-source mute, severity filter, DND) |
| Q40 | **DND** mutes the sliver + pulses; the feed still fills silently + the Alerts pip stays live |
| Q33 | Sources = the **comprehensive set**: mesh peers, `systemctl --failed`, dnf updates, disk/SMART, journal WARN+, cloud (QC-20), node-grade D/F drops |
| Q47 | **Extend the existing CHAT-FIX-2 notify worker** with severity tagging + coalescing + the per-segment rollups (no new worker) |

### The critical cue (the one push, toasts being gone)
| # | Lock |
|---|---|
| Q34/Q37 | A critical fires an **all-edges Carbon-red pulsing light show** (not a toast card — an ambient edge cue); **criticals only**, warnings/info are pull-only (pip + Chat) |
| Q42 | It **pulses a few cycles then settles to a held thin edge glow** while the condition is live; clears on resolve/ack |
| Q41 | Cross-node: the light show fires on the **affected node's own seat**; **other seats get the Alerts pip + a Chat entry** (from that host's contact), not the light show |

### A11y & scope
| # | Lock |
|---|---|
| Q44 | **Full accesskit live-region NOW** for the pips/alerts — an operator override of §4's deferred-a11y, for this epic |
| Q48 | Ship as **ONE epic** — no partial notification area (bar + lane + edge-cue + DND + a11y + actions land together) |

## Architecture

```
  mde-egui::Style ── Carbon status tokens (support-error/warning/success/info) + fade/pulse motion
        │
  mde-shell-egui `status` module (retires tray.rs)
        ├── local A–F grade pip
        ├── the 4-pip bar (Device·Mesh·Power·Alerts) — folds the daemon's per-segment rollups
        ├── chevron → right slide-out expansion (local+peer grades + device sliders)
        ├── the critical edge-cue renderer (all-edges pulse → held glow; own-seat only)
        └── accesskit live-region (pips + alerts announced)
  mde-shell-egui chat.rs ── the Notifications lane (host-contact sourced, severity-colored,
        session-only, typed configurable actions, inline config+DND; unread count on the Chat icon)
        ▲
  mackesd notify worker (CHAT-FIX-2, extended) — severity tagging (R/A/B) · debounce+coalesce ·
        per-segment worst-severity rollups · the comprehensive source set · cross-node critical policy
```

The OSD (vol/brightness centered Carbon pill) is a separate non-notification affordance.
The system quad (Settings/Show-Desktop/Lock/Power) is restyled to the pip language, actions unchanged.

## Acceptance (each runtime-observable)

- The dock shows a calm 4-pip bar; a segment tints (with a pulse) when its domain worsens and dims when healthy; the whole bar is count-free.
- Clicking a segment routes (Mesh→Mesh, Alerts→Chat, Device→sliders, Power→System); the chevron slides out the grade+sliders panel.
- Every notification source lands in the Chat Notifications lane as a message from its host contact, severity-tinted, coalesced; the Chat icon carries the unread count.
- A notification's typed actions fire (destructive ones armed); ack clears, a live condition re-raises; DND mutes the push but not the feed.
- A critical fires the all-edges light-show on the affected node's seat only; other seats see the pip + Chat entry.
- Volume/brightness show the centered Carbon OSD pill; no notification toast appears anywhere.
- `tray.rs` is gone (one status module); the pips announce via accesskit.

## Risks

- **The one-push exception**: the critical edge-cue is a deliberate re-introduction of "push" after killing toasts — keep it strictly critical-only and ambient (no text card) or it becomes the toast by another name.
- **Session-only history** means a critical that fired while you were away and then resolved leaves no durable record — acceptable per Q38, but document it (the durable record is peer chat, not notifications).
- **accesskit-now** pulls deferred a11y forward for one surface; keep it scoped to the pips/alerts, not a platform-wide a11y commitment.
- **Store/rollup coupling**: the bar folds daemon rollups — the shell must degrade honestly (dim/pre-poll) when the worker is absent, never fake green.

## Out of scope

- Persistent/searchable notification history (session-only by Q38).
- Platform-wide accesskit (only the notification pips/alerts this epic).
- Changing the VDOCK-4 Power **action** menu (only a restyle, Q22/Q49).
