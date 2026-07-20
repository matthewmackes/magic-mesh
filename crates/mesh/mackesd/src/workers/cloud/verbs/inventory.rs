//! Workloads U10 — the `inventory` + `output` READ verb handlers.
//!
//! Both are READS: the drain serves them locally on every node (no placement gate),
//! so an operator sees the mesh inventory / a node's tofu outputs from wherever they
//! ask. Each shells its tool through the injectable [`CloudRunner`] seam
//! (production shells `ansible-inventory --list` / `tofu output -json`; tests script
//! the JSON), and this module owns only the parse → neutral-type fold.
//!
//! - **`inventory`** → `ansible-inventory --list` → [`InventoryHost`] rows (id · node
//!   · groups · reachable) — what a `configure` run would target.
//! - **`output`** → `tofu output -json` → [`TofuOutput`] rows (name · value ·
//!   sensitive) — a workload's instance roster / IPs.
//!
//! Honest (§7): an absent/failed tool is an honest `gated`; unparseable output is an
//! honest `error` carrying the raw log — never a fabricated inventory / output set. A
//! `sensitive` tofu output's value is withheld at the source (not persisted to the
//! bus in the clear), carrying only the `sensitive` flag for the shell to render.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use mackes_mesh_types::cloud::{CloudReply, InventoryHost, TofuOutput};

use super::super::CloudWorker;

/// The masked stand-in for a `sensitive` tofu output — the real value is never
/// persisted to the bus, only its existence + the sensitive flag.
const SENSITIVE_PLACEHOLDER: &str = "(sensitive — value withheld)";

/// Cap the raw-log detail carried on a parse failure (a full inventory dump can be
/// large; the shell's expandable raw-log pane wants the head, not megabytes).
const RAW_LOG_CAP: usize = 4096;

/// Handle `action/cloud/inventory` → the resolved mesh Ansible inventory.
pub(super) fn handle_inventory(w: &CloudWorker, verb_name: &str) -> CloudReply {
    match w.runner.resolve_inventory() {
        Ok(json) => match parse_inventory(&json) {
            Ok(hosts) => CloudReply {
                ok: true,
                verb: verb_name.to_string(),
                inventory: Some(hosts),
                ..Default::default()
            },
            Err(e) => CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("mesh inventory could not be parsed: {e}")),
                raw_log: Some(truncate(&json)),
                ..Default::default()
            },
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("mesh inventory not ready: {e}")),
            ..Default::default()
        },
    }
}

/// Handle `action/cloud/output` → a node's tofu outputs.
pub(super) fn handle_output(w: &CloudWorker, verb_name: &str) -> CloudReply {
    match w.runner.tofu_outputs() {
        Ok(json) => match parse_outputs(&json) {
            Ok(outputs) => CloudReply {
                ok: true,
                verb: verb_name.to_string(),
                outputs: Some(outputs),
                ..Default::default()
            },
            Err(e) => CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("tofu outputs could not be parsed: {e}")),
                raw_log: Some(truncate(&json)),
                ..Default::default()
            },
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("tofu outputs not ready: {e}")),
            ..Default::default()
        },
    }
}

