# Mesh Chat — the ICQ-style unified messaging + notification interface (NOTIFY-CHAT-1..6)

> **Status: LOCKED 2026-07-02** — 25-question operator survey (this session, per
> `/plan`). A group + 1:1 chat with an authentic ICQ interface that **becomes the
> one notification interface** for the whole platform: every system alert and every
> clipboard copy from any host — local or remote — arrives as a **message from that
> host's contact**, with the **hostname as the username**. Successor to the
> completed NOTIFY-REDESIGN work; subsumes and removes the standalone Notifications
> and Clipboard surfaces.

## The locks

| # | Fork | Lock |
|---|------|------|
| 1 | Transport | **Hybrid**: `mde-bus` over Nebula for live delivery, **Syncthing-replicated ring-buffer logs** for durable history + offline backfill. |
| 2 | Notify model | **Each host is a contact; its alerts are its messages.** A system notification from peer `nyc3` reads as a message from the `nyc3` contact — **hostname = username**. Human chat + machine notifications share one timeline per contact. |
| 3 | Clipboard | **Fully subsumed.** A clipboard copy on a host is a message from that host's contact (monospace/preview, one-click re-copy); the standalone Clipboard surface is removed. |
| 4 | ICQ layout | **Authentic ICQ**: the surface *is* the contact roster (narrow list, per-contact status icon, **Online/Offline groups**, unread bold + count); selecting a contact opens its conversation as a focused in-shell pane (the DRM shell has no floating OS windows — "message windows" are shell panes). Classic status idiom in Construct `Style`. |
| 5 | Presence | **Auto from mesh health + manual override.** Baseline from real reachability (Online / Away = stale heartbeat / Offline = unreachable, from the existing mesh-status snapshot); the operator may set **Away / DND / Invisible / Free-for-Chat** on their own node, gossiped to peers. DND suppresses sound + toast. |
| 6 | Roster identity | **Every enrolled mesh member + self + VM guests.** One contact per node (by hostname) and per VM-guest mesh peer, plus the local host (self-contact). Role badge (lighthouse / workstation / headless / VM). The roster **is** the peer directory. |
| 7 | Groups | **Ad-hoc named rooms + auto system rooms.** Operators create named rooms (membership = a signed, Syncthing-replicated room descriptor); PLUS auto-provisioned **All Fleet** + **per-severity alert** rooms so fleet-wide notifications have a home. |
| 8 | History | **Recent-only ring buffer** per conversation (bounded, no long archive). |
| 9 | Offline | **Queue in mesh state, deliver on reconnect.** Live Bus send fails → the message stays in the sender's Syncthing log → the peer backfills the ring on return (with an offline/delivered-later indicator). No loss within the ring window. |
| 10 | Security | **Ed25519-signed** (sender node identity — "from nyc3" is unforgeable, load-bearing since notifications drive ops) + **Nebula-encrypted transit** + mesh-trusted Syncthing at rest. No separate E2E layer (consistent with §8 flat-trust + §3 crypto locks). |
| 11 | Bridge | **The chat worker subscribes every existing alert/event Bus lane** and folds each into a message from the originating host (severity → styling, payload → body). **No emitter changes** — every current + future alert lane flows in automatically. |
| 12 | Sound | **Per-kind sounds, DND-aware, via the seat mixer** (E12-16). Human-message vs system-alert distinct; severity-scaled; suppressed by DND / conversation mute; operator-swappable set (ICQ "Uh-oh!"-style default). |
| 13 | Popups | **Toast for messages + alerts**, compact severity-colored egui overlay over anything fullscreen, click-to-open, DND/mute-suppressed — **reuses the host-controls rich-OSD overlay** (one overlay system). |
| 14 | Placement | **New `Surface::Chat`** (roster + conversation panes) **replaces `Surface::Notifications` + `Surface::Clipboard`**; the chrome bar gains a compact presence/unread indicator (total unread + your status flag) that expands into it — the always-visible ICQ tray. |
| 15 | Message kinds | **Text + emoji · clipboard items (re-copyable) · system alert cards (severity + inline action, e.g. `action/shell/goto`) · file send-to (mesh transfer, reuse `mde-files` Send-To) · Call (SIP dialer, `mde-voice`) · Remote Control (VDI/RDP into that host, reuse `mde-vdi`/Instances)** — the last two are per-contact conversation actions: chat is the launch point for voice + remote desktop. |
| 16 | Muting | **Per-contact/room mute + a global per-severity threshold** (e.g. toast/sound only for Warning+; Info stays silent but logged). Tames the machine-alert firehose without losing the audit trail or a real security alert. |
| 17 | Self | **Self-contact carries local alerts/clips** (pinned), no notes-to-self. |
| 18 | Architecture | **New `mde-chat` crate** (message/conversation/roster model, ring-buffer log, signing, alert-fold, presence — services tier) + a **mackesd `chat` worker** (live Bus send/recv, Syncthing logs, offline queue, alert-lane subscriptions, presence gossip); `Surface::Chat` renders it. **`mde-notify` is absorbed** (its `AlertItem`/`Severity` become an `mde-chat` message kind). |
| 19 | Delivery UI | **Sent / Delivered / Queued-offline** (honest three-state checkmark; Delivered = peer worker acked over Bus; Queued = will backfill). No read receipts. |
| 20 | Alert reach | **Fleet-wide**: every host's alerts appear in that host's contact on **every** node's roster (matches no-fixed-center "any node reads fleet state"); the severity mute (16) tames volume. |
| 21 | Naming | **Hostname is the unforgeable identity** (shown, and what signing binds to); a node may set an optional cosmetic **nickname** + an ICQ-style free-text **status message** gossiped with presence. |
| 22 | Group delivery | **Sender fans out on Bus to each online member + appends to one shared Syncthing room ring-log**; offline members backfill from it. One canonical ordered log per room (sender timestamp + signature), Bus is the fast path — no central relay. |
| 23 | Migration | **Hard cutover**: when `Surface::Chat` is §7-complete, delete `Surface::Notifications` + `Surface::Clipboard` + their modules and fold+remove `mde-notify`. One notification interface, no dead parallel surfaces (E12-14 decommission discipline). |
| 24 | Epic | **Folded into the NOTIFY-REDESIGN lineage** as `NOTIFY-CHAT-1..6`. |
| 25 | Room authority | **Open rooms — anyone can join** (discoverable + self-join; every membership change is a signed, attributable room-descriptor update; consistent with §9 no-RBAC + flat-trust). |

