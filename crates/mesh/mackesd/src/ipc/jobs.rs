//! PLANES-10 verbs (the PLANES-9 control surface) — `action/jobs/*`.
//!
//! `list-templates`, `launch` (resolve the selector against the live
//! directory, write the run), `runs` (history), and `run-results`
//! (per-target outcomes). Leaderless: any node serves; the target
//! executor (`job_exec`) does the work. State is the magic_fleet
//! jobs store on the replicated volume.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};

use magic_fleet::jobs::{
    read_run, read_target_results, read_templates, runs_dir, write_run, Candidate, JobRun,
    TargetSelector,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use super::action_auth::{ActionAuthorizer, MutationContext};

/// Action verbs served on `action/jobs/<verb>`.
pub const ACTION_VERBS: [&str; 4] = ["list-templates", "launch", "runs", "run-results"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Jobs service — rooted at the replicated workgroup root + the
/// optional roster DB (for selector resolution).
#[derive(Debug, Default, Clone)]
pub struct JobsService {
    pub workgroup_root: PathBuf,
    pub store_db: Option<PathBuf>,
}

impl JobsService {
    #[must_use]
    pub fn new(workgroup_root: &Path, store_db: Option<PathBuf>) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
            store_db,
        }
    }

    /// The live candidate set: every PeerRecord joined with its
    /// role (roster mirror) + capability tags (PLANES-3).
    fn candidates(&self) -> Vec<Candidate> {
        let roster: std::collections::HashMap<String, String> = self
            .store_db
            .as_ref()
            .and_then(|db| {
                rusqlite::Connection::open_with_flags(
                    db,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .ok()
            })
            .and_then(|conn| crate::nebula_roster::export_roster(&conn).ok())
            .map(|rows| {
                rows.into_iter()
                    // groups carries the role token (host/peer/…) —
                    // map to the §5 role names the selector uses.
                    .map(|r| (r.name, role_from_groups(&r.groups)))
                    .collect()
            })
            .unwrap_or_default();
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(
            &self.workgroup_root,
        ))
        .into_iter()
        .map(|rec| {
            let tags = mackes_mesh_types::cap_tags::read_tags(&self.workgroup_root, &rec.hostname)
                .tags
                .iter()
                .map(|t| t.as_str().to_string())
                .collect();
            Candidate {
                role: roster.get(&rec.hostname).cloned().unwrap_or_default(),
                hostname: rec.hostname,
                tags,
            }
        })
        .collect()
    }
}

