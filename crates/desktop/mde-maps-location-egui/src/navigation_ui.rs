//! Maps-side consumer for the daemon-owned navigation authority.
//!
//! This module is deliberately render-free. The view queues typed intents;
//! the model's off-render Bus refresh supplies the host and wall clock, writes
//! canonical actions, and folds validated retained snapshots.

#![allow(
    missing_docs,
    reason = "closed internal projection fields are self-describing"
)]

use mackes_mesh_types::navigation::{
    navigation_cancel_action_topic, navigation_route_action_topic, CancelNavigationRequest,
    NavigationPhase, NavigationSnapshot, NavigationUnavailableReason, RouteEndpoint, RouteProfile,
    RouteRequest, RouteRequestKind, NAVIGATION_SCHEMA_VERSION,
};
use mackes_mesh_types::nws_alert::GeoPoint;

/// Immutable provider route projected for the existing Maps model and painter.
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationRouteProjection {
    pub route_id: String,
    pub provider_label: String,
    pub offline: bool,
    pub geometry: Vec<GeoPoint>,
    pub maneuver_instruction: String,
    pub maneuver_point: GeoPoint,
    pub maneuver_distance_metres: u32,
    pub distance_remaining_metres: u64,
    pub duration_remaining_seconds: u64,
    pub off_route: bool,
}

/// Stable user-visible authority state. Labels are intentionally deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationRouteStatus {
    Idle,
    RouteQueued,
    Calculating,
    Active,
    CancelQueued,
    Cancelled,
    Unavailable(NavigationUnavailableReason),
}

impl NavigationRouteStatus {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Choose a destination to request a route",
            Self::RouteQueued => "Route request queued",
            Self::Calculating => "Calculating route",
            Self::Active => "Route ready",
            Self::CancelQueued => "Cancel queued",
            Self::Cancelled => "Navigation cancelled",
            Self::Unavailable(NavigationUnavailableReason::ProviderNotConfigured) => {
                "Routing provider not configured"
            }
            Self::Unavailable(NavigationUnavailableReason::ProviderUnavailable) => {
                "Routing provider unavailable"
            }
            Self::Unavailable(NavigationUnavailableReason::InterruptedByRestart) => {
                "Route interrupted by daemon restart"
            }
            Self::Unavailable(NavigationUnavailableReason::InvalidRequest) => {
                "Route request refused"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum NavigationIntent {
    RequestRoute {
        origin: RouteEndpoint,
        destination: RouteEndpoint,
    },
    Cancel,
}

/// Exact serialized action retained across transient Bus write failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationWireAction {
    pub request_id: String,
    pub topic: String,
    pub body: String,
}

/// Generation-aware consumer state owned by the Maps model.
#[derive(Debug, Clone)]
pub struct NavigationConsumer {
    host: Option<String>,
    generation: u64,
    accepted: Option<NavigationSnapshot>,
    intent: Option<NavigationIntent>,
    wire_action: Option<NavigationWireAction>,
    request_sequence: u64,
    status: NavigationRouteStatus,
    route: Option<NavigationRouteProjection>,
    refusal: Option<&'static str>,
}

impl Default for NavigationConsumer {
    fn default() -> Self {
        Self {
            host: None,
            generation: 0,
            accepted: None,
            intent: None,
            wire_action: None,
            request_sequence: 0,
            status: NavigationRouteStatus::Idle,
            route: None,
            refusal: None,
        }
    }
}

impl NavigationConsumer {
    /// Queue a route intent from already-admitted model data. No clock or I/O.
    pub fn request_route(&mut self, origin: RouteEndpoint, destination: RouteEndpoint) {
        self.intent = Some(NavigationIntent::RequestRoute {
            origin,
            destination,
        });
        self.wire_action = None;
        self.status = NavigationRouteStatus::RouteQueued;
        self.refusal = None;
    }

    /// Queue cancellation of the current generation. No clock or I/O.
    pub fn cancel(&mut self) {
        self.intent = Some(NavigationIntent::Cancel);
        self.wire_action = None;
        self.status = NavigationRouteStatus::CancelQueued;
        self.refusal = None;
    }