/// Parse `ansible-inventory --list` JSON into [`InventoryHost`] rows.
///
/// The shape: a top-level object of `group -> {"hosts":[...], "children":[...]}`
/// plus `_meta.hostvars.<host>.<var>`. A host's `node` is its `ansible_host` hostvar
/// (else the host id); its `groups` are the non-synthetic groups listing it; its
/// `reachable` is an explicit `reachable`/`alive` hostvar, defaulting `true` (the
/// mesh dynamic inventory only emits hosts that hold a live keepalive lease).
/// Deterministic ordering (BTree) so the surface renders stably.
fn parse_inventory(json: &str) -> Result<Vec<InventoryHost>, String> {
    let value: Value = serde_json::from_str(json.trim()).map_err(|e| e.to_string())?;
    let root = value
        .as_object()
        .ok_or_else(|| "inventory root is not a JSON object".to_string())?;

    let hostvars = root
        .get("_meta")
        .and_then(|m| m.get("hostvars"))
        .and_then(Value::as_object);

    // host -> its groups. A `BTreeMap`/`BTreeSet` gives deterministic ordering.
    let mut host_groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (group, membership) in root {
        if group == "_meta" {
            continue;
        }
        let Some(hosts) = membership.get("hosts").and_then(Value::as_array) else {
            continue;
        };
        for host in hosts.iter().filter_map(Value::as_str) {
            let entry = host_groups.entry(host.to_string()).or_default();
            // `all` / `ungrouped` are synthetic roll-ups, not meaningful groups.
            if group != "all" && group != "ungrouped" {
                entry.insert(group.clone());
            }
        }
    }

    // A host present only in `_meta.hostvars` (no group membership) still counts.
    if let Some(hv) = hostvars {
        for host in hv.keys() {
            host_groups.entry(host.clone()).or_default();
        }
    }

    let rows = host_groups
        .into_iter()
        .map(|(host, groups)| {
            let vars = hostvars
                .and_then(|hv| hv.get(&host))
                .and_then(Value::as_object);
            let node = vars
                .and_then(|v| v.get("ansible_host"))
                .and_then(Value::as_str)
                .unwrap_or(&host)
                .to_string();
            let reachable = vars
                .and_then(|v| v.get("reachable").or_else(|| v.get("alive")))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            InventoryHost {
                id: host,
                node,
                groups: groups.into_iter().collect(),
                reachable,
            }
        })
        .collect();
    Ok(rows)
}