## Architecture

```
mde-shell-egui                        mackesd                         substrate
┌───────────────────────────┐        ┌────────────────────────┐      ┌──────────────┐
│ Surface::Chat (ICQ)       │ verbs  │ chat worker            │      │ Nebula       │
│  · roster (Online/Offline │───────▶│  · Bus live send/recv  │─────▶│ (transit)    │
│    · status · presence)   │  Bus   │    (action/chat/*,     │      │              │
│  · conversation panes     │◀───────│     event/chat/message)│      │ Syncthing    │
│  · composer + actions     │ state  │  · subscribes ALL alert│─────▶│ (ring logs:  │
│    (call/remote/send-to)  │        │    lanes → fold to msg │      │  per-conv +  │
│  · delivery status        │        │  · offline queue +     │      │  per-room)   │
│ chrome: presence/unread   │        │    backfill            │      └──────────────┘
│ toast (rich-OSD reuse)    │        │  · presence gossip     │
└───────────────────────────┘        └────────────────────────┘
        │ mde-chat (model: Message/Conversation/Room/Roster/Presence, ring-log,
        │           Ed25519 sign+verify, alert-fold, message kinds) — services tier
```

- **`mde-chat`** (services tier) is the shared model + logic: `Message` (kinds:
  Text, Clipboard, Alert{severity,action}, File, CallAction, RemoteAction),
  `Conversation`/`Room` ring-log, `Roster`/`Contact`/`Presence`, Ed25519
  sign/verify, and the pure **alert-fold** (a Bus alert payload → a Message from a
  host). Every fold + the ring-buffer + signing are pure, headless-tested (the
  mde-kvm/mde-seat pattern).
- **§6 tiers**: `mde-chat` + the worker are services/substrate-facing; `Surface::Chat`
  is shell. The chat worker runs on **every** node incl. headless (headless emits +
  relays, just no UI) — so alerts flow fleet-wide. No desktop dep in the substrate.
- **§9 one-state**: conversations/rooms/rosters are etcd-presence + Syncthing-log +
  typed `mackesd` Bus verbs; the GUI is a renderer; CLI parity possible.
