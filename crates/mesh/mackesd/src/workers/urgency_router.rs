//! Portal-57.a (v6.0, R12-Q22 — channel 1 of 3) — sway-urgent →
//! Mackes Bus publish.
//!
//! Subscribes to sway's `EventType::Window`. On every
//! `WindowChange::Urgent` event where the container's `urgent`
//! flag is `true`, the worker publishes a JSON envelope to the
//! Bus topic `bus/mbadge/pulse` carrying `{tier, source, con_id}`.
//! Downstream consumers (Portal-57.b mini-tree + Portal-57.c Dock
//! segment, both deferred until their UI prerequisites ship)
//! subscribe to that topic to render the M-badge crit-pulse +
//! mini-tree cell pulse + breadcrumb segment.
//!
//! Publishing happens via a subprocess call to `mde-bus publish`
//! rather than linking the `mde-bus` crate directly so mackesd
//! avoids the rusqlite + tera + ulid dep tree. Subprocess
//! invocation is cheap relative to urgency-event frequency
//! (handful per session at most). When the `mde-bus` binary
//! isn't installed (development boxes), the worker logs a
//! single warning per attempted publish + continues — the
//! `bus_supervisor` worker is responsible for getting the
//! binary into place; urgency_router gracefully degrades.

#![cfg(feature = "async-services")]

use std::time::Duration;

use futures_util::StreamExt as _;
use swayipc_async::{Connection, EventType};
use tokio::process::Command;

use super::{ShutdownToken, Worker};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Bus topic the M-badge + mini-tree pulse subscribers listen on.
pub const URGENT_TOPIC: &str = "bus/mbadge/pulse";

/// Empty-state worker.
pub struct UrgencyRouterWorker;

impl UrgencyRouterWorker {
    /// Construct a fresh worker.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for UrgencyRouterWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for UrgencyRouterWorker {
    fn name(&self) -> &'static str {
        "urgency_router"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "urgency_router event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Window]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "urgency_router subscribe failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => return Ok(()),
                    next = events.next() => {
                        match next {
                            Some(Ok(swayipc_async::Event::Window(win_ev))) => {
                                if win_ev.change == swayipc_async::WindowChange::Urgent
                                    && win_ev.container.urgent
                                {
                                    publish_urgent(&win_ev.container).await;
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "urgency_router event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("urgency_router event stream ended; reconnecting");
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn sleep_or_shutdown(dur: Duration, shutdown: &mut ShutdownToken) {
    tokio::select! {
        _ = shutdown.wait() => {}
        _ = tokio::time::sleep(dur) => {}
    }
}

async fn publish_urgent(container: &swayipc_async::Node) {
    let con_id = container.id;
    let app_id = container.app_id.as_deref().unwrap_or("");
    let body = urgent_pulse_payload(app_id, con_id);
    let mut cmd = Command::new("mde-bus");
    cmd.arg("publish")
        .arg(URGENT_TOPIC)
        .arg("--priority")
        .arg("crit")
        .arg("--body-flag")
        .arg(&body);
    match cmd.status().await {
        Ok(status) if status.success() => {
            tracing::debug!(con_id, %app_id, "urgency_router published");
        }
        Ok(status) => {
            tracing::warn!(
                con_id,
                %app_id,
                exit = ?status.code(),
                "urgency_router mde-bus publish exited non-zero"
            );
        }
        Err(e) => {
            tracing::warn!(
                con_id,
                %app_id,
                error = %e,
                "urgency_router could not spawn mde-bus (graceful-degrade)"
            );
        }
    }
}

// ── Pure helpers (testable without a sway connection) ───────────────────

/// Build the JSON body for the `bus/mbadge/pulse` topic. Shape:
///
///   `{"tier":"crit","source":"<app_id>","con_id":<n>}`
///
/// Per the R12-Q22 design lock: tier is always `crit` (sway only
/// emits urgent for high-attention windows); source is the app_id
/// the M-badge subscriber renders in its tooltip; con_id is the
/// container ID the click handler dispatches against.
///
/// `app_id` is JSON-string-escaped so unusual app_ids (with quotes,
/// backslashes, etc.) don't break the parser downstream.
#[must_use]
pub fn urgent_pulse_payload(app_id: &str, con_id: i64) -> String {
    let escaped = json_string_escape(app_id);
    format!(r#"{{"tier":"crit","source":"{escaped}","con_id":{con_id}}}"#)
}

fn json_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical payload shape per R12-Q22.
    #[test]
    fn urgent_pulse_payload_canonical_shape() {
        let body = urgent_pulse_payload("foot", 42);
        assert_eq!(body, r#"{"tier":"crit","source":"foot","con_id":42}"#);
    }

    /// Empty app_id (xwayland windows that don't set one) still
    /// produces a parseable envelope — the source slot is "".
    #[test]
    fn urgent_pulse_payload_empty_app_id() {
        let body = urgent_pulse_payload("", 7);
        assert_eq!(body, r#"{"tier":"crit","source":"","con_id":7}"#);
    }

    /// Quotes + backslashes in app_id get JSON-escaped so the
    /// downstream parser doesn't choke.
    #[test]
    fn urgent_pulse_payload_escapes_quotes_and_backslashes() {
        let body = urgent_pulse_payload(r#"quirky"app"#, 99);
        assert_eq!(
            body,
            r#"{"tier":"crit","source":"quirky\"app","con_id":99}"#
        );
        let body = urgent_pulse_payload(r"path\with\slashes", 100);
        assert_eq!(
            body,
            r#"{"tier":"crit","source":"path\\with\\slashes","con_id":100}"#
        );
    }

    /// Control characters get \uXXXX-escaped.
    #[test]
    fn urgent_pulse_payload_escapes_control_chars() {
        let body = urgent_pulse_payload("with\tnewline\n", 5);
        assert_eq!(
            body,
            r#"{"tier":"crit","source":"with\tnewline\n","con_id":5}"#
        );
    }

    /// Negative con_ids (shouldn't happen in practice but lock the
    /// formatting contract) round-trip without quoting.
    #[test]
    fn urgent_pulse_payload_handles_negative_con_id() {
        let body = urgent_pulse_payload("foot", -1);
        assert_eq!(body, r#"{"tier":"crit","source":"foot","con_id":-1}"#);
    }

    /// The payload is valid JSON (lock so downstream parsers can
    /// rely on it).
    #[test]
    fn urgent_pulse_payload_is_valid_json() {
        let body = urgent_pulse_payload(r#"weird "app" name"#, 42);
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("payload must be valid JSON");
        assert_eq!(parsed["tier"], "crit");
        assert_eq!(parsed["source"], r#"weird "app" name"#);
        assert_eq!(parsed["con_id"], 42);
    }

    /// Topic constant matches the R12-Q22 design lock.
    #[test]
    fn urgent_topic_matches_design_lock() {
        assert_eq!(URGENT_TOPIC, "bus/mbadge/pulse");
    }
}