/// Parse `tofu output -json` into [`TofuOutput`] rows (name-sorted for a stable
/// render). A `sensitive` output's value is withheld ([`SENSITIVE_PLACEHOLDER`]) —
/// the secret never reaches the bus; the flag rides along for the shell to mask.
fn parse_outputs(json: &str) -> Result<Vec<TofuOutput>, String> {
    let value: Value = serde_json::from_str(json.trim()).map_err(|e| e.to_string())?;
    let root = value
        .as_object()
        .ok_or_else(|| "tofu output root is not a JSON object".to_string())?;

    let mut rows: Vec<TofuOutput> = root
        .iter()
        .map(|(name, entry)| {
            let sensitive = entry
                .get("sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let value = if sensitive {
                SENSITIVE_PLACEHOLDER.to_string()
            } else {
                entry.get("value").map(render_value).unwrap_or_default()
            };
            TofuOutput {
                name: name.clone(),
                value,
                sensitive,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Render a tofu output value to a string: a bare JSON string is used verbatim; any
/// other shape (list/object/number/bool) is its compact JSON form.
fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Head of `s`, capped at [`RAW_LOG_CAP`] bytes on a char boundary.
fn truncate(s: &str) -> String {
    if s.len() <= RAW_LOG_CAP {
        return s.to_string();
    }
    let mut end = RAW_LOG_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── inventory parse ──

    const INVENTORY_JSON: &str = r#"{
        "_meta": {
            "hostvars": {
                "eagle": {"ansible_host": "10.42.0.7", "reachable": true},
                "otter": {"ansible_host": "10.42.0.9", "alive": false},
                "lonely": {"ansible_host": "10.42.0.3"}
            }
        },
        "all": {"children": ["role_seat", "ungrouped"]},
        "role_seat": {"hosts": ["eagle", "otter"]},
        "delivery_desktop_vm": {"hosts": ["eagle"]},
        "ungrouped": {"hosts": ["lonely"]}
    }"#;

    #[test]
    fn inventory_folds_hosts_groups_node_and_reachability() {
        let hosts = parse_inventory(INVENTORY_JSON).expect("parse");
        // Deterministic (BTree) order: eagle, lonely, otter.
        assert_eq!(hosts.len(), 3);
        let eagle = &hosts[0];
        assert_eq!(eagle.id, "eagle");
        assert_eq!(eagle.node, "10.42.0.7", "ansible_host is the node addr");
        assert_eq!(eagle.groups, vec!["delivery_desktop_vm", "role_seat"]);
        assert!(eagle.reachable);

        let otter = hosts.iter().find(|h| h.id == "otter").unwrap();
        assert!(!otter.reachable, "explicit alive:false is honoured");
        assert_eq!(otter.groups, vec!["role_seat"]);

        let lonely = hosts.iter().find(|h| h.id == "lonely").unwrap();
        // `ungrouped` is synthetic → no meaningful groups.
        assert!(lonely.groups.is_empty());
        assert!(lonely.reachable, "defaults reachable when unstated");
    }

    #[test]
    fn inventory_parse_rejects_a_non_object_root() {
        assert!(parse_inventory("[]").is_err());
        assert!(parse_inventory("not json").is_err());
    }

    #[test]
    fn inventory_handler_serves_the_parsed_roster_via_the_runner() {
        let w = worker_with_reads(Some(Ok(INVENTORY_JSON.to_string())), None);
        let reply = handle_inventory(&w, "inventory");
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert_eq!(reply.inventory.unwrap().len(), 3);
    }

    #[test]
    fn inventory_handler_gates_an_absent_tool() {
        let w = worker_with_reads(Some(Err("ansible-inventory unavailable".into())), None);
        let reply = handle_inventory(&w, "inventory");
        assert!(!reply.ok);
        assert!(reply.inventory.is_none(), "no fabricated inventory");
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    #[test]
    fn inventory_handler_errors_with_raw_log_on_unparseable_output() {
        let w = worker_with_reads(Some(Ok("<<garbage>>".to_string())), None);
        let reply = handle_inventory(&w, "inventory");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("could not be parsed"));
        assert_eq!(reply.raw_log.as_deref(), Some("<<garbage>>"));
    }

    // ── output parse ──

    const OUTPUT_JSON: &str = r#"{
        "instance_ips": {"value": ["10.0.0.5", "10.0.0.6"], "type": ["tuple", []], "sensitive": false},
        "admin_password": {"value": "hunter2", "type": "string", "sensitive": true},
        "vm_name": {"value": "web", "type": "string", "sensitive": false}
    }"#;

    #[test]
    fn outputs_fold_name_value_and_sensitive_masking() {
        let outputs = parse_outputs(OUTPUT_JSON).expect("parse");
        // Name-sorted: admin_password, instance_ips, vm_name.
        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[0].name, "admin_password");
        assert!(outputs[0].sensitive);
        assert_eq!(
            outputs[0].value, SENSITIVE_PLACEHOLDER,
            "a secret is withheld, never persisted to the bus"
        );

        assert_eq!(outputs[1].name, "instance_ips");
        assert!(!outputs[1].sensitive);
        assert_eq!(
            outputs[1].value, r#"["10.0.0.5","10.0.0.6"]"#,
            "list → compact JSON"
        );

        assert_eq!(outputs[2].name, "vm_name");
        assert_eq!(outputs[2].value, "web", "bare string used verbatim");
    }

    #[test]
    fn output_parse_rejects_a_non_object_root() {
        assert!(parse_outputs("[1,2]").is_err());
    }

    #[test]
    fn output_handler_serves_parsed_outputs_via_the_runner() {
        let w = worker_with_reads(None, Some(Ok(OUTPUT_JSON.to_string())));
        let reply = handle_output(&w, "output");
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert_eq!(reply.outputs.unwrap().len(), 3);
    }

    #[test]
    fn output_handler_gates_an_absent_tool() {
        let w = worker_with_reads(None, Some(Err("tofu output unavailable".into())));
        let reply = handle_output(&w, "output");
        assert!(!reply.ok);
        assert!(reply.outputs.is_none());
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    #[test]
    fn render_value_handles_strings_lists_and_scalars() {
        assert_eq!(render_value(&Value::String("x".into())), "x");
        assert_eq!(render_value(&serde_json::json!(42)), "42");
        assert_eq!(render_value(&serde_json::json!(true)), "true");
        assert_eq!(render_value(&serde_json::json!(["a", "b"])), r#"["a","b"]"#);
    }

    // ── a worker wired with scripted runner reads ──

    fn worker_with_reads(
        inventory_json: Option<Result<String, String>>,
        outputs_json: Option<Result<String, String>>,
    ) -> CloudWorker {
        use super::super::super::runner::fake::FakeRunner;
        use std::path::PathBuf;
        use std::sync::Arc;
        CloudWorker::new("me".into(), "peer:me".into(), PathBuf::from("/tmp"))
            .with_runner(Arc::new(FakeRunner {
                inventory_json,
                outputs_json,
                ..Default::default()
            }))
            .with_bus_root(None)
    }
}