    /// Materialize one canonical action outside render using caller-captured time.
    pub fn prepare_action(&mut self, host: &str, now_ms: i64) -> Option<&NavigationWireAction> {
        if self.host.as_deref().is_some_and(|bound| bound != host) {
            self.refusal = Some("Navigation action refused: foreign host authority");
            return None;
        }
        if self.wire_action.is_some() {
            return self.wire_action.as_ref();
        }
        let intent = self.intent.as_ref()?;
        self.request_sequence = self.request_sequence.saturating_add(1);
        let kind = match intent {
            NavigationIntent::RequestRoute { .. } => "route",
            NavigationIntent::Cancel => "cancel",
        };
        let request_id = format!(
            "maps-{kind}-{}-{}-{}",
            self.generation, now_ms, self.request_sequence
        );
        let (topic, body) = match intent {
            NavigationIntent::RequestRoute {
                origin,
                destination,
            } => {
                let request = RouteRequest {
                    schema_version: NAVIGATION_SCHEMA_VERSION,
                    request_id: request_id.clone(),
                    host: host.to_string(),
                    expected_generation: self.generation,
                    issued_at_ms: now_ms,
                    kind: RouteRequestKind::Route,
                    replaces_route_id: None,
                    profile: RouteProfile::Car,
                    origin: origin.clone(),
                    destination: destination.clone(),
                };
                if request.validate_at(now_ms).is_err() {
                    self.refusal = Some("Route request refused: invalid endpoint");
                    self.intent = None;
                    return None;
                }
                (
                    navigation_route_action_topic(host),
                    serde_json::to_string(&request).ok()?,
                )
            }
            NavigationIntent::Cancel => {
                let request = CancelNavigationRequest {
                    schema_version: NAVIGATION_SCHEMA_VERSION,
                    request_id: request_id.clone(),
                    host: host.to_string(),
                    expected_generation: self.generation,
                    issued_at_ms: now_ms,
                };
                if request.validate_at(now_ms).is_err() {
                    self.refusal = Some("Cancel refused: invalid authority state");
                    self.intent = None;
                    return None;
                }
                (
                    navigation_cancel_action_topic(host),
                    serde_json::to_string(&request).ok()?,
                )
            }
        };
        self.host = Some(host.to_string());
        self.refusal = None;
        self.wire_action = Some(NavigationWireAction {
            request_id,
            topic,
            body,
        });
        self.wire_action.as_ref()
    }

    /// Clear an action only after the off-render Bus write succeeds.
    pub fn mark_published(&mut self, request_id: &str) {
        if self
            .wire_action
            .as_ref()
            .is_some_and(|action| action.request_id == request_id)
        {
            self.wire_action = None;
            self.intent = None;
        }
    }

    /// Fold one validated, host-scoped, monotonic daemon projection.
    pub fn fold(&mut self, host: &str, snapshot: NavigationSnapshot, now_ms: i64) -> bool {
        if snapshot.host != host || snapshot.validate_at(now_ms).is_err() {
            self.refusal = Some("Navigation state refused: invalid authority projection");
            return false;
        }
        if self.host.as_deref().is_some_and(|bound| bound != host) {
            self.refusal = Some("Navigation state refused: foreign host authority");
            return false;
        }
        if let Some(accepted) = self.accepted.as_ref() {
            if snapshot.generation < accepted.generation {
                self.refusal = Some("Navigation state refused: stale or conflicting generation");
                return false;
            }
            if snapshot == *accepted {
                return false;
            }
            if snapshot.generation == accepted.generation
                && !same_generation_successor(accepted, &snapshot)
            {
                self.refusal = Some("Navigation state refused: stale or conflicting generation");
                return false;
            }
        }

        self.host = Some(host.to_string());
        self.generation = snapshot.generation;
        self.refusal = None;
        self.route = match &snapshot.phase {
            NavigationPhase::Active { route, progress } => {
                let maneuver = route
                    .maneuvers
                    .get(usize::from(progress.maneuver_index))
                    .expect("validated navigation maneuver index");
                Some(NavigationRouteProjection {
                    route_id: route.route_id.clone(),
                    provider_label: route.attribution.label.clone(),
                    offline: route.attribution.offline,
                    geometry: route.geometry.clone(),
                    maneuver_instruction: maneuver.instruction.clone(),
                    maneuver_point: maneuver.point.clone(),
                    maneuver_distance_metres: maneuver.distance_metres,
                    distance_remaining_metres: progress.distance_remaining_metres,
                    duration_remaining_seconds: progress.duration_remaining_seconds,
                    off_route: progress.off_route,
                })
            }
            _ => None,
        };
        self.status = match &snapshot.phase {
            NavigationPhase::Idle => NavigationRouteStatus::Idle,
            NavigationPhase::Calculating { .. } => NavigationRouteStatus::Calculating,
            NavigationPhase::Active { .. } => NavigationRouteStatus::Active,
            NavigationPhase::Cancelled { .. } => NavigationRouteStatus::Cancelled,
            NavigationPhase::Unavailable { reason, .. } => {
                NavigationRouteStatus::Unavailable(*reason)
            }
        };
        self.accepted = Some(snapshot);
        true
    }

