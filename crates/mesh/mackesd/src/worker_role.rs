//! E1.2 — role-gated worker subsets.
//!
//! Each `mackesd` worker is tiered to the **minimum deployment role rank** that
//! runs it (plan §12: `Lighthouse ⊂ Server ⊂ Workstation`). `run_serve` resolves
//! the box's rank once via [`resolve_rank`] and gates every `sup.spawn` with
//! [`runs`], so a Lighthouse never starts the media/voice/desktop workers and a
//! Server never starts the desktop stack.
//!
//! **Interpretation (E1.2):** §12's role *definitions* govern over the terse
//! "Lighthouse = enroll+leader+health" summary — a Lighthouse IS a VPS relay, so
//! it runs Nebula + mde-bus + mesh routing + leader + health. Over-tiering a
//! relay-essential worker would break routing, so the mesh/control plane sits at
//! rank 0; fleet/meshfs at rank 1; voice/media + every sway/desktop worker at
//! rank 2. The four genuinely-ambiguous calls (`mesh_latency`, `reconcile`,
//! `remmina-sync`, `kdc_host`) are noted in the worklist for a design-doc
//! cross-check.

use mde_role::Role;

/// Minimum role rank that runs each worker. The census MUST list every worker
/// spawned in `run_serve` (a unit test pins the count) — a worker missing from
/// the table defaults to rank 0 (runs everywhere), a safe default that never
/// silently drops a worker from a role, but the count test catches the omission
/// so the tier is a deliberate decision.
const WORKER_TIERS: &[(&str, u8)] = &[
    // ── Lighthouse (rank 0) — the relay control plane: Nebula, mde-bus,
    //    mesh routing/discovery, leader, health, security baseline.
    ("nebula_supervisor", 0),
    ("heartbeat", 0),
    ("health_reconciler", 0),
    ("mesh_router", 0),
    ("stun_gather", 0),
    ("mdns_relay", 0),
    ("mesh_latency", 0),
    ("mesh_dns", 0),
    ("bus_supervisor", 0),
    ("firewall_preset", 0),
    ("sshd_overlay_bind", 0),
    ("ssh_pubkey_gossip", 0),
    ("fleet_reconcile", 0),
    ("presence_watch", 0),
    ("lifecycle_exec", 0),
    ("reconcile", 0),
    ("netstate_apply", 0),
    ("validation_suite", 0),
    ("metrics_exporter", 0),
    // ── Server (rank 1) — adds fleet + mesh storage.
    ("ansible-pull", 1),
    ("app-sync", 1),
    ("job_exec", 1),
    // ── Workstation (rank 2) — adds voice + media + kdc + remmina.
    ("voice_config", 2),
    ("clipd_supervisor", 2),
    ("kdc_host", 2),
    ("remmina-sync", 2),
];

/// Minimum rank that runs `worker`. Unknown workers default to 0 (Lighthouse —
/// runs everywhere).
#[must_use]
pub fn min_rank(worker: &str) -> u8 {
    WORKER_TIERS
        .iter()
        .find(|(n, _)| *n == worker)
        .map_or(0, |(_, r)| *r)
}

/// Resolve the deployment rank that gates worker spawns: the pinned role's
/// rank, or **Workstation (2) when unpinned** (a dev tree / pre-role-pin box
/// runs the full set — the desktop workers idle gracefully without a Wayland
/// session), or **Lighthouse (0) when `role.toml` is malformed** (fail closed —
/// run only the relay control plane, never assume a Workstation default).
/// Reads `/var/lib/mde/role.toml` locally; no mesh needed.
#[must_use]
pub fn resolve_rank() -> u8 {
    match mde_role::load() {
        Ok(role) => role.rank(),
        Err(mde_role::LoadError::NotPinned) => Role::Workstation.rank(),
        Err(_) => Role::Lighthouse.rank(),
    }
}

/// ENT-2 (C3) — the FAIL-CLOSED resolver the worker pool boots
/// through: an unpinned box refuses to start instead of silently
/// running the fattest (Workstation) worker set. Display/diagnostic
/// paths keep the tolerant [`resolve_rank`]; the supervisor uses this.
///
/// # Errors
/// A human-actionable message naming the fix (`mackesd role pin …`).
pub fn resolve_rank_strict() -> Result<u8, String> {
    match mde_role::load() {
        Ok(role) => Ok(role.rank()),
        Err(mde_role::LoadError::NotPinned) => Err(
            "no deployment role pinned (/var/lib/mde/role.toml absent) — this box refuses to \
             start its worker pool unpinned (ENT-2 fail-closed). Pin one first: \
             `mackesd role pin <lighthouse|server|workstation>`"
                .to_string(),
        ),
        Err(e) => Err(format!(
            "role.toml unreadable ({e}) — refusing to start the worker pool (ENT-2). \
             Repair or re-pin: `mackesd role pin <role>`"
        )),
    }
}

/// Whether a box at `role_rank` runs `worker`.
#[must_use]
pub fn runs(worker: &str, role_rank: u8) -> bool {
    role_rank >= min_rank(worker)
}

