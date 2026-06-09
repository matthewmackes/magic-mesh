//! `dev.mackes.MDE.Fleet` — fleet control (push setting revisions,
//! list, diff, rollback), served on the mesh **Bus** at
//! `action/fleet/<verb>` (E0.3.3), replacing the retired
//! `dev.mackes.MDE.Fleet` D-Bus interface.
//!
//! Phase A shipped the interface shell; the verbs are still STUBS
//! ("not implemented until v2.0.0 Phase G"). Per the migrate-all
//! disposition (operator, 2026-06-04) the surface moves onto the
//! Bus now — the responder replies the stub envelope — so Phase G
//! fills in the real revision logic on the Bus (in [`build_reply`]),
//! not on D-Bus. The Workbench fleet panels already drive
//! push/rollback via the `mackesd` CLI; the only Bus reader today is
//! home.rs `probe_fleet_revision` (list-revisions → "no revisions").
//!
//! The old `revision_applied` D-Bus signal retires with the
//! interface: nothing emits it today, so there is no Bus event topic
//! yet — Phase G adds `event/fleet/signals` + a worker emitter +
//! the Workbench subscription when revision-apply actually lands.

#![cfg(feature = "async-services")]

use std::collections::HashMap;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// Fleet control service. Stateless today (the verbs are Phase-G
/// stubs); kept as the responder handle so Phase G can hang the
/// revision store off it without changing call sites.
#[derive(Debug, Default, Clone)]
pub struct FleetService;

/// Action verbs served on `action/fleet/<verb>` (E0.3.3). All are
/// Phase-G stubs today.
pub const ACTION_VERBS: [&str; 4] = [
    "push-revision",
    "list-revisions",
    "diff-revisions",
    "rollback",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for verb `verb`: `action/fleet/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/fleet/{verb}")
}

/// Build the reply body for one `action/fleet/<verb>` request. Every
/// verb is a Phase-G stub today, so the reply is the not-implemented
/// error envelope; consumers surface it as "no fleet data" (home.rs
/// `probe_fleet_revision` → "No revisions pushed yet"). Phase G
/// replaces these arms with real revision logic over the store.
#[must_use]
pub fn build_reply(_svc: &FleetService, verb: &str) -> String {
    let msg = match verb {
        "push-revision" | "list-revisions" | "diff-revisions" | "rollback" => {
            format!("Fleet.{verb} — not implemented until v2.0.0 Phase G")
        }
        other => format!("unknown fleet verb: {other}"),
    };
    json!({ "error": msg }).to_string()
}

/// Run the Fleet Bus responder loop on the current thread until
/// `should_stop()`. No tokio runtime needed (the stub builders are
/// synchronous; `Persist`/rusqlite isn't `Send`, so `mackesd`
/// `run_serve` spawns this on a dedicated OS thread — same shape as
/// the Shell responder).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &FleetService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test can
/// drive it without the sleep loop). For each new request on
/// `action/fleet/<verb>`, writes [`build_reply`] to `reply/<ulid>`.
pub fn poll_once(persist: &Persist, svc: &FleetService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "fleet responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = build_reply(svc, verb);
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "fleet responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_verbs_and_topic_lock() {
        assert_eq!(
            ACTION_VERBS,
            [
                "push-revision",
                "list-revisions",
                "diff-revisions",
                "rollback"
            ]
        );
        assert_eq!(
            action_topic("list-revisions"),
            "action/fleet/list-revisions"
        );
    }

    #[test]
    fn build_reply_stubs_until_phase_g() {
        let svc = FleetService;
        for v in ACTION_VERBS {
            assert!(build_reply(&svc, v).contains("not implemented until v2.0.0 Phase G"));
        }
        assert!(build_reply(&svc, "bogus").contains("unknown fleet verb"));
    }
}
