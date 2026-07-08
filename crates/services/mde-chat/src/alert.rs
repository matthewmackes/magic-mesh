//! The pure **alert-fold** (lock 11): a real Bus alert lane → a [`Message`] from
//! the originating host. Every current + future alert lane the worker subscribes
//! flows through [`fold_alert`] with **no emitter changes** — the payload's
//! severity drives styling, the topic drives the source flag, and the origin
//! host becomes the message sender (so a `nyc3` security alert reads as a message
//! from the `nyc3` contact on every node's roster).
//!
//! This is the seam that unifies notifications into the chat timeline; it is
//! pure (no clock, no I/O — the send time is read from the payload or defaults),
//! so it is exhaustively unit-tested against realistic Bus JSON shapes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::message::{AlertAction, AlertActionKind, Message, MessageKind};

/// Alert severity — the color + mute axis (lock 16).
///
/// Ordered most-severe first, so a per-severity threshold is a simple
/// comparison. This is the `mde-notify` `Severity` absorbed into a chat message
/// kind (lock 18); the three levels the design calls for (Info / Warning /
/// Critical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Needs attention now (red): `crit`/`critical`/`error`/`fatal`, or Bus
    /// priority `urgent`.
    Critical,
    /// Worth noticing (amber): `warn`/`warning`, or Bus priority `high`.
    Warning,
    /// Informational (blue): everything else — the default.
    Info,
}

impl Severity {
    /// Map an explicit `severity` string (the payload field) to a level. The
    /// field wins over Bus priority; see [`classify_severity`].
    #[must_use]
    fn from_field(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "crit" | "critical" | "error" | "err" | "fatal" | "urgent" => Some(Self::Critical),
            "warn" | "warning" | "high" => Some(Self::Warning),
            "info" | "notice" | "debug" | "low" | "min" | "default" => Some(Self::Info),
            _ => None,
        }
    }

    /// A short stable tag (`critical`/`warning`/`info`).
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// Resolve a severity from the payload `severity` field (preferred) and/or the
/// Bus `priority` (fallback) — "field AND/OR Priority", the NOTIFY-2 rule.
/// Unknown → [`Severity::Info`].
#[must_use]
fn classify_severity(severity_field: Option<&str>, priority: Option<&str>) -> Severity {
    if let Some(sev) = severity_field.and_then(Severity::from_field) {
        return sev;
    }
    priority
        .and_then(Severity::from_field)
        .unwrap_or(Severity::Info)
}

/// Map a Bus `topic` to its short **source flag** (lock 11: the flag comes from
/// the topic).
///
/// Used for the alert card's badge + the per-source mute (lock 16). Mirrors the
/// `mde-notify` source grouping, collapsed to a single stable token.
#[must_use]
pub fn alert_flag(topic: &str) -> &'static str {
    let t = topic.trim();
    if t == "fleet/sec" || t.starts_with("fleet/sec/") || t.contains("security") {
        "security"
    } else if t.starts_with("event/firewall") {
        "firewall"
    } else if t.starts_with("event/notify/browser") {
        "browser"
    } else if t.starts_with("compute/event") {
        "compute"
    } else if t.contains("presence") {
        "presence"
    } else if t.starts_with("fdo/") {
        "desktop"
    } else {
        "system"
    }
}

/// The payload keys that get their own dedicated slot on the folded
/// [`MessageKind::Alert`] and so are *not* duplicated into its `fields` map.
const RESERVED_KEYS: [&str; 4] = ["severity", "priority", "action", "actions"];

fn parse_action_kind(v: &serde_json::Value) -> AlertActionKind {
    v.get("kind")
        .and_then(serde_json::Value::as_str)
        .or_else(|| v.get("type").and_then(serde_json::Value::as_str))
        .map(|s| match s.trim().to_ascii_lowercase().as_str() {
            "destructive" | "danger" | "armed" => AlertActionKind::Destructive,
            "ack" | "acknowledge" => AlertActionKind::Ack,
            "snooze" => AlertActionKind::Snooze,
            _ => AlertActionKind::Safe,
        })
        .unwrap_or(AlertActionKind::Safe)
}

