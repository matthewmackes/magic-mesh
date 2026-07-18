//! QC-20 (CONSTRUCT-CLOUD) — **cloud events into mesh chat + idle nudges**.
//!
//! Design Q65 ("notifications KEPT — mesh chat, + cloud contacts") and Q90
//! ("idle nudges — no auto-delete; idle instances trigger a chat nudge").
//!
//! NOTIFY-CHAT doctrine (§6): the one notification surface is the mesh chat,
//! and the chat worker **already folds every `event/notify/<source>` lane** —
//! so this module only *emits* alert-shaped bodies on
//! [`INSTANCE_NOTIFY_TOPIC`] and never touches the chat worker. The body's
//! `host` field is the **service contact** ([`service_contact`] — `nova.mesh`,
//! `cinder.mesh`, …): the chat fold routes it to that contact's conversation,
//! so an instance failure reads as a message **from the Nova service contact**
//! on every roster — services become contacts with no chat-side change.
//!
//! Two producers, one cross-tick [`WatchState`]:
//!
//! - **`OpenStack` notifications** ([`CloudEvent`] /
//!   [`WatchState::note_event`]) — the typed oslo-notification fold behind
//!   the injectable [`EventSource`] seam. A *failure* event emits exactly one
//!   alert (message-id deduped); routine events are feed-silent. The
//!   production feed is honestly gated ([`NotificationFeed`] — the AMQP
//!   consumer needs the `RabbitMQ` endpoint wired; §7: a typed
//!   [`EventFeedGate::NotWired`], never a fake stream), while the **live**
//!   failure leg needs no broker at all:
//! - **The Nova roster watch** ([`WatchState::observe_instances`]) — driven
//!   off the existing [`InstanceOps`] seam on the worker's watch cadence. A
//!   `* → ERROR` status edge emits one failure alert (first sight seeds
//!   silently — no boot flood, the `CloudNotifier` discipline), and an
//!   instance sitting `SHUTOFF` past [`IDLE_AFTER`] draws one **debounced
//!   owner nudge** (re-nudged at most every [`RENUDGE_AFTER`], at most
//!   [`MAX_NUDGES_PER_OBSERVATION`] per tick — bounded, never a flood).
//!
//! Honesty notes (§7): "idle" is defined as the observable `SHUTOFF`-holding-
//! its-slot state — CPU-idle telemetry is explicitly out of scope (design:
//! no Ceilometer/Aodh, Q64/65). The Nova listing carries no owner column, so
//! the nudge rides the compute service contact's conversation naming the
//! instance (the whole workgroup's chat sees it — §8 flat trust); per-owner
//! DM routing lands when the roster carries the owner, not by fabricating
//! one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::Serialize;
use thiserror::Error;

use super::catalog::ServiceKind;
use super::designate::instance_pairs;
use super::verbs::{CloudInstance, InstanceOps};

/// The Bus lane the cloud-event/nudge alerts ride.
///
/// Under the `event/notify/` prefix the chat worker already folds
/// (NOTIFY-CHAT §6: extending coverage is choosing a lane, never an
/// emitter-side chat change).
pub const INSTANCE_NOTIFY_TOPIC: &str = "event/notify/cloud-instance";

/// The stable `source` badge on every emitted body (the chat card flag).
pub const NOTIFY_SOURCE: &str = "cloud";

/// How long an instance must sit `SHUTOFF` before it counts as idle (Q90) —
/// a full day, so a lunch-break shutdown never nudges.
pub const IDLE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The minimum gap between two nudges for the SAME instance — a week, so an
/// ignored nudge doesn't become nagging (Q90: a nudge, never pressure — and
/// emphatically no auto-delete).
pub const RENUDGE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The per-observation nudge ceiling — a fleet full of idle instances drips
/// (the skipped ones fire on later ticks), never floods the chat.
pub const MAX_NUDGES_PER_OBSERVATION: usize = 3;

/// The message-id dedup window ([`WatchState::note_event`]) — bounded so the
/// cross-tick state can't grow without limit.
const SEEN_EVENTS_CAP: usize = 512;

