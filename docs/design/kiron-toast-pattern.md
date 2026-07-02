# KIRON — the global lower-third "chyron" toast pattern (KIRON-1..3)

> **Status: LOCKED 2026-07-02** — 12-question operator survey (this session, per
> `/plan`). "Kiron" = **chyron**, the TV-news lower-third. This is the ONE canonical
> transient-alert pattern for the platform: a lower-third news-style band that every
> surface (chat, seat/host-controls OSD, security, build-farm, compute) emits into,
> replacing the ad-hoc overlays NOTIFY-CHAT and the host-controls rich-OSD each
> rolled on their own. Foundational — it lands **before** NOTIFY-CHAT-5's toasts and
> E12-19's OSD, which both retarget to it.

## The locks

| # | Fork | Lock |
|---|------|------|
| 1 | Home | **A `toast` module in the shared `mde-egui` crate** (beside Style/Motion/widgets): a `ToastHost` the shell paints once per frame + a `Toast` model any surface constructs. One host, headless-testable, lives with the look-source. |
| 2 | Tiers | **Severity tiers (Info / Warning / Critical, reusing the existing `Severity`) for alert chyrons, PLUS a distinct transient OSD/HUD tier** for hardware feedback (volume/brightness level bar — replaces-in-place, no dismissal). Two families, one host. |
| 3 | Placement | **Lower-third chyron band** (TV-news style) for alert toasts, floating above any fullscreen guest; the **OSD level tier flashes center-bottom** (the classic volume/brightness spot), separate + instant. |
| 4 | Queue | **One-at-a-time rotating queue**: the band shows one alert for its dwell then advances; a **Critical preempts to the front** and can hold longer; a small **"N more"** counter shows backlog. The OSD level tier is separate + instant (never queued behind alerts). |
| 5 | Anatomy | **Left:** severity-colored category flag (`SECURITY`/`BUILD`/`CHAT`/…) + source icon. **Center:** source **host (hostname)** + headline. **Right:** optional action button (Open / Go to) + dwell countdown + "N more". Reads like a news lower-third, carries host identity (hostname=username) + a click-through. |
| 6 | Dwell | **Severity-scaled dwell** (Info short → Critical long / **until-acknowledged** for the highest); **hover pauses** the countdown; click-action or dismiss-"X"/swipe removes now; a Critical requires explicit **acknowledge**. Motion-table slide in/out (FAST/BASE). |
| 7 | Emit API | **A typed Bus lane `event/toast/show`** (severity, source-host, flag, headline, optional action-verb) that the shell's `ToastHost` subscribes + renders — so any node/worker (incl. mackesd + remote peers) can raise a chyron, fleet-wide. In-shell surfaces may also call `ToastHost` **directly** for the instant OSD level tier (volume). |
| 8 | Sound | **The ToastHost owns the notification sound** — the single place it fires, severity-scaled via the E12-16 mixer, suppressed by DND / mute. Every emitter gets consistent audio by emitting a toast; no double-beeps. |
| 9 | vs Chat | **KIRON is THE one toast; chat emits into it.** A new chat message / folded alert emits `event/toast/show` → the lower-third band; clicking opens that conversation. NOTIFY-CHAT Lock 13 retargets here. Durable record stays the chat ring-log; the chyron is the transient surface. |
| 10 | Suppression | Respects **DND** and a **per-VM-session focus/gaming mute** (fullscreen game → no lower-third for Info/Warning) — **but a Critical always breaks through** (safety over immersion). |
| 11 | Record | **The chyron is pure transient**; every item it shows also exists as a message in the **NOTIFY-CHAT ring-log** (its source, or a mirror). Dismissing loses nothing; no separate toast-history store (the chat IS the history). |
| 12 | Epic | **KIRON-1..3, foundational — lands before NOTIFY-CHAT-5 toasts + E12-19 OSD**, which both retarget to it. |

## Architecture

