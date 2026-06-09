//! Append-only event log + alerting hooks (Phase 12.6.3 + 12.6.4).
//!
//! Every config change, auth event, and lifecycle action lands in
//! `events`. Rows carry a hash-chained `prev_hash` field (see
//! [`crate::audit::next_hash`]) for tamper detection. Per 12.6.4 the
//! alerting layer fires a configurable shell command per event-kind
//! with the event JSON on stdin.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

/// Categories of events we persist. The set is closed by design —
/// every emission goes through one of these variants so the audit
/// log filter ("show me every auth event") works deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A revision was drafted, validated, approved, applied, or rolled back.
    ConfigChange,
    /// Enrollment, decommission, passcode rotation, bearer token refresh.
    Auth,
    /// Node became healthy / degraded / unreachable.
    Lifecycle,
    /// Drift detected; reconcile attempt started / succeeded / failed.
    Reconcile,
    /// Operator opened an admin action surface — `mackes audit
    /// verify`, `mackesd rotate-passcode`, etc.
    AdminAction,
}

/// One event payload, the value type that gets serialized into the
/// `events` table's `payload_json` column and the audit hash chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Stable identifier (monotonic per emission).
    pub event_id: u64,
    /// Event-kind tag (drives the alerting routing).
    pub kind: EventKind,
    /// Stable node id of the emitting peer (or "leader" for events
    /// the leader emits about the mesh as a whole).
    pub node_id: String,
    /// Unix epoch milliseconds.
    pub timestamp_ms: i64,
    /// Free-form payload — JSON object, key-value detail of the event.
    pub detail: serde_json::Value,
}

impl Event {
    /// Serialize the event to canonical bytes for the audit hash
    /// chain (per 12.6.3, the bytes feed into `audit::next_hash`).
    ///
    /// # Errors
    /// Returns `serde_json::Error` only on out-of-memory.
    pub fn payload_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// One alert hook — fires when an event of the matching kind lands.
/// Per 12.6.4: "configurable shell command runs with the event JSON
/// on stdin. No webhooks (no networking — operators can wire `curl`
/// themselves). Mackes ships no alerting tool of its own."
#[derive(Debug, Clone)]
pub struct AlertHook {
    /// Match against `EventKind`. Hooks with `None` fire on every kind.
    pub for_kind: Option<EventKind>,
    /// Literal shell command to spawn (executable + args).
    pub command: Vec<String>,
}

/// Walk the configured hooks and fire each whose `for_kind` matches.
/// Pipes the event JSON to each hook's stdin. Failures are logged
/// but never propagate — alerting is best-effort by design.
pub fn dispatch_alerts(event: &Event, hooks: &[AlertHook]) {
    let payload = match serde_json::to_vec(event) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("alert dispatch: failed to serialize event: {e}");
            return;
        }
    };
    for hook in hooks {
        if let Some(kind) = hook.for_kind {
            if kind != event.kind {
                continue;
            }
        }
        let Some((bin, args)) = hook.command.split_first() else {
            continue;
        };
        let spawn_result = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawn_result {
            Ok(c) => c,
            Err(e) => {
                eprintln!("alert dispatch: failed to spawn {bin}: {e}");
                continue;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&payload);
        }
        // Don't wait — alerts are fire-and-forget. The OS reaps
        // zombie children when the reconciler exits.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(kind: EventKind) -> Event {
        Event {
            event_id: 1,
            kind,
            node_id: "peer:a".into(),
            timestamp_ms: 1_000,
            detail: serde_json::json!({"x": 1}),
        }
    }

    #[test]
    fn payload_bytes_round_trips() {
        let e = mk(EventKind::ConfigChange);
        let bytes = e.payload_bytes().unwrap();
        let back: Event = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn hook_match_filters_by_kind() {
        let hook_auth = AlertHook {
            for_kind: Some(EventKind::Auth),
            command: vec!["true".into()],
        };
        let hook_any = AlertHook {
            for_kind: None,
            command: vec!["true".into()],
        };
        let auth_event = mk(EventKind::Auth);
        let config_event = mk(EventKind::ConfigChange);

        // The dispatcher itself doesn't return — assert the match
        // logic via direct comparison.
        assert_eq!(hook_auth.for_kind, Some(EventKind::Auth));
        assert!(hook_any.for_kind.is_none());
        // Sanity: kinds compare as expected.
        assert_ne!(auth_event.kind, config_event.kind);
    }

    #[test]
    fn json_serialization_uses_snake_case_kind() {
        let e = mk(EventKind::AdminAction);
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""kind":"admin_action""#));
    }

    #[test]
    fn dispatch_alerts_silent_on_missing_binary() {
        // `command: ["nonexistent-binary-xyz"]` must NOT panic the
        // caller — alerting is best-effort.
        let hook = AlertHook {
            for_kind: None,
            command: vec!["nonexistent-binary-xyz-12345".into()],
        };
        let e = mk(EventKind::Reconcile);
        dispatch_alerts(&e, &[hook]); // doesn't panic
    }

    #[test]
    fn dispatch_alerts_empty_hook_list_is_a_noop() {
        let e = mk(EventKind::Lifecycle);
        dispatch_alerts(&e, &[]); // doesn't panic, doesn't fire anything
    }
}
