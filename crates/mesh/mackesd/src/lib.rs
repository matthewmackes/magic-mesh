//! `mackesd_core` — the authoritative read API for the Mesh control
//! plane. Linked directly into `mackes-panel` (no IPC, no networked
//! API per Phase 12.A.3 lock 2026-05-19).
//!
//! Module organization mirrors the 8-layer architecture in
//! `docs/PROJECT_WORKLIST.md` § Phase 12. Modules land one at a time
//! as their substeps ship; only those whose substep is `[✓] Done`
//! are exposed here.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audit;
// NF-2 (v2.5) — Nebula CA module. Owns mint / sign / seal /
// bundle. Reachable from `bin/mackesd.rs::run_serve` via the
// upcoming NF-3.4 supervisor and from the CLI's `mackesd ca`
// subcommand (NF-2.6). The whole module lands together per
// §0.12; no scaffold-only commit.
pub mod ca;
// MESH-A-7 (v5.0.0) — well-known port → connect-action mappings,
// consumed by the `mackesd connect` CLI + (future) host-card UI.
pub mod connect_actions;
// MESH-A-9 (v5.0.0) — audit log of network-state changes →
// kind="audit" activity entries, consumed by `mackesd audit-log`.
pub mod audit_log;
// HYP-8.5 (v6.5) — operator-edited config-file modules. Currently
// hosts the tag-manifest schema + loader (`~/.config/mde/tags/`).
// Future v6.5 config families (Hyprland-side per-peer overrides,
// etc.) land here as siblings.
pub mod config;
// v2.0.0 Phase 12.1.2 — fleet deploy layer reservation. When Phase G
// submodules (push, rollback, ansible_pull orchestration) actually
// ship with real code, `pub mod deploy;` + `crates/mackesd/src/deploy/`
// come back together in one commit. The empty-scaffold version was
// deleted 2026-05-22 per .claude/CLAUDE.md §0.12 (no stubs).
pub mod enrollment;
pub mod events;
pub mod fleet;
pub mod health;
// MESHFS-14.1 (v5.0.0) — LizardFS state snapshot for the backup bundle.
pub mod identity;
pub mod leader;
pub mod legacy_inventory;
pub mod logging;
pub mod meshfs;
// EPIC-SYNC-APP-CONFIG (Q26) — native-Rust mesh media-server
// discovery (replaces the discovery half of the retired
// `mackes/mesh_media.py`). Consumed by `workers::app_sync`.
pub mod mesh_media;
pub mod metrics;
// NF-3.6.a (v2.5) — peer-side enrollment via the
// `mesh:<id>@<ip>:<port>#<bearer>` join-token shape. Publishes a
// pending-enroll CSR to QNM-Shared + polls for the lighthouse-
// signed bundle. Consumed by the `mackesd enroll --token` CLI +
// the future NF-3.6 D-Bus method.
pub mod nebula_enroll;
// NF-18.2 (v2.5) — typed export of the live nebula_peer_certs
// table, joined with nodes.role for the groups column. Pure-fn
// SQL projection; consumed by the `mackesd nebula export-roster`
// CLI + by the NF-18.4 automated backup worker (planned).
pub mod nebula_roster;
// PLANES-17 — Nebula topology as fleet state: hop subnet routes, exit
// nodes (validation-gated), and external VPN client profiles.
pub mod nebula_topology;
// PLANES-20 / ENT-8 — fleet rollup aggregation (roster grouped by role +
// worst-health) behind `mackesd fleet-status`.
pub mod fleet_rollup;
pub mod passcode;
// EPIC-SEC-PASSCODE-CREDS (Q52) — encrypt the mesh passcode at rest
// via systemd-creds (TPM-or-host-key).
pub mod passcode_creds;
// PC-3 (2026-05-21) — peer-join handler: writes probe.json +
// spawns mde-peer-card on mesh peer-join events. Event-source
// integration into the mesh / topology layer is PC-3.a.
pub mod peer_join;
// v2.0.0 Phase 2.5 — path safety + allowed-roots resolver for the
// Send-To pipeline. Pure-fn validation; no async / DBus surface.
// v2.0.0 Phase 2.6 — Send-To operation orchestrator. Owns the
// validate → execute → verify state machine + the in-process
// audit-log + progress-event stream.
// v2.0.0 Phase 12.18 — HTTPS-tunneled fallback policy layer.
// Failure-window detector + activation state machine. Pure-fn.
pub mod https_fallback;
pub mod orchestrator;
pub mod path_safety;
pub mod policy;
// v2.0.0 Phase 3.5 — pre-flight validation for Send-To requests.
// Consumes path_safety + reports the 8 locked check rows the UI
// renders in the Send-To dialog.
pub mod preflight;
// EPIC-MESH-PROBE (MESH-PROBE-2) — the nmap probe engine (argv
// builders + `-oX` parser + scan runner). Reached from the
// `mackesd probe scan` CLI (bin/mackesd.rs); the scheduled worker +
// GFS write + Bus event are MESH-PROBE-4.
pub mod probe_nmap;
pub mod reconcile;
pub mod revisions;
pub mod secrets;
// v2.0.0 Phase 12.1.2 — service-layer facade traits reservation.
// When concrete cross-cutting trait surfaces actually ship (Phase F.x
// panel reads, Phase G.x fleet writes, Phase 2.x Send-To pipeline),
// `pub mod service;` + `crates/mackesd/src/service/` come back in
// one commit with real code. The empty-scaffold version was deleted
// 2026-05-22 per .claude/CLAUDE.md §0.12 (no stubs), matching the
// same-day deploy/ scaffold deletion.
pub mod settings;
pub mod store;
// MESH-A-4.a (v5.0.0) — surrounding-host taxonomy + classifier,
// consumed by the `mackesd classify-host` CLI + (later) the A-4.b
// collectors + A-4.c worker.
pub mod surrounding_hosts;
// v2.0.0 Phase 12.17 — STUN client for ICE candidate gathering. Gated
// behind `async-services` because it uses tokio UDP + tokio time.
pub mod bearer_ledger;
pub mod descriptors;
pub mod image_build;
pub mod image_catalog;
pub mod install_profiles;
pub mod leave;
pub mod lifecycle;
pub mod mesh_init;
pub mod mirrors;
pub mod node_key;
pub mod policy_engine;
pub mod remediation;
#[cfg(feature = "async-services")]
pub mod stun;
pub mod telemetry;
pub mod topology;
pub mod transport_probe;
// KDC2-1.11 — policy.toml loader. Lives in transport/ rather
// than at the top level so it can grow more files (the future
// KdcTls Transport impl glue, audit integration, etc.) without
// repeatedly editing lib.rs.
pub mod transport;
pub mod validation;
// VV-4 (v4.1.0) — voice-routing heuristic. Pure-function
// best_path + pick_relay over a list of connectivity candidates.
// Consumed by the future VV-2.a policy-lifecycle writer when it
// builds the per-peer `priority` weights baked into
// dispatcher.list rows.
pub mod voice;
// VOIP-4 (v5.0.0) — Vitelity-link RTT telemetry: measures the RTT to the
// Vitelity SIP edge + publishes `voip/link-rtt/<peer>`. Consumed by the
// `mackesd voip-rtt` CLI (VOIP-4.a) + the 60s broadcast worker (VOIP-4.b).
pub mod voip_rtt;
pub mod worker;
/// E1.2 — role-gated worker subsets (which workers `run_serve` spawns per role).
pub mod worker_role;

