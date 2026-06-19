//! CONNECT-1 (part 2) — the Bus responder for the unified connectivity /
//! exposure model, on `action/connect/<verb>` → `reply/<ulid>` (design:
//! `docs/design/connect.md`). Thin typed surface over
//! [`mackes_mesh_types::exposure`]'s TOML state on the shared substrate; the
//! Workbench Connectivity panels (CONNECT-6/7/8) render through these verbs.
//!
//! Same dedicated-OS-thread shape as the Settings/Fleet responders (the
//! exposure free fns are synchronous; `Persist`/rusqlite isn't `Send`).

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use mackes_mesh_types::exposure::{self, ExposurePolicy, ExposureTemplate, Tier};

/// The connectivity responder service — holds the shared-substrate root where
/// the exposure config (`<root>/connect/policy.toml`) lives + this node's
/// hostname (CONNECT-2 candidate discovery tags candidates with their host).
#[derive(Debug, Clone)]
pub struct ConnectService {
    workgroup_root: PathBuf,
    hostname: String,
}

impl ConnectService {
    /// Build the service rooted at the shared workgroup root, for `hostname`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
        }
    }
}

/// Action verbs served on `action/connect/<verb>`.
pub const ACTION_VERBS: [&str; 8] = [
    "list-services",
    "list-candidates",
    "set-policy",
    "expose",
    "unexpose",
    "list-templates",
    "set-template",
    "apply-template",
];