    #[must_use]
    pub fn status(&self) -> &NavigationRouteStatus {
        &self.status
    }

    #[must_use]
    pub fn route(&self) -> Option<&NavigationRouteProjection> {
        self.route.as_ref()
    }

    #[must_use]
    pub fn refusal(&self) -> Option<&'static str> {
        self.refusal
    }

    #[must_use]
    pub fn has_pending_action(&self) -> bool {
        self.intent.is_some() || self.wire_action.is_some()
    }
}

/// The authority deliberately publishes `Calculating` before invoking its
/// provider, then publishes the terminal provider outcome at the same
/// generation. No other conflicting same-generation replacement is admissible.
fn same_generation_successor(
    accepted: &NavigationSnapshot,
    candidate: &NavigationSnapshot,
) -> bool {
    if candidate.produced_at_ms < accepted.produced_at_ms {
        return false;
    }
    let NavigationPhase::Calculating { request_id, .. } = &accepted.phase else {
        return false;
    };
    match &candidate.phase {
        NavigationPhase::Active { route, .. } => route.request_id == *request_id,
        NavigationPhase::Unavailable {
            request_id: Some(candidate_id),
            ..
        } => candidate_id == request_id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::navigation::{
        ManeuverKind, NavigationProgress, RouteAttribution, RouteManeuver, RouteResult,
    };

    const NOW: i64 = 1_800_000_000_000;

    fn endpoint(label: &str, latitude: f64) -> RouteEndpoint {
        RouteEndpoint {
            label: label.into(),
            point: GeoPoint {
                latitude,
                longitude: -75.0,
            },
        }
    }

    fn snapshot(host: &str, generation: u64, phase: NavigationPhase) -> NavigationSnapshot {
        NavigationSnapshot {
            schema_version: NAVIGATION_SCHEMA_VERSION,
            host: host.into(),
            generation,
            produced_at_ms: NOW,
            phase,
        }
    }

    fn active_phase(request_id: &str) -> NavigationPhase {
        let route = RouteResult {
            route_id: "route-1".into(),
            request_id: request_id.into(),
            calculated_at_ms: NOW,
            distance_metres: 1_609,
            duration_seconds: 600,
            geometry: vec![endpoint("Start", 40.0).point, endpoint("End", 40.1).point],
            maneuvers: vec![RouteManeuver {
                sequence: 0,
                kind: ManeuverKind::Depart,
                instruction: "Depart north".into(),
                point: endpoint("Start", 40.0).point,
                distance_metres: 1_609,
                duration_seconds: 600,
            }],
            attribution: RouteAttribution {
                provider_id: "offline-router".into(),
                label: "Approved offline router".into(),
                data_revision: "map-7".into(),
                offline: true,
            },
        };
        NavigationPhase::Active {
            progress: NavigationProgress {
                route_id: route.route_id.clone(),
                position: endpoint("Start", 40.0).point,
                observed_at_ms: NOW,
                maneuver_index: 0,
                distance_remaining_metres: route.distance_metres,
                duration_remaining_seconds: route.duration_seconds,
                off_route: false,
            },
            route,
        }
    }

    #[test]
    fn route_request_is_canonical_and_retry_is_byte_identical() {
        let mut state = NavigationConsumer::default();
        state.request_route(endpoint("Start", 40.0), endpoint("End", 40.1));
        let first = state.prepare_action("seat-1", NOW).unwrap().clone();
        let retry = state.prepare_action("seat-1", NOW + 500).unwrap().clone();
        assert_eq!(first, retry);
        assert_eq!(first.topic, navigation_route_action_topic("seat-1"));
        let request = RouteRequest::from_json_at(first.body.as_bytes(), NOW).unwrap();
        assert_eq!(request.expected_generation, 0);
        assert_eq!(request.destination.label, "End");
        state.mark_published(&first.request_id);
        assert!(state.prepare_action("seat-1", NOW + 1_000).is_none());
    }

    #[test]
    fn cancel_targets_the_latest_accepted_generation() {
        let mut state = NavigationConsumer::default();
        assert!(state.fold("seat-1", snapshot("seat-1", 4, active_phase("req-1")), NOW));
        state.cancel();
        let action = state.prepare_action("seat-1", NOW).unwrap();
        assert_eq!(action.topic, navigation_cancel_action_topic("seat-1"));
        let request = CancelNavigationRequest::from_json_at(action.body.as_bytes(), NOW).unwrap();
        assert_eq!(request.expected_generation, 4);
    }

    #[test]
    fn stale_conflicting_and_wrong_host_state_is_refused_without_route_loss() {
        let mut state = NavigationConsumer::default();
        let calculating = snapshot(
            "seat-1",
            3,
            NavigationPhase::Calculating {
                request_id: "req-1".into(),
                reroute: false,
            },
        );
        assert!(state.fold("seat-1", calculating.clone(), NOW));
        assert!(!state.fold("seat-1", calculating, NOW));
        assert!(state.fold("seat-1", snapshot("seat-1", 3, active_phase("req-1")), NOW));
        assert!(!state.fold("seat-1", snapshot("seat-1", 3, NavigationPhase::Idle), NOW));
        assert!(!state.fold("seat-1", snapshot("seat-1", 2, NavigationPhase::Idle), NOW));
        assert_eq!(
            state.refusal(),
            Some("Navigation state refused: stale or conflicting generation")
        );
        assert!(state.route().is_some());
        assert!(!state.fold("seat-1", snapshot("seat-2", 4, NavigationPhase::Idle), NOW));
        assert!(state.route().is_some());
        assert!(state.fold(
            "seat-1",
            snapshot(
                "seat-1",
                4,
                NavigationPhase::Unavailable {
                    request_id: Some("req-2".into()),
                    reason: NavigationUnavailableReason::ProviderNotConfigured,
                },
            ),
            NOW,
        ));
        assert_eq!(state.status().label(), "Routing provider not configured");
        assert!(state.route().is_none());
    }

    #[test]
    fn retained_navigation_consumer_cannot_rebind_projection_or_action_to_foreign_host() {
        let mut state = NavigationConsumer::default();
        assert!(state.fold("seat-1", snapshot("seat-1", 4, active_phase("req-1")), NOW));

        assert!(!state.fold(
            "seat-2",
            snapshot("seat-2", 5, NavigationPhase::Idle),
            NOW + 1,
        ));
        assert_eq!(
            state.refusal(),
            Some("Navigation state refused: foreign host authority")
        );
        assert_eq!(
            state.route().map(|route| route.route_id.as_str()),
            Some("route-1")
        );

        state.cancel();
        assert!(state.prepare_action("seat-2", NOW + 2).is_none());
        assert!(state.has_pending_action());
        let local_action = state.prepare_action("seat-1", NOW + 3).unwrap();
        assert_eq!(local_action.topic, navigation_cancel_action_topic("seat-1"));
    }
}