```
any node / worker / surface                shell (mde-shell-egui)
┌───────────────────────────┐             ┌──────────────────────────────┐
│ chat worker (folded alert │  event/     │ ToastHost (mde-egui::toast)   │
│  or new message)          │─ toast/ ───▶│  · queue: 1-at-a-time,        │
│ seat / host-controls OSD  │  show (Bus) │    Critical preempt, "N more" │
│ security / build / compute│             │  · dwell (severity), hover-   │
│                           │  direct     │    pause, ack-Critical        │
│ in-shell OSD (volume) ────┼── call ────▶│  · lower-third chyron render  │
└───────────────────────────┘             │  · center-bottom OSD tier     │
                                          │  · sound (mixer, DND-aware)   │
   Toast { tier, severity, source_host,   │  · DND + focus-mute suppress; │
           flag, headline, action_verb }  │    Critical breaks through    │
                                          │  · click action_verb → nav    │
                                          └──────────────────────────────┘
        the record lives in NOTIFY-CHAT (ring-log); KIRON is display-only
```

- **`mde-egui::toast`** (shared crate): the `Toast` model (tiers/severity/source/
  flag/headline/action/dwell), the `ToastHost` (queue + preempt + dwell/hover/ack
  state machine, pure + headless-tested), and the two renders — the lower-third
  chyron + the center-bottom level OSD — over `Style` + `Motion`.
- **§6/§9**: the Bus lane is `event/toast/show` (typed; the shell subscribes). Any
  mackesd worker or remote node raises a toast → fleet-wide alerts + the chat fold
  ride it. In-shell instant OSD (volume/brightness) uses the direct `ToastHost`
  call to avoid a Bus round-trip on a slider.
- **Reuse (§6)**: `Severity` from mde-notify/mde-chat; `Motion` for slides; the
  E12-16 mixer for sound; the DND/focus-mute state from host-controls/NOTIFY-CHAT.

## The units (KIRON-1..3, lifted to WORKLIST)

- **KIRON-1 — `mde-egui::toast` ToastHost + chyron/OSD render.** The `Toast` model
  + `ToastHost` queue/preempt/dwell/hover-pause/ack state machine (pure, headless-
  tested) + the lower-third chyron render (flag+source+host+headline+action+countdown
  +"N more") + the center-bottom OSD level tier. `Style`/`Motion` only.
- **KIRON-2 — the `event/toast/show` Bus lane + shell drain + sound + suppression.**
  The typed Bus message; the shell's `ToastHost` subscribes + paints once per frame;
  DND + per-session focus-mute suppression with Critical-breakthrough; severity-scaled
  DND-aware sound via the mixer; click-through action-verb → shell nav.
- **KIRON-3 — adopt across surfaces (prove ≥2 real emitters).** Retarget E12-19's
  rich-OSD to emit the OSD tier into KIRON, and wire the folded-alert / a security
  emitter as the first real `event/toast/show` producers (so the pattern is
  runtime-reachable, §7). NOTIFY-CHAT-5's toasts then emit here rather than rolling
  their own.

## Acceptance (epic-level, runtime-observable)

1. Three alerts arrive at once → the lower-third shows one at a time with a "2 more"
   counter; a Critical arriving jumps to the front and holds until acknowledged.
2. A remote worker publishes `event/toast/show` → the chyron appears on the shell
   with the source hostname + a working action button that navigates the shell.
3. A volume hotkey flashes the center-bottom OSD level bar instantly (direct call),
   independent of the alert queue.
4. DND (or a per-session focus mute) suppresses Info/Warning chyrons + their sound;
   a Critical still breaks through with sound.
5. Hovering the band pauses the countdown; dismissing an item advances to the next;
   nothing dismissed is lost (it's in the NOTIFY-CHAT log).
6. E12-19's OSD + NOTIFY-CHAT's toasts both render through the one ToastHost — no
   second overlay path remains.

## Risks / out of scope

- **Risks**: a Critical-storm could monopolize the band (mitigate: coalesce
  duplicate sources, "N more" backlog, ack clears); lower-third occlusion of a
  guest's own bottom UI (accept — it's transient + focus-mute exists); Bus latency
  for the alert path vs the direct OSD path (that's why OSD is a direct call).
- **Out of scope**: a full ticker/marquee mode (rejected in Q4 — rotating queue
  instead); a separate toast-history store (Q11 — chat is the record); configurable
  anchors (Q3 — fixed lower-third + center-bottom); rich multi-line cards in the band
  (Q5 — chyron stays a single-line lower-third; deep detail lives in the chat).
