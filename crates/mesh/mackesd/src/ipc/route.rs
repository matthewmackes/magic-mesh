//! ROUTE-TRACE-1 (responder) — `action/route/trace` → a typed PathGraph.
//!
//! The thin daemon gatherer over [`mackes_mesh_types::route_trace`]: it resolves
//! the request's endpoints from real mesh state (the CONNECT exposure policy for
//! a published service, the peer directory for overlay IPs) and calls the pure
//! assemblers. Same dedicated-OS-thread shape as the Connect/Settings responders
//! (the reads are synchronous file/`Persist` access).
//!
//! Request body `{ "from"?, "to", "direction" }`:
//!   * `direction:"ingress"` (default for a service trace) — `to` is a service id;
//!     builds Internet → Ingress → hosting node → Service from `to`'s exposure.
//!   * `direction:"egress"` — `from` is a mesh node, `to` an external dest; builds
//!     the node's WAN egress path.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use mackes_mesh_types::{exposure, peers, route_trace};

/// The route-trace responder — rooted at the shared workgroup root (where the
/// exposure policy + peer directory live).
#[derive(Debug, Clone)]
pub struct RouteService {
    workgroup_root: PathBuf,
}

impl RouteService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/route/<verb>`.
pub const ACTION_VERBS: [&str; 1] = ["trace"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/route/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/route/{verb}")
}

/// This node's hostname (best-effort) — used to decide whether a traced host's
/// firewall is locally readable. Mirrors the daemon's `local_hostname`.
fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Read the LOCAL node's inbound public firewall as ROUTE-TRACE rules from live
/// `firewall-cmd`, or `None` when it can't be read (no firewalld / not permitted).
/// `firewall-cmd --list-ports` → `4242/udp 443/tcp`; `--list-services` maps the
/// foundational service names (ssh→22/tcp). Anything not listed falls to the
/// public-zone default deny.
fn read_local_firewalld() -> Option<Vec<route_trace::FirewallRule>> {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("firewall-cmd")
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let ports = run(&["--list-ports"])?;
    let services = run(&["--list-services"]).unwrap_or_default();
    let mut rules = Vec::new();
    for spec in ports.split_whitespace() {
        if let Some((p, proto)) = spec.split_once('/') {
            if let Ok(port) = p.parse::<u16>() {
                rules.push(route_trace::FirewallRule {
                    port: Some(port),
                    proto: Some(proto.to_string()),
                    action: route_trace::Verdict::Allow,
                    cite: format!("--add-port {spec}"),
                });
            }
        }
    }
    if services.split_whitespace().any(|s| s == "ssh") {
        rules.push(route_trace::FirewallRule {
            port: Some(22),
            proto: Some("tcp".into()),
            action: route_trace::Verdict::Allow,
            cite: "--add-service ssh (22/tcp)".into(),
        });
    }
    Some(rules)
}

/// The inbound firewall ROUTE-TRACE should evaluate for `node_label`: the live
/// local firewalld when `node_label` is THIS node, else `Indeterminate` — a remote
/// host's rules can't be read, and ROUTE-TRACE never guesses a verdict.
fn host_inbound_firewall(node_label: &str) -> route_trace::FirewallProfile {
    let me = local_hostname();
    if !me.is_empty() && node_label.eq_ignore_ascii_case(&me) {
        if let Some(rules) = read_local_firewalld() {
            return route_trace::FirewallProfile::Rules {
                name: "firewalld:public".into(),
                rules,
                default: route_trace::Verdict::Block,
            };
        }
    }
    route_trace::FirewallProfile::Indeterminate {
        name: format!("firewalld:public@{node_label} (remote/unreadable)"),
    }
}

/// This node's overlay IP for `hostname` from the replicated peer directory
/// (etcd-or-fs is abstracted by the directory RPC elsewhere; the trace gatherer
/// reads the roster directly for the bare overlay-IP lookup). `None` when the
/// peer isn't enrolled / has no overlay IP yet — rendered as unknown, not guessed.
fn overlay_ip_of(root: &std::path::Path, hostname: &str) -> Option<String> {
    peers::read_peers(&peers::peers_dir(root))
        .into_iter()
        .find(|p| p.hostname == hostname)
        .and_then(|p| p.overlay_ip)
}

/// Build the reply for one `action/route/<verb>` request.
#[must_use]
pub fn build_reply(svc: &RouteService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    if verb != "trace" {
        return err(format!("unknown route verb: {verb}"));
    }
    let Some(body) = req_body else {
        return err("trace: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("trace: bad json: {e}")),
    };
    let direction = req
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ingress");
    let from = req
        .get("from")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let to = req
        .get("to")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let root = svc.workgroup_root.as_path();

    let graph = match direction {
        "ingress" => {
            if to.is_empty() {
                return err("trace: ingress needs a 'to' service id".into());
            }
            let cfg = exposure::load(root);
            let Some(policy) = cfg.get(to).cloned() else {
                return err(format!(
                    "trace: no such service '{to}' (set a CONNECT exposure policy first)"
                ));
            };
            let overlay = overlay_ip_of(root, &policy.source.node);
            // Lighthouse public IP isn't resolved here (it lives in the nebula
            // static_host_map, root-only) — None renders as unknown, never guessed.
            let mut g = route_trace::assemble_ingress(&policy, overlay.as_deref(), None);
            // ROUTE-TRACE-2: annotate the control points the path crosses with real
            // verdicts. The overlay hop is the Nebula firewall (open-mesh §8 → Allow);
            // the host→service hop crosses the hosting node's inbound firewall — its
            // live firewalld when local, else Indeterminate (never guessed remotely).
            let (port, proto) = (policy.source.port, policy.source.proto.as_str());
            g.evaluate_edge(
                "ingress->host",
                &route_trace::FirewallProfile::nebula_open_mesh(),
                port,
                proto,
            );
            g.evaluate_edge(
                "host->service",
                &host_inbound_firewall(&policy.source.node),
                port,
                proto,
            );
            g
        }
        "egress" => {
            if from.is_empty() {
                return err("trace: egress needs a 'from' node".into());
            }
            let dest = if to.is_empty() { "Internet" } else { to };
            let overlay = overlay_ip_of(root, from);
            route_trace::assemble_egress(from, overlay.as_deref(), dest)
        }
        other => return err(format!("trace: unknown direction '{other}'")),
    };

    match graph.to_json() {
        Ok(g) => format!("{{\"ok\":true,\"graph\":{g}}}"),
        Err(e) => err(format!("trace: encode: {e}")),
    }
}

/// Run the route Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &RouteService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &RouteService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "route responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "route responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::exposure::{ExposurePolicy, IngressBinding, ServiceSource, Tier};

    fn svc() -> (tempfile::TempDir, RouteService) {
        let tmp = tempfile::tempdir().unwrap();
        let s = RouteService::new(tmp.path().to_path_buf());
        (tmp, s)
    }

    fn save_public_service(root: &std::path::Path) {
        let mut cfg = exposure::ExposureConfig::default();
        cfg.upsert(ExposurePolicy {
            id: "grafana".into(),
            source: ServiceSource {
                node: "eagle".into(),
                port: 3000,
                proto: "tcp".into(),
                ..Default::default()
            },
            tier: Tier::PublicViaIngress,
            ingress: Some(IngressBinding {
                lighthouse: "Lighthouse-01".into(),
                hostname: "grafana.services.example".into(),
            }),
            ..Default::default()
        });
        exposure::save(root, &cfg).unwrap();
    }

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("trace"), "action/route/trace");
        assert!(ACTION_VERBS.contains(&"trace"));
    }

    #[test]
    fn ingress_trace_builds_graph_for_published_service() {
        let (_t, s) = svc();
        save_public_service(s.workgroup_root.as_path());
        let r = build_reply(
            &s,
            "trace",
            Some(&json!({"direction":"ingress","to":"grafana"}).to_string()),
        );
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["graph"]["direction"], "ingress");
        // 4 nodes: internet, ingress, host, service; public ⇒ no blocked_at.
        assert_eq!(v["graph"]["nodes"].as_array().unwrap().len(), 4);
        assert!(v["graph"].get("blocked_at").is_none() || v["graph"]["blocked_at"].is_null());
    }

    #[test]
    fn ingress_trace_annotates_real_control_points() {
        // ROUTE-TRACE-2: the overlay hop carries the Nebula open-mesh verdict; the
        // host→service hop is Indeterminate (the hosting node "eagle" is remote from
        // the test runner, so its firewalld can't be read — never guessed).
        let (_t, s) = svc();
        save_public_service(s.workgroup_root.as_path());
        let r = build_reply(
            &s,
            "trace",
            Some(&json!({"direction":"ingress","to":"grafana"}).to_string()),
        );
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        let edges = v["graph"]["edges"].as_array().unwrap();
        let overlay = edges
            .iter()
            .find(|e| e["from"] == "ingress" && e["to"] == "host")
            .unwrap();
        assert_eq!(overlay["control"]["verdict"], "allow");
        assert_eq!(overlay["control"]["firewall"], "nebula:overlay");
        let host_svc = edges
            .iter()
            .find(|e| e["from"] == "host" && e["to"] == "service")
            .unwrap();
        assert_eq!(host_svc["control"]["verdict"], "indeterminate");
        // Indeterminate must NOT mark the path blocked.
        assert!(v["graph"].get("blocked_at").is_none() || v["graph"]["blocked_at"].is_null());
    }

    #[test]
    fn ingress_trace_unknown_service_errors() {
        let (_t, s) = svc();
        let r = build_reply(
            &s,
            "trace",
            Some(&json!({"direction":"ingress","to":"nope"}).to_string()),
        );
        assert!(r.contains("no such service"), "{r}");
    }

    #[test]
    fn egress_trace_builds_host_to_internet() {
        let (_t, s) = svc();
        let r = build_reply(
            &s,
            "trace",
            Some(&json!({"direction":"egress","from":"eagle","to":"1.1.1.1"}).to_string()),
        );
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["graph"]["direction"], "egress");
        assert_eq!(v["graph"]["nodes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn missing_body_and_unknown_verb_error() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "trace", None).contains("missing request body"));
        assert!(build_reply(&s, "bogus", None).contains("unknown route verb"));
    }
}
