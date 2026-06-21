//! `mackesd_core` — the Mesh control-plane library behind the
//! `mackesd` daemon/CLI binary (`src/bin/mackesd.rs`) and its
//! workers. Surfaces reach it over the Mackes Bus
//! (`action/<domain>/<verb>`) and the replicated QNM-Shared volume —
//! no MDE-private D-Bus, no central server (§1/§2/§6).
//!
//! The durable tracker is `docs/WORKLIST.md`; modules land only when
//! runtime-reachable per the §7 Definition of Done.

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
pub mod lighthouse_addr;
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
// ONBOARD-2 — the lighthouse network `/enroll` endpoint core (self-
// signed pinned identity + minimal HTTP framing + the pure POST
// handler). Drives MESH-1's fix: network bootstrap for NAT'd peers.
// The rustls listener that serves it is workers::nebula_enroll_listener.
#[cfg(feature = "async-services")]
pub mod nebula_enroll_endpoint;
// ONBOARD-3 — the peer-side fingerprint-pinned network-enroll client:
// the rustls PinnedCertVerifier (fail-closed) + the POST-CSR-over-TLS
// flow + materializing /etc/nebula from the returned bundle. The peer
// half of the MESH-1 fix.
#[cfg(feature = "async-services")]
pub mod nebula_enroll_client;
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
// AUD3 S-3 (2026-06-12): `peer_join` (PC-3) REMOVED — it spawned the
// `mde-peer-card` modal, a binary deleted in the E11 pivot, so wiring
// it into the enrollment loop (PC-3.a) would have wired a dead spawn.
// Peer-arrival UX lives in the Workbench PEERS/Directory surfaces now.
// v2.0.0 Phase 2.5 — path safety + allowed-roots resolver for the
// Send-To pipeline. Pure-fn validation; no async / DBus surface.
// (The Phase 2.6 Send-To operation orchestrator was removed
// 2026-06-13, AUD6-2 — 522 lines with zero production callers.
// Git history keeps it for the epic that wires the real
// transfer engine into `ipc/files.rs`.)
// v2.0.0 Phase 12.18 — HTTPS-tunneled fallback policy layer.
// Failure-window detector + activation state machine. Pure-fn.
pub mod https_fallback;
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
/// BULLETPROOF-2 — minimal sd_notify (systemd readiness + watchdog), no dep.
pub mod sd_notify;
pub mod settings;
/// SETUP-7 — the `/etc/mackesd/site.yml` convergence playbook emitter.
pub mod site_yml;
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
// NET-INTROSPECT (PD-6/PD-7) — direct-vs-relay tunnel classification via
// Nebula's loopback debug SSH server. Consumed by nebula_supervisor (renders
// the sshd block) + mesh_latency (queries + joins the hostmap).
pub mod nebula_admin;
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
// VPN-GW-2 — encrypted, leader-managed VPN tunnel secrets. Pure seal/unseal of
// a `mackes_mesh_types::vpn::TunnelSecret` under the mesh CA key (reusing the
// `ca::backup` Argon2id/XChaCha20-Poly1305 envelope) + the materialize path that
// lays the decrypted config down for VPN-GW-1's `wg-quick`/`openvpn` bring-up.
pub mod vpn_secret;
// VPN-GW-5 — local WireGuard x25519 keypair generation (dalek, no OpenSSL §3);
// the §3 crypto boundary the pure `vpn_provider` adapters mint keys through.
pub mod vpn_keypair;
// Fire-and-forget subprocess reaping — prevents the `mde-bus publish` zombie
// pile (the live-mesh wedge). Non-gated so the always-compiled `ca::revoke` +
// `voip_rtt` callers can use it in a no-default-features build.
pub mod proc_reap;
pub mod worker;
/// E1.2 — role-gated worker subsets (which workers `run_serve` spawns per role).
pub mod worker_role;

// v2.0.0 Phase A modules — async surface for the unified backend.
// Gated behind `async-services` so the legacy sync read-API still
// builds with only the original Phase 12 deps. Library consumers
// that need DBus / async workers enable the feature.
#[cfg(feature = "async-services")]
pub mod ipc;
/// SUBSTRATE-V2 — the etcd (coordination) + Syncthing (files) substrate clients.
#[cfg(feature = "async-services")]
pub mod substrate;
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
    // Single-sourced in `mackes-mesh-types` so `mde-workbench` resolves
    // byte-for-byte the same mount (EPIC-RETIRE-QNM split-brain fix,
    // 2026-06-14): the workbench panels used to fall back to a phantom
    // `/mnt/mesh-storage` while this read `~/QNM-Shared`, so the GUI
    // showed "mesh-storage not mounted" against a healthy 4-node mesh.
    mackes_mesh_types::peers::default_workgroup_root()
}

/// The canonical deployed QNM-Shared mount point — the only path where an
/// unmounted-but-written-to directory poisons LizardFS (see
/// [`shared_root_writable`]).
pub const CANONICAL_QNM_MOUNT: &str = "/mnt/mesh-storage";

/// ONBOARD-6 / AUDIT-MESH-15 guard: is it SAFE to write under `root`?
///
/// The one unsafe case is the canonical shared mount
/// ([`CANONICAL_QNM_MOUNT`]) when it is **not actually a FUSE mount** — writing
/// there (the leader lock, peer heartbeats, the directory) fills the mountpoint
/// so LizardFS can never `mfsmount` over it again ("mountpoint is not empty"),
/// and the daemon then silently reads a stale local 1-node dir. Any other root
/// (a dev `~/QNM-Shared`, a tempdir, or the mount when genuinely mounted) is
/// always writable, so dev/test paths are unaffected.
#[must_use]
pub fn shared_root_writable(root: &std::path::Path) -> bool {
    if root != std::path::Path::new(CANONICAL_QNM_MOUNT) {
        return true;
    }
    // Canonical mount path: writable only when it's a real mount.
    std::fs::read_to_string("/proc/mounts").is_ok_and(|c| {
        c.lines()
            .any(|l| l.split_whitespace().nth(1) == Some(CANONICAL_QNM_MOUNT))
    })
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
mod shared_root_tests {
    use super::shared_root_writable;
    use std::path::Path;

    #[test]
    fn non_canonical_roots_are_always_writable() {
        // Dev/test paths (tempdirs, ~/QNM-Shared) are never the poison case.
        assert!(shared_root_writable(Path::new("/home/mm/QNM-Shared")));
        assert!(shared_root_writable(Path::new("/tmp/anything")));
        let tmp = tempfile::tempdir().unwrap();
        assert!(shared_root_writable(tmp.path()));
    }

    #[test]
    fn canonical_mount_writable_iff_actually_mounted() {
        // The canonical path is writable exactly when /proc/mounts lists it —
        // so on a machine where it isn't mounted, the guard blocks the write.
        let listed = std::fs::read_to_string("/proc/mounts")
            .map(|c| {
                c.lines()
                    .any(|l| l.split_whitespace().nth(1) == Some("/mnt/mesh-storage"))
            })
            .unwrap_or(false);
        assert_eq!(shared_root_writable(Path::new("/mnt/mesh-storage")), listed);
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
