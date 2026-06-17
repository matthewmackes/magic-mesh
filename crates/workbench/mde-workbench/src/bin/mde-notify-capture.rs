//! `mde-notify-capture` — NOTIFY-SRC-2. Capture desktop-app freedesktop
//! notifications (Firefox, etc.) into the MDE bus so they show in the Alert
//! Center + federate mesh-wide (NOTIFY-DIST-2).
//!
//! On a Cosmic workstation, `cosmic-notifications` owns
//! `org.freedesktop.Notifications`, so MDE never sees an app's `Notify` call —
//! app notifications went to Cosmic only and never reached the MDE bus. Rather
//! than fight for the bus name (which would replace Cosmic's notification UI),
//! this runs as a **session-bus monitor** (`org.freedesktop.DBus.Monitoring`
//! `BecomeMonitor`, §2 FDO interop): it passively observes every `Notify`
//! method call alongside Cosmic and republishes each to `fdo/<app>` on the MDE
//! bus. The mackesd `alert-mirror` worker then federates it; the panel renders.
//!
//! Session helper (uid 1000): autostarted in the Cosmic session next to
//! `mde-notify-toast`/`-center`. Idempotent + best-effort; a publish failure is
//! logged, never fatal.

use std::collections::HashMap;

use futures_util::stream::StreamExt;
use mde_bus::hooks::config::Priority;
use zbus::zvariant::OwnedValue;

/// The `Notify` method signature: app_name, replaces_id, app_icon, summary,
/// body, actions, hints, expire_timeout.
type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

fn local_host() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// freedesktop urgency hint (0 low / 1 normal / 2 critical) → (severity field,
/// bus priority). Critical desktop notifications map to our Critical severity.
fn urgency_to_severity(hints: &HashMap<String, OwnedValue>) -> (&'static str, Priority) {
    let urgency = hints.get("urgency").and_then(|v| match &**v {
        zbus::zvariant::Value::U8(n) => Some(*n),
        _ => None,
    });
    match urgency {
        Some(2) => ("crit", Priority::High),
        Some(0) => ("info", Priority::Min),
        _ => ("info", Priority::Default), // normal / unspecified
    }
}

/// Build the `fdo/<app>` topic + JSON body for one captured notification. Pure +
/// testable. The panel's `alert_from_message` reads `title`/`summary`/`severity`.
fn capture_to_bus(app: &str, summary: &str, body: &str, sev: &str, host: &str) -> (String, String) {
    let app = if app.trim().is_empty() {
        "app"
    } else {
        app.trim()
    };
    let topic = format!("fdo/{app}");
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let json = format!(
        r#"{{"appName":"{}","title":"{}","summary":"{}","severity":"{}","host":"{}"}}"#,
        esc(app),
        esc(summary),
        esc(body),
        sev,
        esc(host),
    );
    (topic, json)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = local_host();
    let conn = zbus::connection::Builder::session()?.build().await?;

    // Become a passive monitor for Notify method calls only — we never disturb
    // Cosmic's handling; we just observe.
    let monitor = zbus::fdo::MonitoringProxy::new(&conn).await?;
    let rule = zbus::MatchRule::try_from(
        "interface='org.freedesktop.Notifications',member='Notify',type='method_call'",
    )?;
    monitor.become_monitor(&[rule], 0).await?;
    eprintln!(
        "mde-notify-capture: monitoring org.freedesktop.Notifications.Notify on the session bus"
    );

    let mut stream = zbus::MessageStream::from(&conn);
    while let Some(Ok(msg)) = stream.next().await {
        let header = msg.header();
        let is_notify = header
            .interface()
            .is_some_and(|i| i.as_str() == "org.freedesktop.Notifications")
            && header.member().is_some_and(|m| m.as_str() == "Notify");
        if !is_notify {
            continue;
        }
        let Ok((app, _replaces, _icon, summary, body, _actions, hints, _expire)) =
            msg.body().deserialize::<NotifyArgs>()
        else {
            continue;
        };
        let (sev, prio) = urgency_to_severity(&hints);
        let (topic, json) = capture_to_bus(&app, &summary, &body, sev, &host);
        if let Some(dir) = mde_bus::client_data_dir() {
            match mde_bus::persist::Persist::open(dir) {
                Ok(persist) => {
                    if let Err(e) = persist.write(&topic, prio, Some(&summary), Some(&json)) {
                        eprintln!("mde-notify-capture: bus write failed: {e}");
                    }
                }
                Err(e) => eprintln!("mde-notify-capture: bus open failed: {e}"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_builds_topic_and_json() {
        let (topic, json) = capture_to_bus(
            "Firefox",
            "Download done",
            "report.pdf",
            "info",
            "UNIT-EAGLE",
        );
        assert_eq!(topic, "fdo/Firefox");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["appName"], "Firefox");
        assert_eq!(v["title"], "Download done");
        assert_eq!(v["summary"], "report.pdf");
        assert_eq!(v["severity"], "info");
        assert_eq!(v["host"], "UNIT-EAGLE");
    }

    #[test]
    fn empty_app_falls_back_and_quotes_escape() {
        let (topic, json) = capture_to_bus("", "He said \"hi\"", "a\\b", "crit", "h");
        assert_eq!(topic, "fdo/app");
        // Must round-trip as valid JSON despite quotes/backslashes.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["title"], "He said \"hi\"");
        assert_eq!(v["summary"], "a\\b");
    }

    #[test]
    fn urgency_maps_to_severity() {
        let mk = |n: u8| OwnedValue::try_from(zbus::zvariant::Value::U8(n)).unwrap();
        let mut h: HashMap<String, OwnedValue> = HashMap::new();
        assert_eq!(urgency_to_severity(&h).0, "info"); // unspecified → normal
        h.insert("urgency".into(), mk(2));
        assert_eq!(urgency_to_severity(&h).0, "crit");
        h.insert("urgency".into(), mk(0));
        assert_eq!(urgency_to_severity(&h).0, "info");
    }
}