/// CONNECT-2 — parse the local-address column of `ss -H -tln` (one socket per
/// line) into the set of listening TCP ports. The local address is the 4th
/// whitespace field; the port is the text after its last `:` (handles IPv4,
/// `addr%iface`, and `[::]`-style IPv6). Pure + testable.
#[must_use]
pub fn parse_listening_ports(ss_out: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = ss_out
        .lines()
        .filter_map(|line| {
            let local = line.split_whitespace().nth(3)?;
            let port = local.rsplit(':').next()?;
            port.parse::<u16>().ok()
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// CONNECT-2 — a friendly label for a well-known service port (the PD-2 canonical
/// mesh services + common app ports), so a discovered candidate reads as e.g.
/// "SSH" / "PostgreSQL" rather than a bare number. `None` ⇒ the UI shows the port.
#[must_use]
pub fn well_known_label(port: u16) -> Option<&'static str> {
    Some(match port {
        22 => "SSH",
        53 => "DNS",
        80 => "HTTP",
        443 => "HTTPS",
        // PD-2 canonical mesh services.
        4222 => "NATS",
        4243 => "Mesh enrollment",
        9418 => "Mesh FS",
        // Common app/server ports operators publish.
        3000 => "Grafana / Node",
        3306 => "MySQL",
        4040 => "Airsonic",
        5432 => "PostgreSQL",
        6379 => "Redis",
        8000 | 8080 | 8888 => "HTTP (alt)",
        8443 => "HTTPS (alt)",
        9090 => "Prometheus",
        19999 => "Netdata",
        _ => return None,
    })
}

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for verb `verb`: `action/connect/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/connect/{verb}")
}

/// Build the reply body for one `action/connect/<verb>` request.
///   * `list-services`  — no body; reply `{ "services": [ExposurePolicy] }`.
///   * `set-policy`     — body is an `ExposurePolicy` JSON; upsert + save.
///   * `expose`         — body `{ "id", "lighthouse", "hostname", "mode"? }`;
///     flips the service to public-via-ingress with that binding.
///   * `unexpose`       — body is the service id (plain); back to mesh-only.
///   * `list-templates` — no body; reply `{ "templates": [ExposureTemplate] }`.
///   * `set-template`   — body is an `ExposureTemplate` JSON; upsert + save.
///
/// Mutations re-load → mutate → validate-on-save (atomic write-through). Any
/// failure is an `{"error": "..."}` envelope so the caller surfaces a diagnostic.
#[must_use]
pub fn build_reply(svc: &ConnectService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    match verb {
        "list-services" => {
            let cfg = exposure::load(root);
            json!({ "ok": true, "services": cfg.service }).to_string()
        }
        "list-templates" => {
            let cfg = exposure::load(root);
            json!({ "ok": true, "templates": cfg.template }).to_string()
        }
        "list-candidates" => {
            // CONNECT-2 — auto-discover exposable services from this node's
            // listening TCP ports; tag each with whether it's already in the
            // exposure config (the UI opts a candidate in to expose it). The
            // compute-registry (VM/container) + descriptor sources layer on later.
            let cfg = exposure::load(root);
            let ports = local_listening_ports();
            let candidates: Vec<serde_json::Value> = ports
                .iter()
                .map(|&port| {
                    let id = format!("{}-{port}", svc.hostname);
                    let configured = cfg
                        .service
                        .iter()
                        .any(|s| s.source.node == svc.hostname && s.source.port == port);
                    json!({
                        "id": id,
                        "node": svc.hostname,
                        "kind": "host",
                        "port": port,
                        "proto": "tcp",
                        "label": well_known_label(port),
                        "configured": configured,
                    })
                })
                .collect();
            json!({ "ok": true, "candidates": candidates }).to_string()
        }
        "set-policy" => {
            let Some(body) = req_body else {
                return err("set-policy: missing ExposurePolicy body".into());
            };
            let policy: ExposurePolicy = match serde_json::from_str(body) {
                Ok(p) => p,
                Err(e) => return err(format!("set-policy: bad json: {e}")),
            };
            if let Err(e) = policy.validate() {
                return err(format!("set-policy: {e}"));
            }
            let mut cfg = exposure::load(root);
            cfg.upsert(policy);
            match exposure::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("set-policy: save: {e}")),
            }
        }
        "expose" => {
            let Some(body) = req_body else {
                return err("expose: missing request body".into());
            };
            let req: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => return err(format!("expose: bad json: {e}")),
            };
            let id = req
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let lighthouse = req
                .get("lighthouse")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let hostname = req
                .get("hostname")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if id.is_empty() || lighthouse.is_empty() || hostname.is_empty() {
                return err("expose: id, lighthouse and hostname are all required".into());
            }
            let mode = req
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .and_then(|m| serde_json::from_value(json!(m)).ok())
                .unwrap_or_default();
            let mut cfg = exposure::load(root);
            let Some(svc_policy) = cfg.get(id).cloned() else {
                return err(format!(
                    "expose: no such service '{id}' (set-policy it first)"
                ));
            };
            let updated = ExposurePolicy {
                tier: Tier::PublicViaIngress,
                ingress: Some(exposure::IngressBinding {
                    lighthouse: lighthouse.to_string(),
                    hostname: hostname.to_string(),
                }),
                mode,
                ..svc_policy
            };
            cfg.upsert(updated);
            match exposure::save(root, &cfg) {
                Ok(_) => json!({ "ok": true, "hostname": hostname }).to_string(),
                Err(e) => err(format!("expose: save: {e}")),
            }
        }
        "unexpose" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("unexpose: missing service id".into());
            };
            let mut cfg = exposure::load(root);
            let Some(svc_policy) = cfg.get(id).cloned() else {
                return err(format!("unexpose: no such service '{id}'"));
            };
            let updated = ExposurePolicy {
                tier: Tier::MeshOnly,
                ingress: None,
                ..svc_policy
            };
            cfg.upsert(updated);
            match exposure::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("unexpose: save: {e}")),
            }
        }
        "set-template" => {
            let Some(body) = req_body else {
                return err("set-template: missing ExposureTemplate body".into());
            };
            let tpl: ExposureTemplate = match serde_json::from_str(body) {
                Ok(t) => t,
                Err(e) => return err(format!("set-template: bad json: {e}")),
            };
            if tpl.name.trim().is_empty() {
                return err("set-template: template name is empty".into());
            }
            let mut cfg = exposure::load(root);
            if let Some(existing) = cfg.template.iter_mut().find(|t| t.name == tpl.name) {
                *existing = tpl;
            } else {
                cfg.template.push(tpl);
            }
            match exposure::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("set-template: save: {e}")),
            }
        }
        "apply-template" => {
            // CONNECT-8 — apply a named template's tier+mode(+ingress lighthouse)
            // to a set of services at once. Body: `{ "template": "<name>",
            // "ids": ["a","b"] }`. A public template needs each target to already
            // carry an ingress hostname (expose it first) — surfaced per-service.
            let Some(body) = req_body else {
                return err("apply-template: missing request body".into());
            };
            let req: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => return err(format!("apply-template: bad json: {e}")),
            };
            let tname = req
                .get("template")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let ids: Vec<String> = req
                .get("ids")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if tname.is_empty() || ids.is_empty() {
                return err("apply-template: 'template' and a non-empty 'ids' are required".into());
            }
            let mut cfg = exposure::load(root);
            let Some(tpl) = cfg.template.iter().find(|t| t.name == tname).cloned() else {
                return err(format!("apply-template: no such template '{tname}'"));
            };
            let mut applied = 0usize;
            let mut skipped: Vec<String> = Vec::new();
            for id in &ids {
                let Some(mut svc_policy) = cfg.get(id).cloned() else {
                    skipped.push(format!("{id} (no such service)"));
                    continue;
                };
                svc_policy.tier = tpl.tier;
                svc_policy.mode = tpl.mode;
                svc_policy.template = Some(tpl.name.clone());
                if tpl.tier == Tier::PublicViaIngress {
                    // Keep the service's existing hostname; set the template's
                    // lighthouse. A target with no hostname can't go public yet.
                    match (&svc_policy.ingress, &tpl.lighthouse) {
                        (Some(b), Some(lh)) => {
                            svc_policy.ingress = Some(exposure::IngressBinding {
                                lighthouse: lh.clone(),
                                hostname: b.hostname.clone(),
                            });
                        }
                        (Some(_), None) => { /* keep existing binding */ }
                        (None, _) => {
                            skipped.push(format!(
                                "{id} (public template needs a hostname — expose it first)"
                            ));
                            continue;
                        }
                    }
                }
                cfg.upsert(svc_policy);
                applied += 1;
            }
            match exposure::save(root, &cfg) {
                Ok(_) => json!({ "ok": true, "applied": applied, "skipped": skipped }).to_string(),
                Err(e) => err(format!("apply-template: save: {e}")),
            }
        }
        other => err(format!("unknown connect verb: {other}")),
    }
}

