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

/// Keep a hostile inventory from turning one read reply into an unbounded roster.
const MAX_INVENTORY_HOSTS: usize = 256;

/// Keep a hostile tofu state from turning one read reply into an unbounded output
/// table.
const MAX_TOFU_OUTPUTS: usize = 256;

/// A host can belong to many groups, all of which become row display data.
const MAX_GROUPS_PER_HOST: usize = 64;

/// Cap untrusted row labels and values before they cross into a [`CloudReply`].
const MAX_DISPLAY_CHARS: usize = 256;

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

    // Select a bounded, deterministic host set before building the per-host group
    // map. This keeps the fold itself bounded even when the tool reports a hostile
    // number of distinct host names.
    let mut selected_hosts = BTreeSet::new();

    for (group, membership) in root {
        if group == "_meta" {
            continue;
        }
        let Some(hosts) = membership.get("hosts").and_then(Value::as_array) else {
            continue;
        };
        for host in hosts.iter().filter_map(Value::as_str) {
            retain_bounded(&mut selected_hosts, host.to_string(), MAX_INVENTORY_HOSTS);
        }
    }

    // A host present only in `_meta.hostvars` (no group membership) still counts.
    if let Some(hv) = hostvars {
        for host in hv.keys() {
            retain_bounded(&mut selected_hosts, host.clone(), MAX_INVENTORY_HOSTS);
        }
    }

    // host -> its groups. A `BTreeMap`/`BTreeSet` gives deterministic ordering.
    let mut host_groups: BTreeMap<String, BTreeSet<String>> = selected_hosts
        .into_iter()
        .map(|host| (host, BTreeSet::new()))
        .collect();

    for (group, membership) in root {
        if group == "_meta" || group == "all" || group == "ungrouped" {
            continue;
        }
        let Some(hosts) = membership.get("hosts").and_then(Value::as_array) else {
            continue;
        };
        let group = bounded_display(group);
        for host in hosts.iter().filter_map(Value::as_str) {
            if let Some(groups) = host_groups.get_mut(host) {
                retain_bounded(groups, group.clone(), MAX_GROUPS_PER_HOST);
            }
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
                .map(bounded_display)
                .unwrap_or_else(|| bounded_display(&host));
            let reachable = vars
                .and_then(|v| v.get("reachable").or_else(|| v.get("alive")))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            InventoryHost {
                id: bounded_display(&host),
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

    let mut names = BTreeSet::new();
    for name in root.keys() {
        retain_bounded(&mut names, name.as_str(), MAX_TOFU_OUTPUTS);
    }

    let mut rows: Vec<TofuOutput> = names
        .into_iter()
        .filter_map(|name| {
            root.get(name).map(|entry| {
                let sensitive = entry
                    .get("sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let value = if sensitive {
                    SENSITIVE_PLACEHOLDER.to_string()
                } else {
                    entry
                        .get("value")
                        .map(render_value)
                        .map(|value| bounded_display(&value))
                        .unwrap_or_default()
                };
                TofuOutput {
                    name: bounded_display(name),
                    value,
                    sensitive,
                }
            })
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

/// Keep row text single-line, visible, and bounded. The read model does not retain
/// a second raw copy, so this is intentionally applied before constructing the
/// neutral rows rather than deferred to a particular shell renderer.
fn bounded_display(value: &str) -> String {
    let normalized: String = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '\u{00ad}'
                        | '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                )
            {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect();
    if normalized.chars().count() <= MAX_DISPLAY_CHARS {
        return normalized;
    }

    let mut bounded: String = normalized
        .chars()
        .take(MAX_DISPLAY_CHARS.saturating_sub(1))
        .collect();
    bounded.push('…');
    bounded
}

/// Retain the lexicographically first `cap` values, regardless of input order.
/// This makes a capped read stable while keeping only bounded folded state.
fn retain_bounded<T: Ord + Clone>(set: &mut BTreeSet<T>, value: T, cap: usize) {
    if cap == 0 || set.contains(&value) {
        return;
    }
    if set.len() >= cap {
        let Some(last) = set.iter().next_back().cloned() else {
            return;
        };
        if value >= last {
            return;
        }
        set.remove(&last);
    }
    set.insert(value);
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
    fn inventory_caps_host_rows_and_bounds_row_display_fields() {
        let long = "a".repeat(MAX_DISPLAY_CHARS + 32);
        let mut host_list = Vec::new();
        let mut hostvars = serde_json::Map::new();
        for index in 0..=MAX_INVENTORY_HOSTS {
            let host = if index == 0 {
                long.clone()
            } else {
                format!("host-{index:03}")
            };
            host_list.push(Value::String(host.clone()));
            hostvars.insert(host, serde_json::json!({"ansible_host": long}));
        }

        let long_group = format!("group-{}", "g".repeat(MAX_DISPLAY_CHARS + 32));
        let mut root = serde_json::Map::new();
        root.insert(
            "_meta".to_string(),
            serde_json::json!({"hostvars": hostvars}),
        );
        root.insert(long_group, serde_json::json!({"hosts": host_list}));

        let rows = parse_inventory(&Value::Object(root).to_string()).expect("parse");
        assert_eq!(rows.len(), MAX_INVENTORY_HOSTS);
        let long_row = rows
            .iter()
            .find(|row| row.id.starts_with('a'))
            .expect("lexicographically first hostile host is retained");
        assert_eq!(long_row.id.chars().count(), MAX_DISPLAY_CHARS);
        assert!(long_row.id.ends_with('…'));
        assert_eq!(long_row.node.chars().count(), MAX_DISPLAY_CHARS);
        assert!(long_row.node.ends_with('…'));
        assert_eq!(long_row.groups.len(), 1);
        assert_eq!(long_row.groups[0].chars().count(), MAX_DISPLAY_CHARS);
        assert!(long_row.groups[0].ends_with('…'));
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
    fn outputs_cap_rows_bound_display_fields_and_keep_sensitive_values_masked() {
        let long_name = "a".repeat(MAX_DISPLAY_CHARS + 32);
        let long_value = "v".repeat(MAX_DISPLAY_CHARS + 32);
        let secret = "secret-that-must-not-cross-the-boundary";
        let mut root = serde_json::Map::new();
        root.insert(
            long_name,
            serde_json::json!({"value": long_value, "sensitive": false}),
        );
        root.insert(
            "admin_password".to_string(),
            serde_json::json!({"value": secret, "sensitive": true}),
        );
        for index in 0..MAX_TOFU_OUTPUTS {
            root.insert(
                format!("output-{index:03}"),
                serde_json::json!({"value": "ok", "sensitive": false}),
            );
        }

        let rows = parse_outputs(&Value::Object(root).to_string()).expect("parse");
        assert_eq!(rows.len(), MAX_TOFU_OUTPUTS);
        let long_row = rows
            .iter()
            .find(|row| row.name.starts_with('a'))
            .expect("lexicographically first hostile output is retained");
        assert_eq!(long_row.name.chars().count(), MAX_DISPLAY_CHARS);
        assert!(long_row.name.ends_with('…'));
        assert_eq!(long_row.value.chars().count(), MAX_DISPLAY_CHARS);
        assert!(long_row.value.ends_with('…'));

        let sensitive = rows
            .iter()
            .find(|row| row.name == "admin_password")
            .expect("sensitive output is retained");
        assert!(sensitive.sensitive);
        assert_eq!(sensitive.value, SENSITIVE_PLACEHOLDER);
        assert!(!rows.iter().any(|row| row.value.contains(secret)));
    }

    #[test]
    fn bounded_display_replaces_invisible_text_and_truncates_on_character_boundaries() {
        let hostile = format!("line\n\u{202e}{}", "🚗".repeat(MAX_DISPLAY_CHARS));
        let display = bounded_display(&hostile);
        assert!(display.chars().count() <= MAX_DISPLAY_CHARS);
        assert!(!display.contains('\n'));
        assert!(!display.contains('\u{202e}'));
        assert!(display.ends_with('…'));
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
