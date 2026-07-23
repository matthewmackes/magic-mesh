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

use mackes_mesh_types::ddns;
use mackes_mesh_types::exposure::{self, ExposurePolicy, ExposureTemplate, Tier};

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

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
                Ok(_) => {
                    // CONNECT-9 — auto-create/update the service's DDNS public name
                    // so the operator no longer has to pre-create it (CONNECT-7's
                    // hostname is now self-naming). Reuses the DDNS-EGRESS-3 config
                    // + reconcile/writer path: we only write the durable record
                    // (the `ddns` worker publishes it). Gated to the binding's own
                    // lighthouse — the `wan` record resolves to the LOCAL node's WAN,
                    // so only the bound ingress node owns it; a cross-node expose is
                    // reconciled by that lighthouse's `connect_firewall` worker.
                    let ddns_synced =
                        sync_ingress_ddns_record(root, &svc.hostname, lighthouse, hostname);
                    json!({ "ok": true, "hostname": hostname, "ddns": ddns_synced }).to_string()
                }
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
            // CONNECT-9 — the binding being torn down (so we can reclaim its DDNS
            // public name below). Captured before we clear the ingress.
            let old_ingress = svc_policy.ingress.clone();
            let updated = ExposurePolicy {
                tier: Tier::MeshOnly,
                ingress: None,
                ..svc_policy
            };
            cfg.upsert(updated);
            match exposure::save(root, &cfg) {
                Ok(_) => {
                    // CONNECT-9 — remove the auto-created DDNS public name on
                    // unexpose (the reverse of the expose auto-create). Same
                    // local-lighthouse gating: only the node that owned the `wan`
                    // record reclaims it here; a cross-node unexpose is reconciled
                    // away by the bound lighthouse's `connect_firewall` worker.
                    let ddns_removed = old_ingress
                        .as_ref()
                        .filter(|b| b.lighthouse == svc.hostname)
                        .is_some_and(|b| remove_ingress_ddns_record(root, &b.hostname));
                    json!({ "ok": true, "ddns_removed": ddns_removed }).to_string()
                }
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

/// Parse a privileged Connect mutation into its stable capability target and
/// the legacy body consumed by [`build_reply`]. Authentication metadata is
/// removed before any config is loaded or written. Read-only verbs remain
/// unauthenticated.
fn mutation_request(
    verb: &str,
    req_body: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    if !matches!(
        verb,
        "set-policy" | "expose" | "unexpose" | "set-template" | "apply-template"
    ) {
        return Ok(None);
    }
    let body = req_body.ok_or_else(|| format!("{verb}: missing request body"))?;
    let mut request: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| format!("{verb}: request body must be a JSON object"))?;
    let object = request
        .as_object_mut()
        .ok_or_else(|| format!("{verb}: request body must be a JSON object"))?;
    object.remove("schema_version");
    object.remove("armed_token");
    let string_field = |field: &str| {
        request
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{verb}: missing `{field}`"))
    };
    let (target, handler_body) = match verb {
        "set-policy" => {
            let id = string_field("id")?;
            (format!("service:{id}"), request.to_string())
        }
        "expose" => {
            let id = string_field("id")?;
            (format!("service:{id}"), request.to_string())
        }
        "unexpose" => {
            let id = string_field("id")?;
            (format!("service:{id}"), id)
        }
        "set-template" => {
            let name = string_field("name")?;
            (format!("template:{name}"), request.to_string())
        }
        "apply-template" => {
            let name = string_field("template")?;
            let ids = request
                .get("ids")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "apply-template: missing `ids`".to_string())?;
            let mut target_ids = Vec::with_capacity(ids.len());
            for id in ids {
                let id = id
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "apply-template: ids must be non-empty strings".to_string())?;
                target_ids.push(id.to_owned());
            }
            if target_ids.is_empty() {
                return Err("apply-template: `ids` must not be empty".to_string());
            }
            (
                format!("template:{name}:{}", target_ids.join(",")),
                request.to_string(),
            )
        }
        _ => unreachable!("the privileged Connect verb set is closed"),
    };
    Ok(Some((target, handler_body)))
}

/// Apply the shared-spool authorization boundary before a Connect mutation
/// can read or write exposure/DDNS state. The request body is verified exactly
/// as published, then auth metadata is stripped before the legacy handler.
fn build_authorized_reply(
    svc: &ConnectService,
    verb: &str,
    req_body: Option<&str>,
    authorizer: &ActionAuthorizer,
) -> String {
    let prepared = match mutation_request(verb, req_body) {
        Ok(prepared) => prepared,
        Err(error) => return json!({ "error": error }).to_string(),
    };
    let Some((target, handler_body)) = prepared else {
        return build_reply(svc, verb, req_body);
    };
    let auth_verb = format!("connect-{verb}");
    let context = MutationContext {
        verb: &auth_verb,
        node: &svc.hostname,
        target: &target,
    };
    if let Err(error) = authorizer.authorize(req_body.expect("a mutation requires a body"), context)
    {
        tracing::warn!(target: "mackesd::action_auth", verb, %error, "refused unauthorized Connect mutation");
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, Some(&handler_body))
}

