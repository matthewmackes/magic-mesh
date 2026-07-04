//! EXPLORER-1 — E9: the typed Bus read verb for the unit stream.
//!
//! Any mesh client can pull a node's folded units with the request/reply idiom
//! (`crates/platform/mde-bus/src/rpc.rs`): publish to [`UNITS_REQUEST_TOPIC`] and
//! read the [`UnitsReply`] on `reply/<request-ulid>`. This is the pull companion
//! to the always-maintained `state/units/<node>` push mirror — the shell renders
//! from the push topic (EXPLORER-3), while this verb lets any Rust/CLI client
//! fetch the current stream on demand. The `get-` verb name marks it a read
//! (audit-exempt like the other observational query verbs). EXPLORER-7 extends
//! the reply with the edge set over the same request.

use serde::{Deserialize, Serialize};

use super::unit::UnitsState;

/// The request topic any mesh client publishes to pull a node's unit stream.
pub const UNITS_REQUEST_TOPIC: &str = "action/units/get-stream";

/// The typed request body.
///
/// Reserved for future scoping (by kind / reachability / EXPLORER-7's
/// `include_edges`); today any object body is a plain "give me the current
/// stream", and an empty body is accepted too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitsRequest {
    /// Reserved: when set, EXPLORER-8/3 can request only one category
    /// (`mesh`/`lan`/`cloud`). Ignored today (the whole stream is returned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// The typed reply published to `reply/<request-ulid>`.
///
/// `ok` mirrors the shared `{"ok":true}` reply convention (the `dc/*` + action
/// lanes) so a generic client can classify success without knowing the units
/// schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitsReply {
    /// `true` when `state` carries the answer; `false` on a rejected request.
    pub ok: bool,
    /// The folded unit stream, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<UnitsState>,
    /// A human-readable rejection reason, on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UnitsReply {
    /// A successful answer carrying the current stream.
    #[must_use]
    pub const fn answer(state: UnitsState) -> Self {
        Self {
            ok: true,
            state: Some(state),
            error: None,
        }
    }

    /// A typed rejection (a malformed request body).
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            state: None,
            error: Some(reason.into()),
        }
    }

    /// JSON body for the `reply/<ulid>` lane. Infallible — a serialize failure
    /// (should never happen for this plain type) degrades to a typed error body.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"units reply encode failed"}"#.to_string())
    }
}

/// Parse a request body into a typed [`UnitsRequest`]. An empty body is a valid
/// "whole stream" request; a non-empty body must be a JSON object.
///
/// # Errors
/// A human-readable message when the body is present but not valid request JSON.
pub fn parse_units_request(body: &str) -> Result<UnitsRequest, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(UnitsRequest::default());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("bad units request body: {e}"))
}

/// The pure verb handler: parse the request and answer with `current`. A
/// malformed body is a typed rejection — never a panic, never a fabricated
/// answer (§7).
#[must_use]
pub fn handle_units_request(body: &str, current: &UnitsState) -> UnitsReply {
    match parse_units_request(body) {
        Ok(_req) => UnitsReply::answer(current.clone()),
        Err(e) => UnitsReply::rejected(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::unit_aggregator::unit::{peer_unit_id, Reachability, Unit, UnitKind};

    fn sample_state() -> UnitsState {
        UnitsState {
            host: "node-a".into(),
            units: vec![Unit {
                id: peer_unit_id("node-a"),
                kind: UnitKind::Peer,
                name: "node-a".into(),
                reachability: Reachability::InMesh,
                address: None,
                health: None,
                telemetry: None,
                mesh: None,
                cloud: None,
                first_seen_ms: 1,
                last_seen_ms: 2,
                extras: super::super::unit::Extras::default(),
            }],
            edges: vec![],
            published_at_ms: 3,
        }
    }

    #[test]
    fn request_topic_is_a_read_verb_under_action_units() {
        assert_eq!(UNITS_REQUEST_TOPIC, "action/units/get-stream");
        assert!(UNITS_REQUEST_TOPIC.starts_with("action/"));
        // `get-` marks it read-only ⇒ audit-exempt (mde_bus::persist::is_auditable).
        assert!(UNITS_REQUEST_TOPIC.contains("/get-"));
    }

    #[test]
    fn empty_and_object_bodies_answer_with_the_current_stream() {
        let state = sample_state();
        for body in ["", "  ", "{}", r#"{"category":"mesh"}"#] {
            let reply = handle_units_request(body, &state);
            assert!(reply.ok, "body {body:?} should be accepted");
            assert_eq!(reply.state.as_ref().expect("state").host, "node-a");
            assert!(reply.error.is_none());
        }
    }

    #[test]
    fn malformed_body_is_a_typed_rejection_not_a_panic() {
        let state = sample_state();
        // A JSON array is not a request object → rejected, no state leaked.
        let reply = handle_units_request("[1,2,3]", &state);
        assert!(!reply.ok);
        assert!(reply.state.is_none());
        assert!(reply.error.expect("error").contains("bad units request"));
    }

    #[test]
    fn reply_carries_the_derived_edge_set_alongside_units() {
        // E9: the read verb returns units + edges over the same request. A client
        // that fires `action/units/get-stream` gets the typed connectivity too.
        use super::super::edges::{Edge, EdgeKind};
        let mut state = sample_state();
        state.edges = vec![Edge {
            kind: EdgeKind::HostPlacement,
            from: "cloud:instance:i1".into(),
            to: peer_unit_id("node-a"),
            detail: Some("runs on node-a".into()),
        }];
        let reply = handle_units_request("{}", &state);
        assert!(reply.ok);
        let body = reply.to_body();
        // The edge rides inside the reply's state, so a decode recovers it.
        let back: UnitsReply = serde_json::from_str(&body).expect("decode");
        let edges = back.state.expect("state").edges;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, EdgeKind::HostPlacement);
        assert_eq!(edges[0].to, peer_unit_id("node-a"));
    }

    #[test]
    fn reply_body_round_trips_and_carries_the_ok_flag() {
        let reply = UnitsReply::answer(sample_state());
        let body = reply.to_body();
        assert!(body.contains(r#""ok":true"#));
        let back: UnitsReply = serde_json::from_str(&body).expect("decode");
        assert!(back.ok);
        assert_eq!(back.state.expect("state").units.len(), 1);
        // A rejection encodes ok:false + the reason, no state.
        let rej = UnitsReply::rejected("nope").to_body();
        assert!(rej.contains(r#""ok":false"#));
        assert!(rej.contains("nope"));
        assert!(!rej.contains(r#""state""#));
    }
}