/// One alert-shaped body this module publishes on [`INSTANCE_NOTIFY_TOPIC`].
///
/// The `mde_chat::fold_alert` JSON shape (`severity` drives the colour,
/// `host` routes it to the contact's conversation, every other string field
/// becomes a card row), mirroring the sibling `CloudNotifier`'s body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloudChatAlert {
    /// `critical` (an instance failure) / `info` (an idle nudge).
    pub severity: &'static str,
    /// The `cloud` source badge.
    pub source: &'static str,
    /// The one-line human message the chat card shows.
    pub summary: String,
    /// The **service contact** the message arrives from (`nova.mesh`, …) —
    /// the QC-20 services-as-roster-contacts routing. `None` (an event no
    /// catalogued service claims) folds to the observing node's own contact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The instance the alert is about (name, or id when unnamed).
    pub instance: String,
    /// The alert's mint time (ms since epoch).
    pub ts_unix_ms: i64,
}

// ─────────────────────── the oslo notification fold ───────────────────────

/// One typed `OpenStack` notification — the oslo shape (`event_type` +
/// `payload`) reduced to what the chat fold needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudEvent {
    /// The dotted oslo event type (`compute.instance.create.error`, …).
    pub event_type: String,
    /// The oslo `message_id` — the dedup key (a synthesized
    /// `<type>/<instance>` stand-in when absent).
    pub message_id: String,
    /// The subject instance/resource (display name preferred, else id).
    pub instance: String,
    /// The payload `state`, when carried (`error`, `active`, …).
    pub state: Option<String>,
    /// The payload's human detail (`message` / fault message), when carried.
    pub detail: Option<String>,
    /// The notification timestamp (ms since epoch; `0` when absent — the
    /// publish path stamps the Bus write time downstream).
    pub ts_unix_ms: i64,
}

/// Parse one oslo-notification JSON body into a [`CloudEvent`]. `None` for a
/// body that isn't an object carrying an `event_type` (never a guessed
/// event, §7).
#[must_use]
pub fn parse_notification(body: &str) -> Option<CloudEvent> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = v.as_object()?;
    let event_type = obj.get("event_type")?.as_str()?.to_string();
    let payload = obj.get("payload").and_then(|p| p.as_object());
    let pstr = |k: &str| -> Option<String> {
        payload
            .and_then(|p| p.get(k))
            .and_then(|s| s.as_str())
            .map(str::to_string)
    };
    let instance = pstr("display_name")
        .or_else(|| pstr("instance_id"))
        .or_else(|| pstr("resource_id"))
        .unwrap_or_else(|| "?".to_string());
    let message_id = obj
        .get("message_id")
        .and_then(|s| s.as_str())
        .map_or_else(|| format!("{event_type}/{instance}"), str::to_string);
    let ts_unix_ms = obj
        .get("ts_unix_ms")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Some(CloudEvent {
        event_type,
        message_id,
        instance,
        state: pstr("state").map(|s| s.to_ascii_lowercase()),
        detail: pstr("message").or_else(|| pstr("fault")),
        ts_unix_ms,
    })
}

/// The **service contact** an event's alerts arrive from.
///
/// QC-20: services as roster contacts — the contact IS the service's mesh
/// hostname, the §6 hostname-as-username doctrine. Maps the oslo
/// `event_type`'s leading segment to the owning catalogued API's mesh-DNS
/// name; `None` for an event no catalogued service claims (the fold then
/// lands on the observing node's own contact — honest, never a fabricated
/// service).
#[must_use]
pub fn service_contact(event_type: &str) -> Option<String> {
    let prefix = event_type.split('.').next().unwrap_or_default();
    let kind = match prefix {
        "compute" | "instance" | "scheduler" | "keypair" | "flavor" | "aggregate" => {
            ServiceKind::NovaApi
        }
        "volume" | "snapshot" | "backup" => ServiceKind::CinderApi,
        "image" => ServiceKind::GlanceApi,
        "network" | "subnet" | "port" | "router" | "floatingip" | "security_group" => {
            ServiceKind::NeutronServer
        }
        "orchestration" | "stack" => ServiceKind::HeatApi,
        "dns" | "zone" | "recordset" => ServiceKind::DesignateApi,
        "identity" => ServiceKind::Keystone,
        "loadbalancer" | "listener" | "pool" | "member" | "octavia" => ServiceKind::OctaviaApi,
        _ => return None,
    };
    kind.mesh_dns_name().map(str::to_string)
}

