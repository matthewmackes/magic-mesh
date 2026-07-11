//! `PeersWhy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `peers-why` subcommand.
#[allow(unreachable_code)]
pub fn run(node_id: String, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Phase 12.4.4 — explanation surface. Loads the node
        // roster from the store, runs `topology::calculate`,
        // and walks the resulting edge set + route table to
        // emit a per-edge reason chain for the named peer.
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let nodes = mackesd_core::store::list_nodes(&conn).context("listing nodes from store")?;
        let report = explain_peer(&node_id, &nodes);
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

/// Build the JSON `peers why` report from a node roster (Phase
/// 12.4.4). Pure function over the store projection so callers can
/// unit-test the reason-chain shape without a real DB.
fn explain_peer(node_id: &str, nodes: &[mackesd_core::store::NodeRow]) -> serde_json::Value {
    let subject = nodes.iter().find(|n| n.node_id == node_id);
    let Some(subject) = subject else {
        return serde_json::json!({
            "node":     node_id,
            "known":    false,
            "reasons":  [],
            "note":     "node id not present in store — run `mackesd inventory-legacy` and `mackesd import-legacy` to seed.",
        });
    };
    let healthy_subject = subject.health == "healthy";
    let reasons: Vec<serde_json::Value> = nodes
        .iter()
        .filter(|other| other.node_id != node_id)
        .map(|other| {
            let same_region = match (&subject.region, &other.region) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let both_healthy = healthy_subject && other.health == "healthy";
            let chain: Vec<&str> = {
                let mut v = Vec::new();
                if both_healthy {
                    v.push("both peers healthy");
                } else {
                    v.push("one or both peers not healthy");
                }
                if same_region {
                    v.push("same region — east-west allowed by default");
                } else {
                    v.push("different regions — gated on policy::allow_east_west");
                }
                if subject.role == "decommissioned" || other.role == "decommissioned" {
                    v.push("decommissioned — no edge expected");
                }
                v
            };
            serde_json::json!({
                "peer":       other.node_id,
                // An edge is expected when both peers are healthy and
                // neither is decommissioned. East-west (cross-region)
                // is allowed by default today, so region does NOT gate
                // `expected` (the `reasons` above still surface the
                // region context). The previous `&& (same_region ||
                // true)` term was always true — a logic bug (clippy
                // overly_complex_bool_expr); a real
                // `policy::allow_east_west` gate would re-add a
                // `(same_region || allow_east_west)` term here.
                "expected":   both_healthy
                              && subject.role != "decommissioned"
                              && other.role != "decommissioned",
                "chain":      chain,
            })
        })
        .collect();
    serde_json::json!({
        "node":    node_id,
        "known":   true,
        "region":  subject.region,
        "role":    subject.role,
        "health":  subject.health,
        "reasons": reasons,
    })
}
