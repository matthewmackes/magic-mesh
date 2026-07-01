# Event Routing — the mesh-native Alertmanager (operator-locked 2026-06-30)

Locked via a 25-question `/plan` survey (EC-Q1..25). The "Event Chooser" routes
**every mesh event** to notifications / sounds / other receivers. OSS studied:
**Prometheus Alertmanager** (routing tree, grouping, silence, inhibition,
receivers), **rsyslog/syslog-ng** (normalized ingestion + property routing),
**ntfy** (pub-sub push; mde-bus already brokers ntfy-style), **Apprise**
(pluggable receiver abstraction), **systemd-journald** (structured records).

## Identity
A **mesh-native Alertmanager**: a leader-coordinated `mackesd` **event-router
worker** over `mde-bus` + the events plane — NOT an external daemon (§6
mesh-tooling-first, §1 no-fixed-center). It is the SINGLE ingest for all events.

## Locks (EC-Q#)
| # | Decision |
|---|---|
| 1 | Shape = routing layer (match → receivers) + grouping/silence/inhibition |
| 2 | Build native on mde-bus, modeled on Alertmanager + Apprise + syslog |
| 3 | Event awareness = **auto-discover** live bus/events-plane topics into a catalog |
| 4 | Routing model = **Alertmanager routing tree** (nested match-blocks, inheritance + `continue`) |
| 5 | Receivers (full set day-one): in-app (toast/hub), sound, email, webhook, ntfy-push, log, OS-native, **KDE Connect** (phone alerting via the mesh's existing KDE-Connect host) — behind a pluggable receiver trait (Apprise-style) |
| 6 | Sounds = per-severity defaults + per-rule override; built-in set + custom file |
| 7 | Notification types a route can target: toast · hub-only(silent) · persistent-banner · OS-native · phone-push (any combination) |
| 8 | Out-of-box = sensible severity default tree: critical→toast+sound+persist, warning→toast, info→silent hub-log |
| 9 | Grouping = yes, by source/type within a window |
| 10 | Silences **and** inhibition both |
| 11 | Rule scope = **mesh-wide single policy** (leader-owned, replicated; one-state doctrine §9) |
| 12 | Chooser UI = a dedicated Workbench **"Event Routing"** panel (Monitoring/System) |
| 13 | Event record = **full syslog-shaped**: source-node · severity · facility(=topic) · message · labels(map) · ts · id · raw-original |
| 14 | Catalog UI = live catalog (type · last-seen · rate · sample), searchable; click → build a route |
| 15 | Testing = synthetic test event + dry-run "what matches" preview (no real delivery) |
| 16 | Audit = full routing-decision log to the hash-chain events plane (§8) |
| 17 | Receiver creds = leader-managed **sealed mesh secret store** (XCP-7/EFF-21), set via the chooser, never plaintext |
| 18 | Throttle = per-receiver rate cap + the grouping window |
| 19 | Persistent banner = sticky in the Hub header until acked; one-at-a-time, highest severity wins |
| 20 | OS-native = route through the existing Cosmic/FDO host (notifyd / org.freedesktop.Notifications), §2 |
| 21 | Receiver failure = retry+backoff → dead-letter to the in-app Hub → audit (nothing silently lost) |
| 22 | Editor = **form-based rule builder + live tree preview** (Carbon forms, no hand-edited config) |
| 23 | Severity = event's declared value + a normalization mapping table (by topic/source) filling gaps; default info |
| 24 | DND/mute = global suppression ABOVE routing: suppresses interruptive receivers (toast/sound/push/banner) but ALWAYS keeps the hub-log + audit; per-source mute drops that source's interruptive receivers |
| 25 | Rollout = **big-bang replace**: the router is the single ingest; today's events→AlertItem→Hub becomes the built-in default in-app route (no behavior regression) |

## Architecture
- **`mackesd` event-router worker** (leader-elected; the mesh-wide routing policy
  lives in etcd, replicated): subscribes to ALL mde-bus / events-plane topics →
  normalizes each to the syslog-shaped `EventRecord` → evaluates the **routing
  tree** (match by node/severity/topic/labels; inheritance; `continue`) →
  applies **grouping** (coalesce by source/type in a window), **silences**
  (time-boxed mutes), **inhibition** (suppress X while Y fires), **per-receiver
  throttle** → dispatches to matched **receivers** via a pluggable
  `Receiver` trait. Every decision (matched routes · receivers fired/failed ·
  silences/inhibitions applied) appends to the hash-chain events plane.
- **Receivers** (the pluggable set): `in_app` (publishes to the notify plane →
  toast/hub/persistent-banner per the notification-type flags), `sound` (the
  per-severity/per-rule sound), `os_native` (→ the FDO/Cosmic host), `email`
  (SMTP, sealed creds), `webhook` (sealed token), `ntfy_push` (sealed auth),
  `log`. Failures retry+backoff then dead-letter to `in_app`.
- **Event catalog** (auto-discovered): the worker records each distinct event
  type it sees (last-seen, rate, a sample) into a queryable catalog the panel
  reads — this is "aware of ALL events".
- **Workbench "Event Routing" panel**: catalog browser · routing-tree form
  builder + live preview · receiver config (creds via the secret store) ·
  silences/inhibition manager · synthetic-test/dry-run. CLI parity via `mde-bus`
  typed verbs (§9 every-surface-CLI-parity).
- **DND/mute** (from the Hub) is a suppression layer the worker consults before
  dispatching interruptive receivers; the hub-log + audit always happen.

## Acceptance (runtime-observable, §7)
- A real mde-bus event flows through the worker, matches the default tree, and
  fires its receivers (verified live: an `info` lands silent-in-hub, a `critical`
  toasts + sounds + persists).
- The catalog lists event types actually seen on the bus with live last-seen/rate.
- A route authored in the panel form changes delivery for the next matching event
  (dry-run preview matched it first); a silence suppresses it for its window; an
  inhibition suppresses children while the parent fires.
- A failing webhook/email receiver retries, then dead-letters to the Hub, and the
  failure + every routing decision appear in the hash-chain audit.
- Receiver secrets never appear in config/logs/`ps` (sealed-store test).
- DND suppresses the toast/sound but the event still appears in the Hub + audit.

## Out of scope (do NOT build yet)
External Alertmanager/PagerDuty integration; ML/anomaly event synthesis; a
cross-mesh (federated) routing exchange; per-user (vs per-node-operator) routing.

## Worklist
Lift as `### EVENT-ROUTING` in `docs/WORKLIST.md` — worker + record/normalize +
catalog + routing-tree eval + each receiver + silences/inhibition + the panel +
the default-tree migration, each a user-story task with the acceptance bullets
above. The Notification Hub redesign (`docs/design/notification-hub-redesign.md`)
is the in-app receiver's surface; the two epics integrate at the `in_app` receiver.