/// Whether an event is a **failure** worth a chat card.
///
/// An `error`-final oslo type (`compute.instance.create.error`), an `error`
/// payload state, or a `fail`-carrying type. Routine lifecycle events
/// (`.start`/`.end`) are feed-silent (§7 — signal, not noise).
#[must_use]
pub fn is_failure(event: &CloudEvent) -> bool {
    event.event_type.rsplit('.').next() == Some("error")
        || event.event_type.contains("fail")
        || event.state.as_deref() == Some("error")
}

/// Fold a failure event into its chat alert (from the owning service
/// contact). `None` for a non-failure.
#[must_use]
pub fn fold_failure(event: &CloudEvent) -> Option<CloudChatAlert> {
    if !is_failure(event) {
        return None;
    }
    let detail = event
        .detail
        .as_deref()
        .map(|d| format!(" — {d}"))
        .unwrap_or_default();
    Some(CloudChatAlert {
        severity: "critical",
        source: NOTIFY_SOURCE,
        summary: format!(
            "cloud instance {} failed ({}){detail}",
            event.instance, event.event_type
        ),
        host: service_contact(&event.event_type),
        instance: event.instance.clone(),
        ts_unix_ms: event.ts_unix_ms,
    })
}

// ─────────────────────── the event-feed seam ───────────────────────

/// A typed reason the event feed can't stream this tick.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventFeedGate {
    /// The live consumer isn't wired in this build/environment — a real
    /// prerequisite named honestly (§7 vocabulary — the
    /// `FleetStateError::IntegrationGated` idiom).
    #[error("cloud event feed: not wired — {reason}")]
    NotWired {
        /// What the live feed needs before it can stream.
        reason: String,
    },
    /// The feed ran and failed for a concrete runtime reason.
    #[error("cloud event feed failed: {reason}")]
    Failed {
        /// The failure detail.
        reason: String,
    },
}

/// The injectable OpenStack-notification source. Tests wire a fixture feed;
/// production wires [`NotificationFeed`] until the AMQP consumer leg lands.
pub trait EventSource {
    /// Drain the net-new notifications since the last call.
    ///
    /// # Errors
    /// A typed [`EventFeedGate`] when the feed can't stream (not wired /
    /// broker unreachable) — never a fabricated empty success.
    fn drain(&self) -> Result<Vec<CloudEvent>, EventFeedGate>;
}

/// Production [`EventSource`]: honestly gated (§7).
///
/// The oslo notification stream rides the cloud's internal `RabbitMQ`
/// (Q16/Q67 — strictly separate from mde-bus), and consuming it needs the
/// broker endpoint + the sealed credential wired into an AMQP client this
/// build doesn't carry; until that leg lands the feed answers a typed
/// [`EventFeedGate::NotWired`] and the **live** failure signal rides the
/// Nova roster watch instead ([`WatchState::observe_instances`] — no broker
/// needed, real today).
#[derive(Debug, Clone, Copy, Default)]
pub struct NotificationFeed;

impl EventSource for NotificationFeed {
    fn drain(&self) -> Result<Vec<CloudEvent>, EventFeedGate> {
        Err(EventFeedGate::NotWired {
            reason: "the oslo notification consumer needs the cloud RabbitMQ endpoint + \
                     sealed credential wired over an AMQP client; the live failure signal \
                     rides the Nova roster watch until then"
                .to_string(),
        })
    }
}

// ─────────────────────── the cross-tick watch state ───────────────────────

/// One instance's idle tracking.
#[derive(Debug, Clone)]
struct IdleTrack {
    /// When the current continuous `SHUTOFF` stretch was first observed.
    shutoff_since_ms: i64,
    /// The last nudge sent for this stretch (`None` — not yet nudged).
    last_nudge_ms: Option<i64>,
}

/// The QC-20 producer's cross-tick state — rides the worker's watch loop by
/// value (the `CloudNotifier`/verb-cursor discipline).
#[derive(Debug, Clone, Default)]
pub struct WatchState {
    /// instance id → last-seen Nova status (the `* → ERROR` edge baseline;
    /// first sight seeds silently).
    seen_status: BTreeMap<String, String>,
    /// instance id → the current `SHUTOFF` stretch's idle tracking.
    idle: BTreeMap<String, IdleTrack>,
    /// The bounded seen-event dedup set (+ its FIFO eviction order).
    seen_events: BTreeSet<String>,
    /// FIFO order for [`Self::seen_events`] eviction.
    seen_order: VecDeque<String>,
}