/// CONNECT-9 — auto-create/update the DDNS public name for an ingress `hostname`
/// bound to `lighthouse`, but only when `this_host` IS that lighthouse (the
/// `wan`-sourced record resolves to the LOCAL node's WAN, so only the bound
/// ingress node may own it). Writes the durable [`ddns::RecordDef`] into the shared
/// `[ddns]` config and lets the DDNS-EGRESS-3 reconcile worker publish it — no
/// second DNS path. Returns `true` when this node wrote/updated the record. A save
/// failure is logged and treated as not-synced (the worker re-derives on its tick).
fn sync_ingress_ddns_record(
    root: &std::path::Path,
    this_host: &str,
    lighthouse: &str,
    hostname: &str,
) -> bool {
    if this_host != lighthouse {
        // Not our ingress: the bound lighthouse's connect_firewall worker owns it.
        return false;
    }
    let mut cfg = ddns::load(root);
    let label = ddns::ingress_record_label(hostname, &cfg.zone);
    let Some(rec) = ddns::ingress_record(&label) else {
        return false; // hostname collapsed to the bare zone — nothing to publish.
    };
    if !cfg.upsert_record(rec) {
        return true; // already present + identical (no-churn); record is in place.
    }
    match ddns::save(root, &cfg) {
        Ok(_) => {
            tracing::info!(hostname, label = %label, "connect expose: auto-created DDNS public name (CONNECT-9)");
            true
        }
        Err(e) => {
            tracing::warn!(hostname, error = %e, "connect expose: DDNS record save failed");
            false
        }
    }
}

