//! `dev.mackes.MDE.Fleet` — fleet control (push setting revisions,
//! list, diff, rollback), served on the mesh **Bus** at
//! `action/fleet/<verb>` (E0.3.3), replacing the retired
//! `dev.mackes.MDE.Fleet` D-Bus interface.
//!
//! **FPG-4 (2026-06-09): the verbs are real.** They run against the
//! `magic_fleet::store` append-only revision log on the Syncthing
//! workgroup root (FPG-2) — replication is the transport, the
//! directory is the truth. **Leaderless (FPG-3):** any node serves
//! these verbs and any node mints; `next_version` + the append-only
//! write + the `version → at → author` total order make concurrent
//! mints converge identically everywhere. The leader lease guards
//! only per-node SQLite mirror writes, never authorship.
//!
//! Verb contract (request body / reply, all JSON):
//! - `push-revision`  `{spec: <BaselineSpec YAML>, author?}` →
//!   `{ok, version}` — mints the next version.
//! - `list-revisions` `{}` → `{ok, head, revisions: [{version,
//!   author, at}]}` — the full held set tagged with the winner (Q16).
//! - `diff-revisions` `{from, to}` → `{ok, from, to, changed:
//!   [<domain>…]}` — flat top-level domain diff (Q7).
//! - `rollback`       `{target}` → `{ok, version, of}` — mints a
//!   HIGHER version carrying the target's spec; history is immutable
//!   (Q6).
//!
//! `event/fleet/signals` (apply-acks) lands with FPG-5.

#![cfg(feature = "async-services")]

use std::collections::HashMap;

use std::path::PathBuf;

use magic_fleet::{store, BaselineSpec, Revision};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use super::action_auth::{ActionAuthorizer, MutationContext};

/// Fleet control service — owns the revision-log location + the
/// author identity stamped on mints from this node.
#[derive(Debug, Default, Clone)]
pub struct FleetService {
    /// The replicated workgroup root (acks live under it, FPG-5).
    pub workgroup_root: PathBuf,
    /// `<workgroup-root>/fleet/revisions` (FPG-2).
    pub revisions_dir: PathBuf,
    /// This node's id — the default `author` for push/rollback mints.
    pub node_id: String,
}

impl FleetService {
    /// Service rooted at the replicated workgroup root.
    #[must_use]
    pub fn new(workgroup_root: &std::path::Path, node_id: String) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
            revisions_dir: store::revisions_dir(workgroup_root),
            node_id,
        }
    }
}

/// The Bus event topic apply-acks surface on (FPG-5 / Q15). One
/// retained-style event per new `(version, peer, status)` triple:
/// `{revision, peer, status, at}` — the Workbench subscribes here.
pub const SIGNALS_TOPIC: &str = "event/fleet/signals";

/// Flat top-level domain diff (Q7): the domain names whose content
/// differs between two specs.
#[must_use]
pub fn diff_domains(a: &BaselineSpec, b: &BaselineSpec) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if a.packages != b.packages {
        changed.push("packages");
    }
    if a.services != b.services {
        changed.push("services");
    }
    if a.files != b.files {
        changed.push("files");
    }
    if a.users != b.users {
        changed.push("users");
    }
    if a.groups != b.groups {
        changed.push("groups");
    }
    if a.cron != b.cron {
        changed.push("cron");
    }
    if a.sysctl != b.sysctl {
        changed.push("sysctl");
    }
    if a.firewall != b.firewall {
        changed.push("firewall");
    }
    if a.settings != b.settings {
        changed.push("settings");
    }
    changed
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn err(msg: impl std::fmt::Display) -> String {
    json!({ "ok": false, "error": msg.to_string() }).to_string()
}