impl WatchState {
    /// Fold one notification: a **failure** event emits exactly one alert
    /// (message-id deduped across ticks/replays); everything else is silent.
    pub fn note_event(&mut self, event: &CloudEvent) -> Option<CloudChatAlert> {
        if self.seen_events.contains(&event.message_id) {
            return None;
        }
        self.seen_events.insert(event.message_id.clone());
        self.seen_order.push_back(event.message_id.clone());
        while self.seen_order.len() > SEEN_EVENTS_CAP {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen_events.remove(&old);
            }
        }
        fold_failure(event)
    }

    /// Fold one Nova roster observation: `* → ERROR` status edges (failure
    /// alerts from the compute service contact) + the bounded, debounced
    /// idle nudges (Q90), then re-baseline and prune vanished instances.
    pub fn observe_instances(
        &mut self,
        instances: &[CloudInstance],
        now_ms: i64,
    ) -> Vec<CloudChatAlert> {
        let mut out = Vec::new();
        let nova = ServiceKind::NovaApi.mesh_dns_name().map(str::to_string);

        // 1 — ERROR edges (first sight seeds silently — no boot flood).
        for i in instances {
            let prev = self.seen_status.get(&i.id);
            if i.status == "ERROR" && prev.is_some_and(|p| p != "ERROR") {
                out.push(CloudChatAlert {
                    severity: "critical",
                    source: NOTIFY_SOURCE,
                    summary: format!(
                        "cloud instance {} went into ERROR (was {})",
                        i.name,
                        prev.map_or("?", String::as_str)
                    ),
                    host: nova.clone(),
                    instance: i.name.clone(),
                    ts_unix_ms: now_ms,
                });
            }
            self.seen_status.insert(i.id.clone(), i.status.clone());
        }

        // 2 — idle nudges (Q90): SHUTOFF continuously ≥ IDLE_AFTER, one
        // nudge per RENUDGE_AFTER, at most MAX_NUDGES_PER_OBSERVATION now.
        let mut nudged = 0_usize;
        for i in instances {
            if i.status == "SHUTOFF" {
                let track = self.idle.entry(i.id.clone()).or_insert(IdleTrack {
                    shutoff_since_ms: now_ms,
                    last_nudge_ms: None,
                });
                let idle_ms = now_ms.saturating_sub(track.shutoff_since_ms);
                let due = idle_ms >= duration_ms(IDLE_AFTER)
                    && track
                        .last_nudge_ms
                        .is_none_or(|at| now_ms.saturating_sub(at) >= duration_ms(RENUDGE_AFTER));
                if due && nudged < MAX_NUDGES_PER_OBSERVATION {
                    nudged += 1;
                    track.last_nudge_ms = Some(now_ms);
                    let days = idle_ms / 86_400_000;
                    out.push(CloudChatAlert {
                        severity: "info",
                        source: NOTIFY_SOURCE,
                        summary: format!(
                            "cloud instance {} has been SHUTOFF for {days}d — still \
                             holding its slot; start it or delete it (no auto-delete)",
                            i.name
                        ),
                        host: nova.clone(),
                        instance: i.name.clone(),
                        ts_unix_ms: now_ms,
                    });
                }
            } else {
                // Any non-SHUTOFF sighting ends the stretch — the clock
                // restarts if it shuts off again.
                self.idle.remove(&i.id);
            }
        }

        // 3 — prune vanished instances (deleted → no stale cross-tick rows).
        let live: BTreeSet<&str> = instances.iter().map(|i| i.id.as_str()).collect();
        self.seen_status.retain(|id, _| live.contains(id.as_str()));
        self.idle.retain(|id, _| live.contains(id.as_str()));

        out
    }
}

/// A `Duration` as signed millis (the alert-clock arithmetic domain).
fn duration_ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

// ─────────────────────── one watch cycle ───────────────────────

/// Publish one alert body on [`INSTANCE_NOTIFY_TOPIC`]. Best-effort — a
/// write failure is logged, never fatal (§7; the `CloudNotifier` discipline).
fn publish_alert(persist: &Persist, alert: &CloudChatAlert) {
    let Ok(json) = serde_json::to_string(alert) else {
        return;
    };
    if let Err(e) = persist.write(INSTANCE_NOTIFY_TOPIC, Priority::Default, None, Some(&json)) {
        tracing::debug!(
            target: "mackesd::openstack",
            topic = INSTANCE_NOTIFY_TOPIC,
            error = %e,
            "cloud instance notify publish failed"
        );
    }
}