/// CONNECT-9 — reclaim the DDNS public name for an unexposed ingress `hostname`
/// (the reverse of [`sync_ingress_ddns_record`]). Removes the record from the
/// shared `[ddns]` config so the reconcile worker deletes it (per its `on_down`
/// policy). Returns `true` when a record was removed. Best-effort save.
fn remove_ingress_ddns_record(root: &std::path::Path, hostname: &str) -> bool {
    let mut cfg = ddns::load(root);
    let label = ddns::ingress_record_label(hostname, &cfg.zone);
    if label.is_empty() || !cfg.remove_record(&label) {
        return false;
    }
    match ddns::save(root, &cfg) {
        Ok(_) => {
            tracing::info!(hostname, label = %label, "connect unexpose: removed DDNS public name (CONNECT-9)");
            true
        }
        Err(e) => {
            tracing::warn!(hostname, error = %e, "connect unexpose: DDNS record removal save failed");
            false
        }
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
    let authorizer = ActionAuthorizer::production();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors, &authorizer);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(
    persist: &Persist,
    svc: &ConnectService,
    cursors: &mut HashMap<String, String>,
    authorizer: &ActionAuthorizer,
) {
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
                build_authorized_reply(svc, verb, msg.body.as_deref(), authorizer)
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
    use crate::ipc::action_auth::authorize_test_body;

    const AUTH_KEY: &[u8] = b"connect-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn svc() -> (tempfile::TempDir, ConnectService) {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ConnectService::new(tmp.path().to_path_buf(), "testhost".into());
        (tmp, svc)
    }

    fn authorized_reply(svc: &ConnectService, verb: &str, unsigned: &str, nonce: &str) -> String {
        let auth = ActionAuthorizer::for_test(AUTH_KEY, svc.workgroup_root.join("auth"), AUTH_NOW);
        let target = match verb {
            "set-policy" | "expose" => format!(
                "service:{}",
                serde_json::from_str::<serde_json::Value>(unsigned).unwrap()["id"]
                    .as_str()
                    .unwrap()
            ),
            "unexpose" => format!(
                "service:{}",
                serde_json::from_str::<serde_json::Value>(unsigned).unwrap()["id"]
                    .as_str()
                    .unwrap()
            ),
            "set-template" => format!(
                "template:{}",
                serde_json::from_str::<serde_json::Value>(unsigned).unwrap()["name"]
                    .as_str()
                    .unwrap()
            ),
            "apply-template" => {
                let v: serde_json::Value = serde_json::from_str(unsigned).unwrap();
                let ids = v["ids"].as_array().unwrap();
                format!(
                    "template:{}:{}",
                    v["template"].as_str().unwrap(),
                    ids.iter()
                        .map(|id| id.as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            }
            _ => panic!("not a mutation"),
        };
        let auth_verb = format!("connect-{verb}");
        let body = authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb: &auth_verb,
                node: &svc.hostname,
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        );
        build_authorized_reply(svc, verb, Some(&body), &auth)
    }

    #[test]
    fn connect_mutations_fail_closed_before_state_io() {
        let (_t, s) = svc();
        let policy = json!({
            "id": "grafana",
            "source": { "node": "eagle", "kind": "container", "port": 3000, "proto": "tcp" },
            "tier": "mesh-only"
        });
        let refused = build_authorized_reply(
            &s,
            "set-policy",
            Some(&policy.to_string()),
            &ActionAuthorizer::for_test(AUTH_KEY, s.workgroup_root.join("auth"), AUTH_NOW),
        );
        assert!(refused.contains("authorization refused"), "{refused}");
        assert!(exposure::load(&s.workgroup_root).service.is_empty());
    }

    #[test]
    fn authorized_connect_mutation_is_exact_body_bound_and_single_use() {
        let (_t, s) = svc();
        let unsigned = json!({
            "schema_version": 1,
            "id": "grafana",
            "source": { "node": "eagle", "kind": "container", "port": 3000, "proto": "tcp" },
            "tier": "mesh-only"
        })
        .to_string();
        let first = authorized_reply(&s, "set-policy", &unsigned, "connect-once");
        assert!(first.contains("\"ok\":true"), "{first}");
        let auth = ActionAuthorizer::for_test(AUTH_KEY, s.workgroup_root.join("auth2"), AUTH_NOW);
        let target = "service:grafana";
        let auth_verb = "connect-set-policy";
        let body = authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: auth_verb,
                node: &s.hostname,
                target,
            },
            "connect-tamper",
            AUTH_NOW + 30_000,
        );
        let tampered = body.replace("grafana", "other");
        let refused = build_authorized_reply(&s, "set-policy", Some(&tampered), &auth);
        assert!(refused.contains("authorization refused"), "{refused}");
        let accepted = build_authorized_reply(&s, "set-policy", Some(&body), &auth);
        assert!(accepted.contains("\"ok\":true"), "{accepted}");
        let replay = build_authorized_reply(&s, "set-policy", Some(&body), &auth);
        assert!(replay.contains("authorization refused"), "{replay}");
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

    #[test]
    fn expose_on_bound_lighthouse_auto_creates_ddns_name_unexpose_removes() {
        // CONNECT-9 — exposing a service on the lighthouse it binds to auto-creates
        // the DDNS public name (CONNECT-7 no longer needs an operator-typed record);
        // unexpose reclaims it. The service's hostname is FQDN under the default
        // DDNS zone, so the record's bare label is the leading segment.
        let tmp = tempfile::tempdir().unwrap();
        // The responder's host IS the ingress lighthouse so it owns the wan record.
        let s = ConnectService::new(tmp.path().to_path_buf(), "lighthouse-01".into());
        let _ = build_reply(
            &s,
            "set-policy",
            Some(
                &json!({"id":"grafana","source":{"node":"eagle","kind":"container","port":3000},"tier":"mesh-only"})
                    .to_string(),
            ),
        );
        let ok = build_reply(
            &s,
            "expose",
            Some(
                &json!({
                    "id":"grafana",
                    "lighthouse":"lighthouse-01",
                    "hostname":"grafana.services.matthewmackes.com",
                    "mode":"http"
                })
                .to_string(),
            ),
        );
        let v: serde_json::Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{ok}");
        assert_eq!(v["ddns"], serde_json::Value::Bool(true), "{ok}");
        // The DDNS config now carries the auto-created wan-sourced record.
        let dcfg = mackes_mesh_types::ddns::load(tmp.path());
        let rec = dcfg
            .record
            .iter()
            .find(|r| r.name == "grafana")
            .expect("CONNECT-9 auto-created the DDNS record");
        assert_eq!(rec.source, "wan");
        // Unexpose reclaims the DDNS name.
        let u = build_reply(&s, "unexpose", Some("grafana"));
        let uv: serde_json::Value = serde_json::from_str(&u).unwrap();
        assert_eq!(uv["ok"], serde_json::Value::Bool(true), "{u}");
        assert_eq!(uv["ddns_removed"], serde_json::Value::Bool(true), "{u}");
        assert!(
            mackes_mesh_types::ddns::load(tmp.path())
                .record
                .iter()
                .all(|r| r.name != "grafana"),
            "unexpose removed the DDNS record"
        );
    }

    #[test]
    fn expose_from_non_bound_node_defers_ddns_to_the_lighthouse() {
        // CONNECT-9 — when the responder's host is NOT the ingress lighthouse, the
        // wan record (which would resolve to THIS node's WAN) is not written here;
        // the bound lighthouse's connect_firewall worker reconciles it instead.
        let tmp = tempfile::tempdir().unwrap();
        let s = ConnectService::new(tmp.path().to_path_buf(), "some-other-node".into());
        let _ = build_reply(
            &s,
            "set-policy",
            Some(
                &json!({"id":"grafana","source":{"node":"eagle","kind":"container","port":3000},"tier":"mesh-only"})
                    .to_string(),
            ),
        );
        let ok = build_reply(
            &s,
            "expose",
            Some(
                &json!({
                    "id":"grafana",
                    "lighthouse":"lighthouse-01",
                    "hostname":"grafana.services.matthewmackes.com"
                })
                .to_string(),
            ),
        );
        let v: serde_json::Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["ddns"], serde_json::Value::Bool(false), "{ok}");
        // No record was written on this node.
        assert!(mackes_mesh_types::ddns::load(tmp.path()).record.is_empty());
    }
}