// v2.0.0 Phase A modules — async surface for the unified backend.
// Gated behind `async-services` so the legacy sync read-API still
// builds with only the original Phase 12 deps. Library consumers
// that need DBus / async workers enable the feature.
#[cfg(feature = "async-services")]
pub mod ipc;
#[cfg(feature = "async-services")]
pub mod workers;

/// Crate-wide error type. Every public function returns
/// `Result<T, mackesd_core::Error>` so callers don't have to import
/// half a dozen error types from internal modules.
pub type Error = anyhow::Error;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Default `SQLite` path inside `$MDE_HOME` (or fallback to the
/// legacy `$MACKESD_HOME`, or `/var/lib/mde/mded.db`).
///
/// v2.0.0 Phase 0.6 shim — reads `MDE_HOME` first; if unset, reads
/// the legacy `MACKESD_HOME` and logs a one-shot deprecation
/// warning to stderr. The fallback path falls through to the new
/// `/var/lib/mde/` location once both env vars are unset.
#[must_use]
pub fn default_db_path() -> std::path::PathBuf {
    if let Some(home) = env_with_legacy_fallback("MDE_HOME", "MACKESD_HOME") {
        return std::path::PathBuf::from(home).join("mded.db");
    }
    std::path::PathBuf::from("/var/lib/mde/mded.db")
}

/// Default MDE-Workgroup coordination root (formerly QNM-Shared).
/// Heartbeats + link telemetry land at
/// `<root>/<peer>/mackesd/{heartbeat,links}.json`; the leader lock
/// is `<root>/.mackesd-leader.lock`.
///
/// EPIC-RETIRE-QNM Phase C (2026-05-26, Q14 + Q77 of the 100-Q
/// tightening survey): env-var precedence is now `MDE_WORKGROUP_ROOT`
/// (canonical) > `QNM_SHARED_ROOT` (back-compat); both are read so
/// existing systemd drop-ins / shell profiles keep working through
/// the rename. The default path stays `~/QNM-Shared` (legacy installs
/// still use it; v5+ installs additionally have `~/.mde-mesh/`
/// gluster-mounted per GF-4.1 / Q21). The function name stays
/// `default_qnm_shared_root` for back-compat — symbol-level rename
/// is EPIC-RETIRE-QNM Phase B.
#[must_use]
pub fn default_qnm_shared_root() -> std::path::PathBuf {
    if let Ok(root) = std::env::var("MDE_WORKGROUP_ROOT") {
        return std::path::PathBuf::from(root);
    }
    if let Ok(root) = std::env::var("QNM_SHARED_ROOT") {
        return std::path::PathBuf::from(root);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join("QNM-Shared");
    }
    std::path::PathBuf::from("/var/lib/mackesd/qnm-shared")
}

/// v2.0.0 Phase 0.6 — env-var rename shim.
///
/// Reads `$new_name` first. If unset, reads `$legacy_name`; when
/// that's set, emits a one-shot deprecation warning naming both the
/// legacy variable and its successor so operators know to update
/// their systemd drop-ins / shell profiles. Returns `None` when
/// neither variable is set, leaving the caller to fall back to its
/// hardcoded default.
///
/// The deprecation log goes to stderr via `tracing::warn!` so it
/// lands in the journal alongside other mded output without
/// requiring a separate stream. The legacy fallback drops in v2.1
/// per the upgrade-path lock in
/// `docs/design/v2.0.0-mde-rebrand/identifiers.md`.
#[must_use]
pub fn env_with_legacy_fallback(new_name: &str, legacy_name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(new_name) {
        return Some(v);
    }
    match std::env::var(legacy_name) {
        Ok(v) => {
            // tracing isn't always initialized at lib load time
            // (e.g. for one-shot CLI calls); fall back to a direct
            // stderr write so the warning is visible regardless.
            tracing::warn!(
                legacy = legacy_name,
                replacement = new_name,
                "MDE rebrand: {legacy_name} is deprecated; \
                 switch to {new_name} (legacy fallback drops in v2.1)"
            );
            Some(v)
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod env_shim_tests {
    use super::env_with_legacy_fallback;

    /// Use a unique env-var name per test to avoid interference with
    /// parallel `cargo test` workers (which all share one process).
    fn unique_name(prefix: &str) -> (String, String) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        (
            format!("MDE_SHIM_TEST_{prefix}_NEW_{nonce}"),
            format!("MDE_SHIM_TEST_{prefix}_OLD_{nonce}"),
        )
    }

    #[test]
    fn prefers_new_when_both_are_set() {
        let (new, old) = unique_name("prefers");
        std::env::set_var(&new, "new-value");
        std::env::set_var(&old, "old-value");
        let got = env_with_legacy_fallback(&new, &old);
        assert_eq!(got.as_deref(), Some("new-value"));
        std::env::remove_var(&new);
        std::env::remove_var(&old);
    }

    #[test]
    fn falls_back_to_legacy_when_new_unset() {
        let (new, old) = unique_name("falls");
        std::env::remove_var(&new);
        std::env::set_var(&old, "legacy-value");
        let got = env_with_legacy_fallback(&new, &old);
        assert_eq!(got.as_deref(), Some("legacy-value"));
        std::env::remove_var(&old);
    }

    #[test]
    fn returns_none_when_neither_is_set() {
        let (new, old) = unique_name("none");
        std::env::remove_var(&new);
        std::env::remove_var(&old);
        assert!(env_with_legacy_fallback(&new, &old).is_none());
    }
}