/// One QC-20 watch cycle.
///
/// Drains the notification seam, observes the Nova roster (ERROR edges +
/// idle nudges), publishes every resulting alert, and returns the fresh
/// `(name, ip)` roster snapshot the QC-17 Designate zone feed derives
/// instance records from (`None` when the roster couldn't be read — the
/// caller keeps its previous snapshot, never an emptied feed).
///
/// Synchronous + seam-pure: the worker drives it on a blocking task; tests
/// drive it directly with a real [`Persist`] tempdir + fakes.
pub fn watch_cycle(
    persist: &Persist,
    ops: &dyn InstanceOps,
    source: &dyn EventSource,
    state: &mut WatchState,
    now_ms: i64,
) -> Option<Vec<(String, String)>> {
    // 1 — the (seam) OpenStack notification feed. A NotWired production feed
    // is an honest quiet gate, not an alert-flood: the live failure signal
    // rides the roster watch below.
    match source.drain() {
        Ok(events) => {
            for event in &events {
                if let Some(alert) = state.note_event(event) {
                    publish_alert(persist, &alert);
                }
            }
        }
        Err(gate) => {
            tracing::debug!(target: "mackesd::openstack", %gate, "cloud event feed gated");
        }
    }

    // 2 — the live Nova roster: ERROR edges + idle nudges, off the existing
    // InstanceOps seam. A CLI-less/failed read gates quietly (§7 — the same
    // honest degrade the list-instances verb reports loudly on demand).
    match ops.list() {
        Ok(instances) => {
            for alert in state.observe_instances(&instances, now_ms) {
                publish_alert(persist, &alert);
            }
            Some(instance_pairs(&instances))
        }
        Err(e) => {
            tracing::debug!(target: "mackesd::openstack", error = %e, "instance watch: roster read gated");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: &str, name: &str, status: &str) -> CloudInstance {
        CloudInstance {
            id: id.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            flavor: None,
            image: None,
            networks: Some(format!("mesh=10.42.100.{}", id.len())),
        }
    }

    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    // ── the oslo fold ──

    #[test]
    fn a_failure_event_folds_to_one_notify_emission_from_its_service_contact() {
        // QC-20 acceptance: a failure event → ONE notify emission, from the
        // owning service's contact; a replay of the same message id is silent.
        let body = r#"{
            "event_type": "compute.instance.create.error",
            "message_id": "msg-1",
            "payload": {"display_name": "web-1", "state": "error",
                        "message": "No valid host was found"}
        }"#;
        let event = parse_notification(body).expect("parses");
        let mut state = WatchState::default();
        let alert = state.note_event(&event).expect("a failure emits");
        assert_eq!(alert.severity, "critical");
        assert_eq!(alert.host.as_deref(), Some("nova.mesh"));
        assert!(alert.summary.contains("web-1"), "{}", alert.summary);
        assert!(alert.summary.contains("No valid host"), "{}", alert.summary);
        // The dedup: the same notification never emits twice (§7 — one card).
        assert_eq!(state.note_event(&event), None, "deduped on message_id");
    }

    #[test]
    fn routine_events_and_unknown_bodies_are_feed_silent() {
        let mut state = WatchState::default();
        let ok = parse_notification(
            r#"{"event_type":"compute.instance.create.end",
                "message_id":"m2","payload":{"display_name":"web-1","state":"active"}}"#,
        )
        .unwrap();
        assert_eq!(state.note_event(&ok), None, "a routine event is silent");
        // A non-object / event_type-less body never fabricates an event.
        assert_eq!(parse_notification("not json"), None);
        assert_eq!(parse_notification(r#"{"payload":{}}"#), None);
    }

    #[test]
    fn service_contacts_map_event_types_to_catalogued_mesh_names() {
        // QC-20 — services as roster contacts: the contact is the owning
        // API's mesh hostname (§6 hostname-as-username).
        assert_eq!(
            service_contact("compute.instance.update").as_deref(),
            Some("nova.mesh")
        );
        assert_eq!(
            service_contact("volume.create.error").as_deref(),
            Some("cinder.mesh")
        );
        assert_eq!(
            service_contact("image.upload").as_deref(),
            Some("glance.mesh")
        );
        assert_eq!(
            service_contact("dns.zone.create").as_deref(),
            Some("designate.mesh")
        );
        assert_eq!(
            service_contact("orchestration.stack.create.error").as_deref(),
            Some("heat.mesh")
        );
        // An unclaimed prefix is honestly None (the fold lands on the
        // observing node's own contact), never a fabricated service.
        assert_eq!(service_contact("weird.thing"), None);
        let ev = CloudEvent {
            event_type: "weird.thing.error".to_string(),
            message_id: "m".to_string(),
            instance: "x".to_string(),
            state: None,
            detail: None,
            ts_unix_ms: 0,
        };
        assert_eq!(fold_failure(&ev).expect("still a failure").host, None);
    }

    // ── the roster watch: ERROR edges ──

    #[test]
    fn an_error_edge_emits_once_and_first_sight_seeds_silently() {
        let mut state = WatchState::default();
        // First sight in ERROR: seeded silently (a boot never floods).
        let seeded = state.observe_instances(&[instance("u1", "web-1", "ERROR")], 0);
        assert!(seeded.is_empty(), "{seeded:?}");
        // ACTIVE → ERROR: exactly one critical from the nova contact.
        let mut state = WatchState::default();
        assert!(state
            .observe_instances(&[instance("u1", "web-1", "ACTIVE")], 0)
            .is_empty());
        let alerts = state.observe_instances(&[instance("u1", "web-1", "ERROR")], 1000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "critical");
        assert_eq!(alerts[0].host.as_deref(), Some("nova.mesh"));
        assert!(alerts[0].summary.contains("web-1"), "{}", alerts[0].summary);
        // Still-ERROR is not a new edge; recovery + re-failure is.
        assert!(state
            .observe_instances(&[instance("u1", "web-1", "ERROR")], 2000)
            .is_empty());
        state.observe_instances(&[instance("u1", "web-1", "ACTIVE")], 3000);
        assert_eq!(
            state
                .observe_instances(&[instance("u1", "web-1", "ERROR")], 4000)
                .len(),
            1
        );
    }

    // ── the idle nudge (Q90) ──

    #[test]
    fn an_idle_instance_draws_one_debounced_nudge() {
        // QC-20 acceptance: idle → ONE debounced nudge. SHUTOFF must hold
        // for IDLE_AFTER, the nudge repeats no sooner than RENUDGE_AFTER,
        // and a restart clears the stretch.
        let mut state = WatchState::default();
        let shutoff = [instance("u1", "web-1", "SHUTOFF")];
        // First sight starts the clock — no instant nudge.
        assert!(state.observe_instances(&shutoff, 0).is_empty());
        // Under the threshold: still silent.
        assert!(state.observe_instances(&shutoff, DAY_MS - 1).is_empty());
        // Past 24h: exactly one info nudge from the compute contact.
        let nudges = state.observe_instances(&shutoff, DAY_MS + HOUR_MS);
        assert_eq!(nudges.len(), 1, "{nudges:?}");
        assert_eq!(nudges[0].severity, "info");
        assert_eq!(nudges[0].host.as_deref(), Some("nova.mesh"));
        assert!(
            nudges[0].summary.contains("SHUTOFF for 1d"),
            "{}",
            nudges[0].summary
        );
        assert!(
            nudges[0].summary.contains("no auto-delete"),
            "Q90: {}",
            nudges[0].summary
        );
        // Debounced: an hour later there is NO second nudge.
        assert!(state
            .observe_instances(&shutoff, DAY_MS + 2 * HOUR_MS)
            .is_empty());
        // …until RENUDGE_AFTER elapses.
        assert_eq!(
            state
                .observe_instances(&shutoff, DAY_MS + HOUR_MS + 7 * DAY_MS)
                .len(),
            1
        );
        // A start clears the stretch; a fresh SHUTOFF restarts the clock.
        let t = 10 * DAY_MS;
        state.observe_instances(&[instance("u1", "web-1", "ACTIVE")], t);
        assert!(state.observe_instances(&shutoff, t + HOUR_MS).is_empty());
        assert!(state
            .observe_instances(&shutoff, t + 2 * HOUR_MS)
            .is_empty());
    }

    #[test]
    fn nudges_are_bounded_per_observation_and_drip_across_ticks() {
        // The flood bound: 5 idle instances → 3 nudges now, the remaining 2
        // on the next observation (their debounce was never consumed).
        let mut state = WatchState::default();
        let fleet: Vec<CloudInstance> = (0..5)
            .map(|n| instance(&format!("u{n}"), &format!("vm-{n}"), "SHUTOFF"))
            .collect();
        assert!(state.observe_instances(&fleet, 0).is_empty());
        let first = state.observe_instances(&fleet, 2 * DAY_MS);
        assert_eq!(first.len(), MAX_NUDGES_PER_OBSERVATION);
        let second = state.observe_instances(&fleet, 2 * DAY_MS + 1000);
        assert_eq!(second.len(), 2, "the skipped two drip through next");
        // Everyone nudged exactly once across the two ticks.
        let mut all: Vec<&str> = first
            .iter()
            .chain(second.iter())
            .map(|a| a.instance.as_str())
            .collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn vanished_instances_are_pruned_from_the_cross_tick_state() {
        let mut state = WatchState::default();
        state.observe_instances(&[instance("u1", "web-1", "SHUTOFF")], 0);
        assert_eq!(state.idle.len(), 1);
        assert_eq!(state.seen_status.len(), 1);
        // Deleted from the cloud → deleted from the state (no stale rows).
        state.observe_instances(&[], 1000);
        assert!(state.idle.is_empty());
        assert!(state.seen_status.is_empty());
    }

    #[test]
    fn the_event_dedup_window_is_bounded() {
        let mut state = WatchState::default();
        for n in 0..(SEEN_EVENTS_CAP + 10) {
            let ev = CloudEvent {
                event_type: "compute.instance.update".to_string(),
                message_id: format!("m{n}"),
                instance: "x".to_string(),
                state: None,
                detail: None,
                ts_unix_ms: 0,
            };
            let _ = state.note_event(&ev);
        }
        assert_eq!(state.seen_events.len(), SEEN_EVENTS_CAP);
        assert_eq!(state.seen_order.len(), SEEN_EVENTS_CAP);
    }

    // ── one whole watch cycle over the seams ──

    struct FixtureFeed {
        events: Vec<CloudEvent>,
    }

    impl EventSource for FixtureFeed {
        fn drain(&self) -> Result<Vec<CloudEvent>, EventFeedGate> {
            Ok(self.events.clone())
        }
    }

    #[test]
    fn watch_cycle_publishes_alerts_and_returns_the_designate_snapshot() {
        use super::super::testkit::FakeInstanceOps;

        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        let ops = FakeInstanceOps::new().with_instances(vec![instance("u1", "web-1", "ACTIVE")]);
        let feed = FixtureFeed {
            events: vec![parse_notification(
                r#"{"event_type":"volume.create.error","message_id":"m9",
                    "payload":{"display_name":"data-vol","state":"error"}}"#,
            )
            .unwrap()],
        };
        let mut state = WatchState::default();
        let snapshot = watch_cycle(&persist, &ops, &feed, &mut state, 0).expect("roster read");
        // The QC-17 tie-in: the fresh (name, ip) pairs feed the zone render.
        assert_eq!(
            snapshot,
            vec![("web-1".to_string(), "10.42.100.2".to_string())]
        );
        // The failure event landed on the folded lane, from cinder's contact.
        let rows = persist
            .list_since(INSTANCE_NOTIFY_TOPIC, None)
            .unwrap_or_default();
        assert_eq!(rows.len(), 1, "one emission");
        let body = rows[0].body.clone().unwrap();
        assert!(body.contains("\"host\":\"cinder.mesh\""), "{body}");
        assert!(body.contains("data-vol"), "{body}");
        // A second cycle re-draining the same feed emits nothing new.
        let _ = watch_cycle(&persist, &ops, &feed, &mut state, 1000);
        assert_eq!(
            persist
                .list_since(INSTANCE_NOTIFY_TOPIC, None)
                .unwrap_or_default()
                .len(),
            1
        );
    }

    #[test]
    fn watch_cycle_gates_quietly_when_the_roster_is_unreadable() {
        use super::super::testkit::FakeInstanceOps;

        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        let ops = FakeInstanceOps::new().failing("keystone unreachable");
        let mut state = WatchState::default();
        // The production NotWired feed + a failed roster read: both honest
        // quiet gates — no snapshot, no fabricated alerts (§7).
        let snapshot = watch_cycle(&persist, &ops, &NotificationFeed, &mut state, 0);
        assert_eq!(snapshot, None, "keep the previous snapshot");
        assert!(persist
            .list_since(INSTANCE_NOTIFY_TOPIC, None)
            .unwrap_or_default()
            .is_empty());
    }
}