/// CONNECT-2 — this node's listening TCP ports via `ss -H -tln` (best-effort:
/// an empty list if `ss` is absent/fails). Parsed by [`parse_listening_ports`].
fn local_listening_ports() -> Vec<u16> {
    std::process::Command::new("ss")
        .args(["-H", "-tln"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|out| parse_listening_ports(&out))
        .unwrap_or_default()
}

/// Run the connect Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &ConnectService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &ConnectService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "connect responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "connect responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> (tempfile::TempDir, ConnectService) {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ConnectService::new(tmp.path().to_path_buf(), "testhost".into());
        (tmp, svc)
    }

    #[test]
    fn parse_listening_ports_handles_ipv4_v6_iface() {
        // CONNECT-2 — real `ss -H -tln` shapes incl. %iface + [::] IPv6.
        let out = "LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:*\n\
                   LISTEN 0 128 10.42.0.3:8443 0.0.0.0:*\n\
                   LISTEN 0 4096 [::]:443 [::]:*\n\
                   LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n";
        let ports = parse_listening_ports(out);
        assert_eq!(ports, vec![22, 53, 443, 8443]); // sorted + deduped
        assert!(parse_listening_ports("").is_empty());
    }

    #[test]
    fn well_known_labels_cover_canonical_and_common() {
        assert_eq!(well_known_label(22), Some("SSH"));
        assert_eq!(well_known_label(4222), Some("NATS"));
        assert_eq!(well_known_label(5432), Some("PostgreSQL"));
        assert_eq!(well_known_label(19999), Some("Netdata"));
        assert_eq!(well_known_label(12345), None); // unknown → UI shows the port
    }

    #[test]
    fn list_candidates_marks_configured() {
        let (_t, s) = svc();
        // list-candidates returns an ok envelope (ports depend on the host, so
        // just assert the shape + that it doesn't error).
        let r = build_reply(&s, "list-candidates", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert!(v["candidates"].is_array());
    }

    #[test]
    fn verbs_and_topic_lock() {
        assert_eq!(action_topic("expose"), "action/connect/expose");
        assert!(ACTION_VERBS.contains(&"list-services"));
    }

    #[test]
    fn set_policy_then_list_round_trip() {
        let (_t, s) = svc();
        let policy = json!({
            "id": "grafana",
            "source": { "node": "eagle", "kind": "container", "port": 3000, "proto": "tcp" },
            "tier": "mesh-only"
        });
        let r = build_reply(&s, "set-policy", Some(&policy.to_string()));
        assert!(r.contains("\"ok\":true"), "{r}");
        let list = build_reply(&s, "list-services", None);
        assert!(list.contains("grafana"), "{list}");
    }

    #[test]
    fn expose_requires_existing_service_then_flips_public() {
        let (_t, s) = svc();
        // expose before set-policy → error.
        let e = build_reply(
            &s,
            "expose",
            Some(&json!({"id":"x","lighthouse":"LH","hostname":"x.example"}).to_string()),
        );
        assert!(e.contains("error"), "{e}");
        // Create then expose.
        let _ = build_reply(
            &s,
            "set-policy",
            Some(
                &json!({"id":"x","source":{"node":"n","kind":"host","port":80},"tier":"mesh-only"})
                    .to_string(),
            ),
        );
        let ok = build_reply(
            &s,
            "expose",
            Some(
                &json!({"id":"x","lighthouse":"LH-01","hostname":"x.services.example"}).to_string(),
            ),
        );
        assert!(ok.contains("\"ok\":true"), "{ok}");
        let cfg = exposure::load(s.workgroup_root.as_path());
        let p = cfg.get("x").unwrap();
        assert!(p.is_public());
        assert_eq!(p.ingress.as_ref().unwrap().hostname, "x.services.example");
        // unexpose → mesh-only.
        let u = build_reply(&s, "unexpose", Some("x"));
        assert!(u.contains("\"ok\":true"), "{u}");
        assert!(!exposure::load(s.workgroup_root.as_path())
            .get("x")
            .unwrap()
            .is_public());
    }

    #[test]
    fn apply_template_sets_tier_mode_across_services() {
        let (_t, s) = svc();
        // Two mesh-only services + a mesh-only template, applied to both.
        for id in ["a", "b"] {
            let _ = build_reply(
                &s,
                "set-policy",
                Some(&json!({"id":id,"source":{"node":"n","kind":"host","port":80},"tier":"mesh-only"}).to_string()),
            );
        }
        let _ = build_reply(
            &s,
            "set-template",
            Some(&json!({"name":"internal","tier":"mesh-only","mode":"http"}).to_string()),
        );
        let r = build_reply(
            &s,
            "apply-template",
            Some(&json!({"template":"internal","ids":["a","b","missing"]}).to_string()),
        );
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["applied"], serde_json::json!(2));
        assert_eq!(v["skipped"].as_array().unwrap().len(), 1); // "missing"
        let cfg = exposure::load(s.workgroup_root.as_path());
        assert_eq!(cfg.get("a").unwrap().template.as_deref(), Some("internal"));
    }

    #[test]
    fn unknown_verb_errors() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "bogus", None).contains("unknown connect verb"));
    }
}
