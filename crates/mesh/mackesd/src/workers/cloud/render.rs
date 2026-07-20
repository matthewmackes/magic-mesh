//! Workloads U4 + U5 (shared) — the pure tfvars renderer + `tofu plan -json` parser.
//!
//! This module is deliberately I/O-free: it turns a placement node's desired-state
//! slice into the `terraform.tfvars.json` document the `infra/tofu/cloud` root
//! consumes (U11's per-delivery-type `var.vms` shape), and it parses the newline-
//! delimited JSON stream `tofu plan -json` emits into a neutral [`PlanCounts`]. The
//! effectful legs — persisting the desired doc, shelling `tofu` — live in
//! [`super::reconcile`] / [`super::verbs::desired`] over the injectable seams, so
//! every function here is unit-tested without a hypervisor or a live store.
//!
//! Honest by construction (§7): [`parse_plan_counts`] returns a truthful `Err` when
//! `tofu` emitted no `change_summary` (a failed / aborted plan), never a fabricated
//! all-zero "in sync" [`PlanCounts`].

use mackes_mesh_types::cloud::{PlanCounts, WorkloadSpec};

/// The tfvars key carrying the libvirt connection URI for the placement node's
/// local hypervisor (E12 local-first default `qemu:///system`).
pub(crate) const TFVARS_LIBVIRT_URI: &str = "libvirt_uri";

/// The tfvars key carrying the per-node `for_each` workload map (`var.vms`).
pub(crate) const TFVARS_VMS: &str = "vms";

/// Render placement node `node`'s desired-state `specs` into the
/// `terraform.tfvars.json` document string the `infra/tofu/cloud` root consumes.
///
/// The document is a JSON object of exactly the two per-node-varying variables:
///
/// - `libvirt_uri` — the placement node's local hypervisor URI, so a remote apply
///   drives the right host;
/// - `vms` — a `for_each`-shaped map keyed by workload name, each value the
///   per-delivery-type shape `var.vms` declares (`delivery_type` / `vcpu` /
///   `memory_mb` / `disk_gb` / `image` / `network_isolation`).
///
/// Only specs actually placed on `node` are rendered (a defensive re-slice — the
/// caller already reads the per-node desired dir), so a stray cross-node doc never
/// leaks into another node's tfvars. Deterministic: the map is emitted in
/// name-sorted order via `serde_json`'s ordered object, so a re-render of an
/// unchanged slice is byte-identical (no spurious drift).
#[must_use]
pub(crate) fn render_tfvars(node: &str, specs: &[WorkloadSpec], libvirt_uri: &str) -> String {
    use std::collections::BTreeMap;

    let node = node.trim();
    let mut vms: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for spec in specs.iter().filter(|s| s.node.trim() == node) {
        let name = spec.name.trim();
        if name.is_empty() {
            continue;
        }
        vms.insert(
            name.to_string(),
            serde_json::json!({
                "delivery_type": spec.delivery_type.as_str(),
                "vcpu": spec.vcpu,
                "memory_mb": spec.memory_mb,
                "disk_gb": spec.disk_gb,
                "image": spec.image.clone().unwrap_or_default(),
                "network_isolation": spec.network_isolation,
            }),
        );
    }
    let doc = serde_json::json!({
        TFVARS_LIBVIRT_URI: libvirt_uri,
        TFVARS_VMS: vms,
    });
    // Pretty so a persisted tfvars.json reads cleanly + diffs sanely; falls back to
    // a compact form only if pretty-printing somehow fails (it cannot for this
    // shape, but we never `unwrap`).
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| doc.to_string())
}