fn role_from_groups(groups: &str) -> String {
    if groups.contains("host") {
        "lighthouse".into()
    } else {
        "workstation".into()
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Build the reply for one `action/jobs/<verb>` request.
#[must_use]
pub fn build_reply(svc: &JobsService, verb: &str, body: Option<&str>, ulid: &str) -> String {
    let req: serde_json::Value =
        serde_json::from_str(body.unwrap_or("{}")).unwrap_or(serde_json::Value::Null);
    match verb {
        "list-templates" => {
            let tpls: Vec<_> = read_templates(&svc.workgroup_root)
                .into_iter()
                .map(|t| {
                    json!({ "id": t.id, "description": t.description,
                            "playbook": t.playbook, "schedule": t.schedule })
                })
                .collect();
            json!({ "ok": true, "templates": tpls }).to_string()
        }
        "launch" => {
            // {playbook, targets:{tags,roles,peers}, vars?} → resolve +
            // write the run; the message ulid is the run id.
            let Some(playbook) = req.get("playbook").and_then(|v| v.as_str()) else {
                return json!({ "ok": false, "error": "launch: need `playbook`" }).to_string();
            };
            let selector: TargetSelector = req
                .get("targets")
                .and_then(|t| serde_json::from_value(t.clone()).ok())
                .unwrap_or_default();
            let targets = selector.resolve(&svc.candidates());
            if targets.is_empty() {
                return json!({ "ok": false, "error": "launch: selector matched no nodes" })
                    .to_string();
            }
            let vars = req
                .get("vars")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let run = JobRun {
                run_id: ulid.to_string(),
                playbook: playbook.to_string(),
                vars,
                targets: targets.clone(),
                launched_by: "local".into(),
                at: now_s(),
            };
            match write_run(&svc.workgroup_root, &run) {
                Ok(_) => json!({ "ok": true, "run_id": ulid, "targets": targets }).to_string(),
                Err(e) => json!({ "ok": false, "error": format!("launch: {e}") }).to_string(),
            }
        }
        "runs" => {
            let mut runs: Vec<_> = std::fs::read_dir(runs_dir(&svc.workgroup_root))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter_map(|id| read_run(&svc.workgroup_root, &id))
                .map(|r| {
                    let results = read_target_results(&svc.workgroup_root, &r.run_id);
                    let failed = results.iter().filter(|t| t.status == "failed").count();
                    json!({
                        "run_id": r.run_id, "playbook": r.playbook, "at": r.at,
                        "targets": r.targets.len(),
                        "done": results.len(), "failed": failed,
                    })
                })
                .collect();
            runs.sort_by(|a, b| b["at"].as_u64().cmp(&a["at"].as_u64()));
            json!({ "ok": true, "runs": runs }).to_string()
        }
        "run-results" => {
            let Some(run_id) = req.get("run_id").and_then(|v| v.as_str()) else {
                return json!({ "ok": false, "error": "run-results: need `run_id`" }).to_string();
            };
            let results: Vec<_> = read_target_results(&svc.workgroup_root, run_id)
                .into_iter()
                .map(|t| json!({ "host": t.hostname, "status": t.status, "detail": t.detail }))
                .collect();
            json!({ "ok": true, "run_id": run_id, "results": results }).to_string()
        }
        other => json!({ "ok": false, "error": format!("unknown jobs verb: {other}") }).to_string(),
    }
}

/// Apply the shared-Bus authorization boundary before a jobs mutation reaches
/// the replicated run store. Query verbs remain available without a token.
fn build_bus_reply(
    svc: &JobsService,
    verb: &str,
    body: Option<&str>,
    ulid: &str,
    authorizer: &ActionAuthorizer,
) -> String {
    if verb == "launch" {
        let raw = body.unwrap_or_default();
        let target = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|request| {
                request
                    .get("playbook")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let context = MutationContext {
            verb: "jobs-launch",
            node: "jobs",
            target: &target,
        };
        if let Err(error) = authorizer.authorize(raw, context) {
            tracing::warn!(
                target: "mackesd::jobs",
                %error,
                "refused unauthorized jobs launch"
            );
            return json!({ "ok": false, "error": format!("launch authorization refused: {error}") })
                .to_string();
        }
    }
    build_reply(svc, verb, body, ulid)
}

/// Run the jobs Bus responder until `should_stop()`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &JobsService, should_stop: F) {
    let mut cursors: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let authorizer = ActionAuthorizer::production();
    while !should_stop() {
        for verb in ACTION_VERBS {
            let topic = format!("action/jobs/{verb}");
            let since = cursors.get(&topic).map(String::as_str);
            let Ok(msgs) = persist.list_since(&topic, since) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                // EFF-23 — refuse an oversized body before build_reply parses it.
                let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                    build_bus_reply(svc, verb, msg.body.as_deref(), &msg.ulid, &authorizer)
                } else {
                    tracing::warn!(
                        topic = %topic,
                        len = msg.body.as_ref().map_or(0, String::len),
                        cap = crate::ipc::MAX_RPC_BODY_BYTES,
                        "jobs responder: body exceeds cap; refusing",
                    );
                    crate::ipc::body_too_large_reply(verb)
                };
                let _ = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply),
                );
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::authorize_test_body;
    use mackes_mesh_types::peers::{write_peer_record, PeerRecord};

    const AUTH_KEY: &[u8] = b"jobs-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn svc_with_peer(root: &Path, host: &str, tags: &[mackes_mesh_types::cap_tags::CapabilityTag]) {
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        write_peer_record(
            &pdir,
            &PeerRecord {
                hostname: host.into(),
                mde_version: None,
                last_seen_ms: 1,
                health: "healthy".into(),
                descriptors: None,
                overlay_ip: None,
                role: None,
                external_addr: None,
                media: false,
            },
        )
        .unwrap();
        let mut t = mackes_mesh_types::cap_tags::NodeTags::default();
        for tag in tags {
            t.tags.insert(*tag);
        }
        mackes_mesh_types::cap_tags::write_tags(root, host, &t).unwrap();
    }

    fn signed_launch_body(playbook: &str, nonce: &str, expires_at_ms: i64) -> String {
        let unsigned = json!({
            "schema_version": 1,
            "playbook": playbook,
            "targets": { "peers": ["oak"] }
        })
        .to_string();
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: "jobs-launch",
                node: "jobs",
                target: playbook,
            },
            nonce,
            expires_at_ms,
        )
    }

    #[test]
    fn hostile_bus_launches_never_write_a_run_and_replay_executes_once() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = JobsService::new(tmp.path(), None);
        svc_with_peer(
            tmp.path(),
            "oak",
            &[mackes_mesh_types::cap_tags::CapabilityTag::Execution],
        );
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW);

        let unsigned = json!({
            "playbook": "unsigned.yml",
            "targets": { "peers": ["oak"] }
        })
        .to_string();
        let future = signed_launch_body("future.yml", "future", AUTH_NOW + 30_000)
            .replace("\"schema_version\":1", "\"schema_version\":2");
        let overlong = signed_launch_body("overlong.yml", "overlong", AUTH_NOW + 30_001);
        let tampered = signed_launch_body("before.yml", "tampered", AUTH_NOW + 30_000)
            .replace("before.yml", "after.yml");
        for (index, body) in [&unsigned, &future, &overlong, &tampered]
            .into_iter()
            .enumerate()
        {
            let reply: serde_json::Value = serde_json::from_str(&build_bus_reply(
                &svc,
                "launch",
                Some(body),
                &format!("hostile-{index}"),
                &authorizer,
            ))
            .unwrap();
            assert_eq!(reply["ok"], false);
        }
        assert!(read_run(tmp.path(), "hostile-0").is_none());
        assert!(read_run(tmp.path(), "hostile-1").is_none());
        assert!(read_run(tmp.path(), "hostile-2").is_none());
        assert!(read_run(tmp.path(), "hostile-3").is_none());

        let replay = signed_launch_body("once.yml", "replay", AUTH_NOW + 30_000);
        let first: serde_json::Value = serde_json::from_str(&build_bus_reply(
            &svc,
            "launch",
            Some(&replay),
            "authorized-once",
            &authorizer,
        ))
        .unwrap();
        assert_eq!(first["ok"], true);
        let second: serde_json::Value = serde_json::from_str(&build_bus_reply(
            &svc,
            "launch",
            Some(&replay),
            "authorized-replay",
            &authorizer,
        ))
        .unwrap();
        assert_eq!(second["ok"], false);
        assert!(read_run(tmp.path(), "authorized-once").is_some());
        assert!(read_run(tmp.path(), "authorized-replay").is_none());

        let reads = ["list-templates", "runs", "run-results"];
        for verb in reads {
            let reply: serde_json::Value = serde_json::from_str(&build_bus_reply(
                &svc,
                verb,
                Some(r#"{"run_id":"missing"}"#),
                "read",
                &authorizer,
            ))
            .unwrap();
            assert_eq!(reply["ok"], true, "{verb} must remain an open read");
        }
    }

    #[test]
    fn launch_resolves_the_selector_and_writes_a_run() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = JobsService::new(tmp.path(), None);
        svc_with_peer(
            tmp.path(),
            "oak",
            &[mackes_mesh_types::cap_tags::CapabilityTag::Execution],
        );
        svc_with_peer(tmp.path(), "pine", &[]);
        let reply: serde_json::Value = serde_json::from_str(&build_reply(
            &svc,
            "launch",
            Some(r#"{"playbook":"playbooks/p.yml","targets":{"tags":["execution"]}}"#),
            "run-xyz",
        ))
        .unwrap();
        assert_eq!(reply["ok"], true);
        // Only the execution-tagged oak resolved.
        assert_eq!(reply["targets"], json!(["oak"]));
        assert!(read_run(tmp.path(), "run-xyz").is_some());
    }

    #[test]
    fn launch_with_no_match_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = JobsService::new(tmp.path(), None);
        svc_with_peer(tmp.path(), "pine", &[]);
        let reply: serde_json::Value = serde_json::from_str(&build_reply(
            &svc,
            "launch",
            Some(r#"{"playbook":"x","targets":{"tags":["execution"]}}"#),
            "r",
        ))
        .unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"]
            .as_str()
            .unwrap()
            .contains("matched no nodes"));
    }

    #[test]
    fn runs_and_results_surface_the_history() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = JobsService::new(tmp.path(), None);
        svc_with_peer(
            tmp.path(),
            "oak",
            &[mackes_mesh_types::cap_tags::CapabilityTag::Execution],
        );
        let _ = build_reply(
            &svc,
            "launch",
            Some(r#"{"playbook":"p","targets":{"peers":["oak"]}}"#),
            "run-1",
        );
        magic_fleet::jobs::write_target_result(
            tmp.path(),
            "run-1",
            &magic_fleet::jobs::TargetResult {
                hostname: "oak".into(),
                status: "ok".into(),
                detail: String::new(),
            },
        )
        .unwrap();
        let runs: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "runs", None, "x")).unwrap();
        assert_eq!(runs["runs"][0]["run_id"], "run-1");
        assert_eq!(runs["runs"][0]["done"], 1);
        let res: serde_json::Value = serde_json::from_str(&build_reply(
            &svc,
            "run-results",
            Some(r#"{"run_id":"run-1"}"#),
            "x",
        ))
        .unwrap();
        assert_eq!(res["results"][0]["host"], "oak");
        assert_eq!(res["results"][0]["status"], "ok");
    }
}