fn parse_actions(obj: Option<&serde_json::Map<String, serde_json::Value>>) -> Vec<AlertAction> {
    let mut actions = Vec::new();
    if let Some(values) = obj
        .and_then(|o| o.get("actions"))
        .and_then(serde_json::Value::as_array)
    {
        for (idx, value) in values.iter().enumerate() {
            let Some(action) = value.as_object() else {
                continue;
            };
            let label = action
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Action")
                .trim();
            if label.is_empty() {
                continue;
            }
            let id = action
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map_or_else(|| format!("action-{idx}"), str::to_string);
            let verb = action
                .get("verb")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
            actions.push(AlertAction {
                id,
                label: label.to_string(),
                verb,
                kind: parse_action_kind(value),
            });
        }
    }
    actions
}

/// **The alert-fold** (lock 11): turn one Bus alert into a [`Message`] from the
/// originating host `origin_host`.
///
/// `payload_json` is the raw Bus message body (parsed as JSON when it is; a
/// non-JSON body degrades to a single `body`
/// field). Severity comes from the payload (`severity` field, else `priority`),
/// the `flag` from the `topic` ([`alert_flag`]), the inline action from an
/// `action` field, and every remaining string field is preserved in `fields`
/// (ordered — a `BTreeMap` — so the resulting message signs deterministically).
///
/// The message is returned **unsigned** and stamped with the payload's
/// `ts_unix_ms` when present (else `0`): the worker signs it with the node key
/// and, if it minted the time, can override before signing — this stays pure.
#[must_use]
pub fn fold_alert(bus_topic: &str, payload_json: &str, origin_host: &str) -> Message {
    let parsed: Option<serde_json::Value> = serde_json::from_str(payload_json).ok();
    let obj = parsed.as_ref().and_then(serde_json::Value::as_object);

    let str_field = |k: &str| -> Option<String> {
        obj.and_then(|o| o.get(k))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };

    let severity = classify_severity(
        str_field("severity").as_deref(),
        str_field("priority").as_deref(),
    );
    let action_verb = str_field("action");
    let mut actions = parse_actions(obj);
    if actions.is_empty() {
        if let Some(verb) = &action_verb {
            actions.push(AlertAction {
                id: "open".to_string(),
                label: "Open".to_string(),
                verb: Some(verb.clone()),
                kind: AlertActionKind::Safe,
            });
        }
    }

    // Every remaining string field becomes part of the card body, ordered.
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    if let Some(o) = obj {
        for (k, v) in o {
            if RESERVED_KEYS.contains(&k.as_str()) {
                continue;
            }
            if let Some(s) = v.as_str() {
                fields.insert(k.clone(), s.to_string());
            }
        }
    }
    // A non-JSON (or non-object) body isn't lost — carry it as the summary.
    if obj.is_none() && !payload_json.trim().is_empty() {
        fields.insert("body".to_string(), payload_json.trim().to_string());
    }

    let ts_unix_ms = obj
        .and_then(|o| o.get("ts_unix_ms"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);

    Message::new(
        origin_host,
        ts_unix_ms,
        MessageKind::Alert {
            severity,
            flag: alert_flag(bus_topic).to_string(),
            fields,
            action_verb,
            actions,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering_is_most_severe_first() {
        assert!(Severity::Critical < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
    }

    #[test]
    fn classify_prefers_field_then_priority_then_info() {
        assert_eq!(
            classify_severity(Some("crit"), Some("default")),
            Severity::Critical
        );
        assert_eq!(
            classify_severity(Some("warn"), Some("urgent")),
            Severity::Warning
        );
        assert_eq!(classify_severity(None, Some("urgent")), Severity::Critical);
        assert_eq!(classify_severity(None, Some("high")), Severity::Warning);
        assert_eq!(classify_severity(None, None), Severity::Info);
        assert_eq!(
            classify_severity(Some("weird"), Some("weird")),
            Severity::Info
        );
    }

    #[test]
    fn flag_comes_from_the_topic() {
        assert_eq!(alert_flag("event/security/alert"), "security");
        assert_eq!(alert_flag("fleet/sec"), "security");
        assert_eq!(alert_flag("event/firewall/host-a"), "firewall");
        assert_eq!(alert_flag("event/notify/browser"), "browser");
        assert_eq!(alert_flag("compute/event/node2"), "compute");
        assert_eq!(alert_flag("peer/x/presence"), "presence");
        assert_eq!(alert_flag("fdo/firefox"), "desktop");
        assert_eq!(alert_flag("mackesd::alert"), "system");
    }

    #[test]
    fn folds_a_realistic_security_alert_from_the_origin_host() {
        // A realistic event/security/alert Bus body.
        let payload = r#"{
            "severity": "critical",
            "priority": "urgent",
            "alert": "nebula.cert.revoked",
            "summary": "peer certificate was revoked by the CA",
            "host": "nyc3",
            "action": "action/shell/goto",
            "ts_unix_ms": 1720000000000
        }"#;
        let msg = fold_alert("event/security/alert", payload, "nyc3");

        assert_eq!(
            msg.sender, "nyc3",
            "the alert is a message from its origin host"
        );
        assert_eq!(msg.ts_unix_ms, 1_720_000_000_000);
        let MessageKind::Alert {
            severity,
            flag,
            fields,
            action_verb,
            actions,
        } = &msg.kind
        else {
            unreachable!("expected an Alert kind, got {}", msg.kind.tag());
        };
        assert_eq!(*severity, Severity::Critical);
        assert_eq!(flag, "security", "flag derived from the topic");
        assert_eq!(action_verb.as_deref(), Some("action/shell/goto"));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].label, "Open");
        assert_eq!(actions[0].verb.as_deref(), Some("action/shell/goto"));
        assert_eq!(actions[0].kind, AlertActionKind::Safe);
        assert_eq!(
            fields.get("alert").map(String::as_str),
            Some("nebula.cert.revoked")
        );
        assert_eq!(
            fields.get("summary").map(String::as_str),
            Some("peer certificate was revoked by the CA")
        );
        assert_eq!(fields.get("host").map(String::as_str), Some("nyc3"));
        // Reserved keys are not duplicated into fields.
        assert!(!fields.contains_key("severity"));
        assert!(!fields.contains_key("action"));
    }

    #[test]
    fn folds_configured_typed_action_set() {
        let payload = r#"{
            "severity": "critical",
            "summary": "unit failed",
            "host": "eagle",
            "actions": [
                {"id":"restart","label":"Restart","verb":"action/systemd/restart","kind":"safe"},
                {"id":"poweroff","label":"Power Off","verb":"action/power/off","kind":"destructive"},
                {"id":"ack","label":"Ack","kind":"ack"},
                {"id":"later","label":"Snooze","kind":"snooze"}
            ]
        }"#;
        let msg = fold_alert("event/notify/service", payload, "eagle");
        let MessageKind::Alert {
            fields, actions, ..
        } = &msg.kind
        else {
            unreachable!("expected Alert");
        };
        assert_eq!(actions.len(), 4);
        assert_eq!(actions[0].kind, AlertActionKind::Safe);
        assert_eq!(actions[1].kind, AlertActionKind::Destructive);
        assert_eq!(actions[2].kind, AlertActionKind::Ack);
        assert_eq!(actions[3].kind, AlertActionKind::Snooze);
        assert!(!fields.contains_key("actions"));
    }

    #[test]
    fn folds_severity_from_priority_when_no_field() {
        let msg = fold_alert(
            "event/firewall/h",
            r#"{"priority":"high","summary":"scan"}"#,
            "eagle",
        );
        let MessageKind::Alert { severity, flag, .. } = &msg.kind else {
            unreachable!("expected Alert");
        };
        assert_eq!(*severity, Severity::Warning);
        assert_eq!(flag, "firewall");
    }

    #[test]
    fn degrades_a_non_json_body_into_a_summary_field() {
        let msg = fold_alert("fdo/firefox", "Download complete", "eagle");
        let MessageKind::Alert {
            severity,
            fields,
            action_verb,
            ..
        } = &msg.kind
        else {
            unreachable!("expected Alert");
        };
        assert_eq!(*severity, Severity::Info, "no severity → Info");
        assert_eq!(
            fields.get("body").map(String::as_str),
            Some("Download complete")
        );
        assert!(action_verb.is_none());
    }

    #[test]
    fn a_folded_alert_can_be_signed_and_verified() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;
        let mut msg = fold_alert("event/security/alert", r#"{"severity":"warn"}"#, "nyc3");
        crate::message::sign(&mut msg, &SigningKey::generate(&mut OsRng));
        assert!(
            msg.verify(),
            "a folded alert signs + verifies like any message"
        );
    }
}