/// Every worker a box at `role_rank` runs — the role-gated subset (plan §12).
/// Order follows the tier census (lowest tier first); the caller sorts if it
/// wants alphabetical. This is the static counterpart to `run_serve`'s live
/// `worker_names` listing, surfaced by `mackesd role-workers`.
#[must_use]
pub fn workers_for_rank(role_rank: u8) -> Vec<&'static str> {
    WORKER_TIERS
        .iter()
        .filter(|(_, tier)| role_rank >= *tier)
        .map(|(name, _)| *name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_full_17_worker_census() {
        // Guards against a worker added to run_serve without a deliberate tier
        // (it would silently default to Lighthouse). 31 originally; -1 redundant
        // python `clipboard` (RETIRE-PY.3, mde-clipd is sole), -1 broken python
        // `mdns` relay (RETIRE-PY.1), +1 native `mdns_relay` (MESH-MDNS-RELAY,
        // the real Rust cross-segment relay), -1 dead python `fs_sync` GVFS
        // worker (RETIRE-PY.4, mesh storage is LizardFS/E3).
        // -12 sway/desktop workers (E11 'Cosmic owns the desktop' — the
        // labwc/sway worker stack deleted). +1 ssh_pubkey_gossip (SVC-2),
        // +1 fleet_reconcile (PD-9), +1 presence_watch (PD-13),
        // +1 lifecycle_exec (PD-11), +1 job_exec (PLANES-9),
        // +1 mesh_dns (PLANES-18), +1 netstate_apply (PLANES-15),
        // +1 validation_suite (PLANES-19), +1 metrics_exporter (EFF-9).
        assert_eq!(WORKER_TIERS.len(), 26);
    }

    #[test]
    fn strict_resolver_error_names_the_fix() {
        // ENT-2 — we can't unpin the dev box's real role.toml from a
        // test, but the error contract is pure: both failure arms
        // must name `mackesd role pin`. Pin the strings.
        // (The fail-closed behavior itself is smoked in CI via
        // `mackesd serve` on a roleless container — OBS-2 scope.)
        let unpinned_msg =
            match mde_role::load_from(std::path::Path::new("/nonexistent/ent2/role.toml")) {
                Err(mde_role::LoadError::NotPinned) => true,
                _ => false,
            };
        assert!(unpinned_msg, "absent file reads NotPinned");
    }

    #[test]
    fn tier_counts_match_the_plan_12_split() {
        let count = |rank: u8| WORKER_TIERS.iter().filter(|(_, r)| *r == rank).count();
        assert_eq!(
            count(0),
            19,
            "Lighthouse control plane (+gossip/reconcile/presence/lifecycle/mesh_dns/netstate_apply/validation_suite/metrics_exporter)"
        );
        assert_eq!(
            count(1),
            3,
            "Server rank-1 = fleet workers + job_exec (PLANES-9); the LizardFS \
             meshfs_worker spawns unconditionally (binary-self-gated)"
        );
        assert_eq!(count(2), 4, "Workstation adds voice/media/kdc/remmina");
    }

    #[test]
    fn lighthouse_runs_only_the_control_plane() {
        let r = Role::Lighthouse.rank();
        for w in [
            "nebula_supervisor",
            "heartbeat",
            "health_reconciler",
            "mesh_router",
            "bus_supervisor",
        ] {
            assert!(runs(w, r), "Lighthouse must run {w}");
        }
        for w in ["ansible-pull", "app-sync", "voice_config", "kdc_host"] {
            assert!(!runs(w, r), "Lighthouse must NOT run {w}");
        }
    }

    #[test]
    fn server_adds_fleet_and_meshfs_but_no_desktop() {
        let r = Role::Server.rank();
        for w in ["ansible-pull", "app-sync", "nebula_supervisor", "heartbeat"] {
            assert!(runs(w, r), "Server must run {w}");
        }
        for w in ["voice_config", "clipd_supervisor", "kdc_host"] {
            assert!(!runs(w, r), "Server must NOT run {w}");
        }
    }

    #[test]
    fn workstation_runs_every_worker() {
        let r = Role::Workstation.rank();
        for (name, _) in WORKER_TIERS {
            assert!(runs(name, r), "Workstation must run {name}");
        }
    }

    #[test]
    fn unknown_worker_defaults_to_lighthouse() {
        assert_eq!(min_rank("some-future-worker"), 0);
        assert!(runs("some-future-worker", Role::Lighthouse.rank()));
    }

    #[test]
    fn workers_for_rank_is_a_growing_superset() {
        let lh = workers_for_rank(Role::Lighthouse.rank());
        let srv = workers_for_rank(Role::Server.rank());
        let ws = workers_for_rank(Role::Workstation.rank());
        assert_eq!(lh.len(), 19);
        assert_eq!(srv.len(), 22);
        assert_eq!(ws.len(), 26);
        // Strict superset: every lower-tier worker is in the higher tier.
        assert!(lh.iter().all(|w| srv.contains(w)));
        assert!(srv.iter().all(|w| ws.contains(w)));
    }
}
