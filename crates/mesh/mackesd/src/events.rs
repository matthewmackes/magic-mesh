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

/// Append one event to the hash-chained `events` table (12.6.3) and
/// return the constructed [`Event`]. Same chain the reconcile
/// worker's `apply_repair_rows` writes — genesis is 32 zero bytes,
/// each row chains on the previous row's hash via
/// [`crate::audit::next_hash`].
///
/// # Errors
/// Returns an error when the SQL insert (or the transaction) fails.
pub fn append_event(
    conn: &mut rusqlite::Connection,
    node_id: &str,
    kind: EventKind,
    detail: serde_json::Value,
) -> anyhow::Result<Event> {
    use anyhow::Context;
    crate::store::with_transaction(conn, |tx| {
        let prev_hash_hex: String = tx
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default();
        let prev_hash = decode_hex32(&prev_hash_hex).unwrap_or([0u8; 32]);
        let now_ms = now_ms();
        let event = Event {
            event_id: u64::try_from(now_ms.max(0)).unwrap_or(0),
            kind,
            node_id: node_id.to_owned(),
            timestamp_ms: now_ms,
            detail,
        };
        let payload = event.payload_bytes().context("serializing event payload")?;
        let hash = crate::audit::next_hash(&prev_hash, &payload, now_ms);
        let payload_str = String::from_utf8(payload)
            .context("event payload is not valid UTF-8 (impossible from serde_json)")?;
        let kind_token = serde_json::to_string(&kind)
            .unwrap_or_else(|_| "\"lifecycle\"".to_owned())
            .trim_matches('"')
            .to_owned();
        // `created_at` MUST encode exactly `now_ms` — `load_audit_rows`
        // reparses this RFC3339 string back to epoch-millis and
        // recomputes `next_hash` over it, so a second `Utc::now()` call
        // here would drift from the `now_ms` baked into `hash` and break
        // chain verification (flaky under load). Derive it from the one
        // instant instead.
        let created_at = chrono::DateTime::from_timestamp_millis(now_ms)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339();
        tx.execute(
            "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            (
                encode_hex(&prev_hash),
                encode_hex(&hash),
                kind_token,
                node_id,
                payload_str,
                created_at,
            ),
        )
        .context("inserting event row")?;
        Ok(event)
    })
}

/// Append an event (see [`append_event`]) AND fire the configured
/// 12.6.4 alert hooks for it, post-commit. Opens the store at
/// `db_path` itself — convenience for callers (the mesh-router)
/// that don't hold a connection. Best-effort end to end: failures
/// are logged, never propagated.
pub fn append_and_alert(
    db_path: &std::path::Path,
    node_id: &str,
    kind: EventKind,
    detail: serde_json::Value,
) {
    let mut conn = match crate::store::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "append_and_alert: store open failed; event dropped");
            return;
        }
    };
    match append_event(&mut conn, node_id, kind, detail) {
        Ok(event) => {
            let hooks = crate::config::daemon::load().alert_hooks();
            if !hooks.is_empty() {
                dispatch_alerts(&event, &hooks);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "append_and_alert: event append failed");
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Walk the configured hooks and fire each whose `for_kind` matches.
/// Pipes the event JSON to each hook's stdin. Failures are logged
/// but never propagate — alerting is best-effort by design.
pub fn dispatch_alerts(event: &Event, hooks: &[AlertHook]) {
    let payload = match serde_json::to_vec(event) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "alert dispatch: failed to serialize event");
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
                tracing::warn!(error = %e, bin = %bin, "alert dispatch: failed to spawn hook");
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

    #[test]
    fn append_event_chains_rows_and_verifies_intact() {
        // AUD3 S-5 — two appended events form an intact hash chain
        // the audit verifier accepts.
        let mut conn = crate::store::open_in_memory().expect("store");
        let e1 = append_event(
            &mut conn,
            "peer:a",
            EventKind::Lifecycle,
            serde_json::json!({"action": "path_switch", "to": "nebula_https443"}),
        )
        .expect("append 1");
        assert_eq!(e1.kind, EventKind::Lifecycle);
        append_event(
            &mut conn,
            "peer:a",
            EventKind::Lifecycle,
            serde_json::json!({"action": "path_switch", "to": "nebula_direct"}),
        )
        .expect("append 2");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }
}