/// Parse the newline-delimited JSON stream `tofu plan -json` emits into the neutral
/// [`PlanCounts`].
///
/// `tofu`/`terraform` `plan -json` emits one JSON object per line; the pending-change
/// tally rides the `{"type":"change_summary","changes":{"add":N,"change":N,
/// "remove":N,…}}` message. We scan for the LAST such message (a plan emits exactly
/// one, but a combined plan+refresh stream can carry an earlier refresh summary) and
/// map `remove` → [`PlanCounts::destroy`].
///
/// # Errors
/// Returns an honest `Err` when the stream carries no `change_summary` — a plan that
/// errored, was interrupted, or never ran. §7: never a fabricated all-zero "in sync"
/// [`PlanCounts`] (which would read as "nothing to do" and mask the failure).
pub(crate) fn parse_plan_counts(ndjson: &str) -> Result<PlanCounts, String> {
    let mut found: Option<PlanCounts> = None;
    let mut last_error: Option<String> = None;
    for line in ndjson.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Surface the most recent diagnostic so an errored plan reports *why*.
        if val.get("@level").and_then(|v| v.as_str()) == Some("error") {
            if let Some(msg) = val.get("@message").and_then(|v| v.as_str()) {
                last_error = Some(msg.to_string());
            }
        }
        if val.get("type").and_then(|v| v.as_str()) != Some("change_summary") {
            continue;
        }
        let Some(changes) = val.get("changes") else {
            continue;
        };
        let count = |key: &str| -> u32 {
            changes
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0)
        };
        found = Some(PlanCounts {
            add: count("add"),
            change: count("change"),
            destroy: count("remove"),
        });
    }
    found.ok_or_else(|| match last_error {
        Some(e) => format!("tofu plan reported no change summary: {e}"),
        None => "tofu plan produced no change summary (the plan did not complete)".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::DeliveryType;

    fn spec(name: &str, node: &str, dt: DeliveryType) -> WorkloadSpec {
        WorkloadSpec {
            name: name.to_string(),
            delivery_type: dt,
            node: node.to_string(),
            vcpu: 2,
            memory_mb: 2048,
            disk_gb: 20,
            image: None,
            network_isolation: false,
            raw_hcl: None,
        }
    }

    #[test]
    fn render_emits_the_libvirt_uri_and_a_for_each_map_of_only_this_nodes_specs() {
        let specs = vec![
            spec("web", "eagle", DeliveryType::ServiceVm),
            spec("phone", "eagle", DeliveryType::AndroidVm),
            // A stray spec placed on another node is defensively excluded.
            spec("elsewhere", "otter", DeliveryType::ServiceVm),
        ];
        let doc = render_tfvars("eagle", &specs, "qemu:///system");
        let v: serde_json::Value = serde_json::from_str(&doc).expect("valid json");
        assert_eq!(v[TFVARS_LIBVIRT_URI], "qemu:///system");
        let vms = v[TFVARS_VMS].as_object().expect("vms object");
        assert_eq!(vms.len(), 2, "only eagle's two specs render");
        assert!(vms.contains_key("web") && vms.contains_key("phone"));
        assert!(!vms.contains_key("elsewhere"));
        // The per-delivery-type shape the U11 var.vms object expects.
        assert_eq!(vms["web"]["delivery_type"], "service_vm");
        assert_eq!(vms["phone"]["delivery_type"], "android_vm");
        assert_eq!(vms["web"]["vcpu"], 2);
        assert_eq!(vms["web"]["memory_mb"], 2048);
        assert_eq!(vms["web"]["network_isolation"], false);
        assert_eq!(vms["web"]["image"], "");
    }

    #[test]
    fn render_is_deterministic_across_input_order() {
        let a = vec![
            spec("b", "n", DeliveryType::ServiceVm),
            spec("a", "n", DeliveryType::ServiceVm),
        ];
        let b = vec![
            spec("a", "n", DeliveryType::ServiceVm),
            spec("b", "n", DeliveryType::ServiceVm),
        ];
        assert_eq!(
            render_tfvars("n", &a, "qemu:///system"),
            render_tfvars("n", &b, "qemu:///system"),
            "the map is name-sorted so a re-render is byte-identical"
        );
    }

    #[test]
    fn parse_reads_the_change_summary_and_maps_remove_to_destroy() {
        let ndjson = concat!(
            r#"{"@level":"info","@message":"Refreshing...","type":"refresh_start"}"#,
            "\n",
            r#"{"@level":"info","@message":"Plan: 3 to add, 1 to change, 2 to destroy.","type":"change_summary","changes":{"add":3,"change":1,"remove":2,"operation":"plan"}}"#,
            "\n",
        );
        let counts = parse_plan_counts(ndjson).expect("summary parses");
        assert_eq!(
            counts,
            PlanCounts {
                add: 3,
                change: 1,
                destroy: 2
            }
        );
        assert!(!counts.is_noop());
    }

    #[test]
    fn parse_reads_a_no_op_plan_as_an_honest_zero_summary() {
        // A real no-op plan DOES emit a change_summary of all zeros — that is an
        // honest "in sync", distinct from a plan that never produced one.
        let ndjson = r#"{"type":"change_summary","changes":{"add":0,"change":0,"remove":0,"operation":"plan"}}"#;
        let counts = parse_plan_counts(ndjson).expect("zero summary parses");
        assert!(counts.is_noop());
    }

    #[test]
    fn parse_without_a_change_summary_is_an_honest_error_not_a_fake_in_sync() {
        // §7 — a plan that errored (no summary) must NOT read as all-zero in-sync.
        let errored = concat!(
            r#"{"@level":"error","@message":"Error: Failed to load plugin","type":"diagnostic"}"#,
            "\n",
        );
        let err = parse_plan_counts(errored).expect_err("no summary must fail");
        assert!(err.contains("Failed to load plugin"), "surfaces why: {err}");
        // Empty / garbage streams also fail honestly.
        assert!(parse_plan_counts("").is_err());
        assert!(parse_plan_counts("not json\n<html>").is_err());
    }
}
