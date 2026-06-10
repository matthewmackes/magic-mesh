//! PLANES-20 / ENT-8 — the fleet rollup aggregation.
//!
//! Groups the node roster by **role** and summarises each group's health
//! into the card the Fleet-rollup dashboard renders (W86): member count,
//! a per-state breakdown, and the group's **worst** health (the headline
//! a card shows). Pure over `(role, health)` pairs so it's unit-tested
//! without a store; `mackesd fleet-status --json` maps the node rows into
//! it and the panel consumes the JSON.

use serde::Serialize;

/// Health states, worst-first. The order IS the severity ranking used by
/// [`worst_health`].
const SEVERITY: [&str; 4] = ["unreachable", "degraded", "unknown", "healthy"];

/// One role group's rollup card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleRollup {
    /// `host` | `peer` | `observer` | …
    pub role: String,
    /// Members in this role.
    pub total: usize,
    /// Members reporting `healthy`.
    pub healthy: usize,
    /// Members reporting `degraded`.
    pub degraded: usize,
    /// Members reporting `unreachable`.
    pub unreachable: usize,
    /// Members reporting `unknown` (or any unrecognised state).
    pub unknown: usize,
    /// The worst health present in the group (the card headline).
    pub worst_health: String,
}

/// The worst (most severe) health among `states`, or `"healthy"` for an
/// empty set. Unrecognised states rank as `unknown`.
#[must_use]
pub fn worst_health<'a>(states: impl IntoIterator<Item = &'a str>) -> String {
    let mut worst_rank = SEVERITY.len(); // start past "healthy"
    for s in states {
        let canon = if SEVERITY.contains(&s) { s } else { "unknown" };
        if let Some(rank) = SEVERITY.iter().position(|x| *x == canon) {
            worst_rank = worst_rank.min(rank);
        }
    }
    SEVERITY
        .get(worst_rank.min(SEVERITY.len() - 1))
        .unwrap_or(&"healthy")
        .to_string()
}

/// Group `(role, health)` pairs into per-role rollups, sorted by role.
#[must_use]
pub fn rollup(nodes: &[(String, String)]) -> Vec<RoleRollup> {
    use std::collections::BTreeMap;
    let mut by_role: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (role, health) in nodes {
        by_role.entry(role).or_default().push(health);
    }
    by_role
        .into_iter()
        .map(|(role, healths)| {
            let count = |s: &str| healths.iter().filter(|h| **h == s).count();
            let healthy = count("healthy");
            let degraded = count("degraded");
            let unreachable = count("unreachable");
            // anything not one of the three known-bad/good states is unknown.
            let unknown = healths.len() - healthy - degraded - unreachable;
            RoleRollup {
                role: role.to_string(),
                total: healths.len(),
                healthy,
                degraded,
                unreachable,
                unknown,
                worst_health: worst_health(healths.iter().copied()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(role: &str, health: &str) -> (String, String) {
        (role.to_string(), health.to_string())
    }

    #[test]
    fn worst_health_ranks_unreachable_above_degraded_above_unknown_above_healthy() {
        assert_eq!(worst_health(["healthy", "healthy"]), "healthy");
        assert_eq!(worst_health(["healthy", "unknown"]), "unknown");
        assert_eq!(worst_health(["unknown", "degraded"]), "degraded");
        assert_eq!(worst_health(["degraded", "unreachable", "healthy"]), "unreachable");
        assert_eq!(worst_health(std::iter::empty()), "healthy");
        // Unrecognised states fall back to unknown severity.
        assert_eq!(worst_health(["weird", "healthy"]), "unknown");
    }

    #[test]
    fn rollup_groups_by_role_with_counts_and_worst() {
        let nodes = vec![
            pair("host", "healthy"),
            pair("peer", "healthy"),
            pair("peer", "degraded"),
            pair("peer", "unreachable"),
        ];
        let groups = rollup(&nodes);
        assert_eq!(groups.len(), 2);
        // Sorted by role: host then peer.
        assert_eq!(groups[0].role, "host");
        assert_eq!(groups[0].total, 1);
        assert_eq!(groups[0].worst_health, "healthy");
        assert_eq!(groups[1].role, "peer");
        assert_eq!(groups[1].total, 3);
        assert_eq!(groups[1].healthy, 1);
        assert_eq!(groups[1].degraded, 1);
        assert_eq!(groups[1].unreachable, 1);
        assert_eq!(groups[1].worst_health, "unreachable");
    }

    #[test]
    fn unrecognised_health_counts_as_unknown() {
        let groups = rollup(&[pair("host", "starting"), pair("host", "healthy")]);
        assert_eq!(groups[0].unknown, 1);
        assert_eq!(groups[0].healthy, 1);
        assert_eq!(groups[0].worst_health, "unknown");
    }
}