/// Action verbs served on `action/fleet/<verb>` (E0.3.3).
pub const ACTION_VERBS: [&str; 5] = [
    "push-revision",
    "list-revisions",
    "diff-revisions",
    "rollback",
    "nudge",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for verb `verb`: `action/fleet/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/fleet/{verb}")
}

/// Build the reply body for one `action/fleet/<verb>` request
/// against the revision log (FPG-4). `body` is the request JSON
/// (absent body = `{}`).
#[must_use]
pub fn build_reply(svc: &FleetService, verb: &str, body: Option<&str>) -> String {
    let req: serde_json::Value =
        serde_json::from_str(body.unwrap_or("{}")).unwrap_or(serde_json::Value::Null);
    match verb {
        "push-revision" => {
            let Some(spec_yaml) = req.get("spec").and_then(|v| v.as_str()) else {
                return err("push-revision: missing `spec` (BaselineSpec YAML)");
            };
            let spec = match BaselineSpec::from_yaml(spec_yaml) {
                Ok(s) => s,
                Err(e) => return err(format!("push-revision: bad spec: {e}")),
            };
            let author = req
                .get("author")
                .and_then(|v| v.as_str())
                .unwrap_or(&svc.node_id)
                .to_string();
            let revision = Revision {
                version: store::next_version(&svc.revisions_dir),
                author,
                at: now_s(),
                spec,
            };
            match store::write_revision(&svc.revisions_dir, &revision) {
                Ok(_) => json!({ "ok": true, "version": revision.version }).to_string(),
                Err(e) => err(format!("push-revision: {e}")),
            }
        }
        "list-revisions" => {
            let all = store::read_revisions(&svc.revisions_dir);
            let head = magic_fleet::elect_revision(&all).map(|r| r.version);
            let rows: Vec<_> = all
                .iter()
                .map(|r| {
                    let acks = store::read_acks(&svc.workgroup_root, r.version);
                    let applied = acks.iter().filter(|a| a.status == "applied").count();
                    let failed = acks.iter().filter(|a| a.status == "failed").count();
                    json!({
                        "version": r.version, "author": r.author, "at": r.at,
                        "acks": { "applied": applied, "failed": failed },
                    })
                })
                .collect();
            json!({ "ok": true, "head": head, "revisions": rows }).to_string()
        }
        "diff-revisions" => {
            let (Some(from), Some(to)) = (
                req.get("from").and_then(serde_json::Value::as_u64),
                req.get("to").and_then(serde_json::Value::as_u64),
            ) else {
                return err("diff-revisions: need numeric `from` + `to`");
            };
            let all = store::read_revisions(&svc.revisions_dir);
            let (Some(a), Some(b)) = (
                all.iter().find(|r| r.version == from),
                all.iter().find(|r| r.version == to),
            ) else {
                return err(format!("diff-revisions: unknown version ({from} or {to})"));
            };
            json!({ "ok": true, "from": from, "to": to,
                    "changed": diff_domains(&a.spec, &b.spec) })
            .to_string()
        }
        "rollback" => {
            let Some(target) = req.get("target").and_then(serde_json::Value::as_u64) else {
                return err("rollback: need numeric `target`");
            };
            let all = store::read_revisions(&svc.revisions_dir);
            let Some(old) = all.iter().find(|r| r.version == target) else {
                return err(format!("rollback: unknown version {target}"));
            };
            let revision = Revision {
                version: store::next_version(&svc.revisions_dir),
                author: req
                    .get("author")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&svc.node_id)
                    .to_string(),
                at: now_s(),
                spec: old.spec.clone(),
            };
            match store::write_revision(&svc.revisions_dir, &revision) {
                Ok(_) => {
                    json!({ "ok": true, "version": revision.version, "of": target }).to_string()
                }
                Err(e) => err(format!("rollback: {e}")),
            }
        }
        "nudge" => {
            // PD-9 — "Apply now": write the target's nudge file on the
            // replicated volume; its reconcile worker consumes it and
            // converges to the elected head immediately (Q16 — hurries
            // convergence, never forks state).
            let Some(peer) = req.get("peer").and_then(|v| v.as_str()) else {
                return err("nudge: missing `peer`");
            };
            match magic_fleet::store::write_nudge(&svc.workgroup_root, peer) {
                Ok(_) => json!({ "ok": true, "nudged": peer }).to_string(),
                Err(e) => err(format!("nudge: {e}")),
            }
        }
        other => err(format!("unknown fleet verb: {other}")),
    }
}

fn build_bus_reply(
    svc: &FleetService,
    verb: &str,
    body: Option<&str>,
    authorizer: &ActionAuthorizer,
) -> String {
    if matches!(verb, "push-revision" | "rollback" | "nudge") {
        let raw = body.unwrap_or_default();
        let request = serde_json::from_str::<serde_json::Value>(raw).ok();
        let target = match verb {
            "push-revision" => "baseline".to_string(),
            "rollback" => request
                .as_ref()
                .and_then(|value| value.get("target"))
                .and_then(serde_json::Value::as_u64)
                .map_or_else(String::new, |target| target.to_string()),
            "nudge" => request
                .as_ref()
                .and_then(|value| value.get("peer"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => unreachable!("closed mutation verb set"),
        };
        let auth_verb = format!("fleet-{verb}");
        let context = MutationContext {
            verb: &auth_verb,
            node: &svc.node_id,
            target: &target,
        };
        if let Err(error) = authorizer.authorize(raw, context) {
            tracing::warn!(
                target: "mackesd::fleet",
                %error,
                verb,
                "refused unauthorized fleet mutation"
            );
            return err(format!("{verb} authorization refused: {error}"));
        }
    }
    build_reply(svc, verb, body)
}

/// Run the Fleet Bus responder loop on the current thread until
/// `should_stop()`. No tokio runtime needed (the verb handlers are
/// synchronous filesystem reads/writes; `Persist`/rusqlite isn't
/// `Send`, so `mackesd` `run_serve` spawns this on a dedicated OS
/// thread — same shape as the Shell responder).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &FleetService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    // FPG-5 — the ack→signal emitter's dedup memory. Seeded silently
    // on the first sweep so a responder restart doesn't replay
    // historical acks as fresh notifications.
    let mut seen_acks: std::collections::HashSet<(u64, String, String)> =
        std::collections::HashSet::new();
    let mut first_sweep = true;
    let authorizer = ActionAuthorizer::production();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors, &authorizer);
        emit_new_acks(persist, svc, &mut seen_acks, first_sweep);
        first_sweep = false;
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// FPG-5 — scan the ack dirs for `(version, peer, status)` triples not
/// yet seen and emit each on [`SIGNALS_TOPIC`]. `seed_only` populates
/// the dedup set without emitting (the restart-replay guard).
pub fn emit_new_acks(
    persist: &Persist,
    svc: &FleetService,
    seen: &mut std::collections::HashSet<(u64, String, String)>,
    seed_only: bool,
) {
    for r in store::read_revisions(&svc.revisions_dir) {
        for ack in store::read_acks(&svc.workgroup_root, r.version) {
            let key = (r.version, ack.peer.clone(), ack.status.clone());
            if !seen.insert(key) {
                continue;
            }
            if seed_only {
                continue;
            }
            let body = json!({
                "revision": r.version, "peer": ack.peer,
                "status": ack.status, "at": ack.at,
            })
            .to_string();
            if let Err(e) = persist.write(SIGNALS_TOPIC, Priority::Default, None, Some(&body)) {
                tracing::warn!(error = %e, "fleet responder: signal emit failed");
            }
        }
    }
}

/// One poll sweep across the action verbs (split out so a test can
/// drive it without the sleep loop). For each new request on
/// `action/fleet/<verb>`, writes [`build_reply`] to `reply/<ulid>`.
pub fn poll_once(
    persist: &Persist,
    svc: &FleetService,
    cursors: &mut HashMap<String, String>,
    authorizer: &ActionAuthorizer,
) {
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
            // EFF-23 — refuse an oversized body before build_reply parses it.
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_bus_reply(svc, verb, msg.body.as_deref(), authorizer)
            } else {
                tracing::warn!(
                    topic = %topic,
                    len = msg.body.as_ref().map_or(0, String::len),
                    cap = crate::ipc::MAX_RPC_BODY_BYTES,
                    "fleet responder: body exceeds cap; refusing",
                );
                crate::ipc::body_too_large_reply(verb)
            };
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
    use crate::ipc::action_auth::authorize_test_body;

    const AUTH_KEY: &[u8] = b"fleet-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    #[test]
    fn action_verbs_and_topic_lock() {
        assert_eq!(
            ACTION_VERBS,
            [
                "push-revision",
                "list-revisions",
                "diff-revisions",
                "rollback",
                "nudge"
            ]
        );
        assert_eq!(
            action_topic("list-revisions"),
            "action/fleet/list-revisions"
        );
    }

    fn svc_in(dir: &std::path::Path) -> FleetService {
        FleetService::new(dir, "peer:test".into())
    }

    fn signed_push_body(nonce: &str, expires_at_ms: i64) -> String {
        let unsigned = json!({
            "schema_version": 1,
            "spec": "packages: []\n",
        })
        .to_string();
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: "fleet-push-revision",
                node: "peer:test",
                target: "baseline",
            },
            nonce,
            expires_at_ms,
        )
    }

    #[test]
    fn hostile_bus_mutations_are_refused_and_authorized_push_is_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW);

        for (verb, body) in [
            ("push-revision", r#"{"spec":"packages: []\n"}"#),
            ("rollback", r#"{"target":1}"#),
            ("nudge", r#"{"peer":"oak"}"#),
        ] {
            let reply: serde_json::Value =
                serde_json::from_str(&build_bus_reply(&svc, verb, Some(body), &authorizer))
                    .unwrap();
            assert_eq!(reply["ok"], false, "unsigned {verb} must fail closed");
        }
        assert!(store::read_revisions(&svc.revisions_dir).is_empty());
        assert!(!magic_fleet::store::take_nudge(tmp.path(), "oak"));

        let future = signed_push_body("future", AUTH_NOW + 30_000)
            .replace("\"schema_version\":1", "\"schema_version\":2");
        let overlong = signed_push_body("overlong", AUTH_NOW + 30_001);
        let tampered = signed_push_body("tampered", AUTH_NOW + 30_000)
            .replace("packages: []", "packages: [vim]");
        for body in [&future, &overlong, &tampered] {
            let reply: serde_json::Value = serde_json::from_str(&build_bus_reply(
                &svc,
                "push-revision",
                Some(body),
                &authorizer,
            ))
            .unwrap();
            assert_eq!(reply["ok"], false);
        }
        assert!(store::read_revisions(&svc.revisions_dir).is_empty());

        let replay = signed_push_body("replay", AUTH_NOW + 30_000);
        let first: serde_json::Value = serde_json::from_str(&build_bus_reply(
            &svc,
            "push-revision",
            Some(&replay),
            &authorizer,
        ))
        .unwrap();
        assert_eq!(first["ok"], true);
        let second: serde_json::Value = serde_json::from_str(&build_bus_reply(
            &svc,
            "push-revision",
            Some(&replay),
            &authorizer,
        ))
        .unwrap();
        assert_eq!(second["ok"], false);
        assert_eq!(store::read_revisions(&svc.revisions_dir).len(), 1);

        for verb in ["list-revisions", "diff-revisions"] {
            let reply = build_bus_reply(&svc, verb, None, &authorizer);
            assert!(
                !reply.contains("authorization refused"),
                "{verb} remains an open read"
            );
        }
    }

    #[test]
    fn push_then_list_round_trips_with_head() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let r1 = build_reply(&svc, "push-revision", Some(r#"{"spec": "packages: []\n"}"#));
        let v1: serde_json::Value = serde_json::from_str(&r1).unwrap();
        assert_eq!(v1["ok"], true);
        assert_eq!(v1["version"], 1);
        let list: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "list-revisions", None)).unwrap();
        assert_eq!(list["ok"], true);
        assert_eq!(
            list["head"], 1,
            "the held set is tagged with the winner (Q16)"
        );
        assert_eq!(list["revisions"].as_array().unwrap().len(), 1);
        assert_eq!(list["revisions"][0]["author"], "peer:test");
    }

    #[test]
    fn rollback_mints_a_higher_version_of_the_old_spec() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let _ = build_reply(
            &svc,
            "push-revision",
            Some(r#"{"spec": "packages:\n  - name: vim\n"}"#),
        );
        let _ = build_reply(&svc, "push-revision", Some(r#"{"spec": "packages: []\n"}"#));
        let rb: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "rollback", Some(r#"{"target": 1}"#))).unwrap();
        assert_eq!(rb["ok"], true);
        assert_eq!(rb["version"], 3, "rollback = a HIGHER version (Q6)");
        assert_eq!(rb["of"], 1);
        // The new head carries v1's spec again.
        let head = magic_fleet::store::elect_head(&svc.revisions_dir).unwrap();
        assert_eq!(head.spec.packages.len(), 1);
    }

    #[test]
    fn diff_reports_flat_changed_domains() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let _ = build_reply(&svc, "push-revision", Some(r#"{"spec": "packages: []\n"}"#));
        let _ = build_reply(
            &svc,
            "push-revision",
            Some(r#"{"spec": "packages:\n  - name: vim\nsettings:\n  theme.accent: '\"x\"'\n"}"#),
        );
        let d: serde_json::Value = serde_json::from_str(&build_reply(
            &svc,
            "diff-revisions",
            Some(r#"{"from": 1, "to": 2}"#),
        ))
        .unwrap();
        assert_eq!(d["ok"], true);
        let changed: Vec<&str> = d["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            changed,
            ["packages", "settings"],
            "flat top-level diff (Q7)"
        );
    }

    #[test]
    fn list_revisions_carries_ack_summaries() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let _ = build_reply(&svc, "push-revision", Some(r#"{"spec": "packages: []\n"}"#));
        magic_fleet::store::write_ack(
            tmp.path(),
            1,
            &magic_fleet::store::ApplyAck {
                peer: "oak".into(),
                status: "applied".into(),
                at: 5,
                detail: String::new(),
            },
        )
        .unwrap();
        let list: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "list-revisions", None)).unwrap();
        assert_eq!(list["revisions"][0]["acks"]["applied"], 1);
        assert_eq!(list["revisions"][0]["acks"]["failed"], 0);
    }

    #[test]
    fn nudge_writes_the_targets_nudge_file() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        let r: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "nudge", Some(r#"{"peer":"oak"}"#))).unwrap();
        assert_eq!(r["ok"], true);
        assert!(magic_fleet::store::take_nudge(tmp.path(), "oak"));
    }

    #[test]
    fn bad_requests_reply_honest_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = svc_in(tmp.path());
        for (verb, body) in [
            ("push-revision", None),
            ("diff-revisions", Some(r#"{"from": 1}"#)),
            ("rollback", Some(r#"{"target": 99}"#)),
        ] {
            let r: serde_json::Value =
                serde_json::from_str(&build_reply(&svc, verb, body)).unwrap();
            assert_eq!(r["ok"], false, "{verb} must report not panic");
        }
        assert!(build_reply(&svc, "bogus", None).contains("unknown fleet verb"));
    }
}