- **Reuse (§6 glue, not reimplementation)**: presence from the existing mesh-status
  snapshot + voice roster; clipboard from the existing `event/clipboard/clip`;
  Send-To from `mde-files`; Call from `mde-voice`; Remote from `mde-vdi`/Instances;
  toast from the host-controls rich-OSD; sound from the E12-16 mixer.

## The units (NOTIFY-CHAT-1..6, lifted to WORKLIST)

- **NOTIFY-CHAT-1 — `mde-chat` core crate.** The model + ring-log + Ed25519
  sign/verify + the pure alert-fold + presence types. Headless-tested; no I/O.
- **NOTIFY-CHAT-2 — mackesd `chat` worker.** Live Bus send/recv, the Syncthing
  per-conversation + per-room ring logs, offline queue + reconnect backfill, the
  subscribe-all-alert-lanes fold (Lock 11), presence gossip + manual status.
  Runs on every node (headless included).
- **NOTIFY-CHAT-3 — `Surface::Chat` ICQ UI.** Roster (Online/Offline groups,
  status icons, presence, unread), conversation panes, composer, delivery-status
  checkmarks — Construct `Style`, over the real worker (no demo_data).
- **NOTIFY-CHAT-4 — message kinds + per-contact actions.** Text/emoji, clipboard
  re-copy, alert cards + inline action (`action/shell/goto`), file Send-To,
  Call (SIP), Remote Control (VDI) — each reusing its owning crate.
- **NOTIFY-CHAT-5 — rooms, muting, sound, toast.** Ad-hoc + auto system rooms
  (open-join, shared room-log fan-out), per-contact + per-severity mute, per-kind
  DND-aware sounds via the mixer, rich-OSD toasts.
- **NOTIFY-CHAT-6 — hard cutover.** Remove `Surface::Notifications` +
  `Surface::Clipboard` + modules, fold + remove `mde-notify`, chrome
  presence/unread indicator, packaging (sound assets, any new deps).

**Serialization**: CHAT-1 first (the model everything imports); CHAT-2 + CHAT-3
parallelize on it (worker vs UI, disjoint crates); CHAT-4/5 layer on 2+3; CHAT-6
last (it deletes surfaces + folds mde-notify — touches dock.rs/main.rs, serialize
against any other shell-wiring unit).

## Acceptance (epic-level, runtime-observable)

1. Two workstations: a 1:1 message typed on A appears live on B's `A` contact,
   signed + verified; B offline → A shows "Queued-offline", and it backfills when
   B returns.
2. A `event/security/alert` on `nyc3` appears as a severity-colored alert-card
   message from the `nyc3` contact on **every** node's roster, with a working
   inline "Go to …" action.
3. A clipboard copy on a peer appears as a re-copyable message from that peer;
   clicking re-copies it locally. No standalone Clipboard surface remains.
4. A named room with 3 members: a message reaches all online members live and an
   offline member on reconnect; anyone can join; the creator can dissolve it.
5. Presence: setting DND silences sound + toast but still logs; an unreachable
   node shows Offline within a heartbeat window; a custom status message shows
   beside the hostname.
6. From a contact: Call opens a SIP session and Remote Control opens that host's
   VDI desktop, both from the conversation.
7. After cutover: `Surface::Notifications` + `Surface::Clipboard` are gone,
   `mde-notify` removed, and the chrome unread indicator expands into `Surface::Chat`.

## Risks / out of scope

- **Risks**: Syncthing sync latency for group-log consistency (Bus is the live
  path; the log is backfill/ordering — accept eventual consistency for history);
  ring-buffer sizing vs a security-alert flood (per-severity mute + caps);
  fleet-wide alert replication volume (every node stores every host's ring — bounded
  by the ring cap); signing throughput on a chatty alert stream (batch-verify).
- **Out of scope v1**: read receipts, message editing/threading, E2E per-conversation
  encryption, per-person (non-node) identity, federation beyond the mesh, voice/video
  *inside* chat (Call hands off to `mde-voice`). The **global "Kiron" toast pattern**
  is a **separate follow-up epic** (`/plan` queued 2026-07-02) — this epic reuses the
  existing rich-OSD; that epic generalizes the toast system.
