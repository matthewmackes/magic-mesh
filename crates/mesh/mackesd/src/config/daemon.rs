//! E1.3 #3 — the system daemon config `mackesd` reads at startup:
//! `/etc/mackesd/mackesd.toml`.
//!
//! Unlike the per-tag manifests ([`super::tag_manifest`], user-scoped
//! under `~/.config/mde/tags/`), this is the **system** daemon config:
//! one root-owned file read once when the `mackesd` worker section
//! starts. It carries the daemon-wide cadence knobs an operator tunes
//! per deployment role — a Lighthouse relay can run a slower heartbeat
//! than a busy Workstation without a code change.
//!
//! ## Fields
//!
//! - `heartbeat_interval_secs` — how often the heartbeat worker writes
//!   this peer's liveness row to the mesh-FS (default 10 s, the 12.3.3
//!   lock — see [`crate::telemetry::HEARTBEAT_INTERVAL_S`]). Lower =
//!   fresher peer health at a higher write cost.
//! - `mesh_latency_sweep_secs` — how often the mesh-latency worker pings
//!   every peer and refreshes the link-sample cache (default 30 s,
//!   mirrors `workers::mesh_latency::DEFAULT_SWEEP_INTERVAL`).
//!
//! ## Fail-open contract
//!
//! A missing file → every default (the shipped behavior, so an
//! un-templated box behaves exactly as before this loader existed). A
//! malformed / unreadable file → every default **plus a logged
//! warning**; `mackesd` never refuses to boot over an operator typo in
//! its config. A pathological value (e.g. `0`) clamps to
//! [`MackesdConfig::MIN_INTERVAL_SECS`] at the accessor so no knob can
//! turn a worker into a 0 s busy-loop.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default mesh-latency sweep cadence, in seconds.
///
/// Mirrors `workers::mesh_latency::DEFAULT_SWEEP_INTERVAL` (which is gated
/// behind `async-services`, so it can't be referenced from this
/// always-compiled module directly — a `#[cfg(feature = "async-services")]`
/// test below pins the two together so they can't silently drift).
pub const DEFAULT_MESH_LATENCY_SWEEP_SECS: u64 = 30;

/// The daemon config families a node operator can tune.
///
/// Every field has a serde default (struct-level `#[serde(default)]`), so a
/// partial or empty file parses cleanly and falls back to the locked
/// defaults. Unknown fields are ignored (forward-compat: an older `mackesd`
/// reading a newer template still boots).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MackesdConfig {
    /// Heartbeat-worker write cadence, in seconds. Default
    /// [`crate::telemetry::HEARTBEAT_INTERVAL_S`].
    pub heartbeat_interval_secs: u64,
    /// Mesh-latency-worker sweep cadence, in seconds. Default
    /// [`DEFAULT_MESH_LATENCY_SWEEP_SECS`].
    pub mesh_latency_sweep_secs: u64,
    /// EFF-25 — 12.6.4 alert hooks. Each entry fires a shell command
    /// with the event JSON on stdin when a matching audit event lands:
    ///
    /// ```toml
    /// [[alert_hooks]]
    /// kind = "reconcile"            # omit to fire on every kind
    /// command = ["/usr/local/bin/notify-ops", "--channel", "mesh"]
    /// ```
    ///
    /// No webhooks by design — operators wire `curl` themselves.
    /// Default: empty (no hooks fire).
    pub alert_hooks: Vec<AlertHookEntry>,

    /// mackesd-03 — master switch for the reconcile worker's
    /// safe-repair apply step. When `false`, the reconciler is
    /// observe-only: drift is still detected + surfaced (audit log +
    /// operator inbox) but no repair action ever fires. Default
    /// [`crate::worker::DEFAULT_AUTO_REPAIR`] (`true`), so a box with
    /// no config template keeps the locked opt-out semantics. An
    /// operator flips this to `false` to fall the fleet back to
    /// observe-only without a code change.
    pub auto_repair: bool,

    /// mackesd-03 — blast-radius cap: the maximum number of safe
    /// repair *actions* (overlay re-probes) the reconcile worker takes
    /// in a single tick. Overflow drift is deferred to the next tick
    /// (audit-logged) so a mass-drift event can't trigger a fleet-wide
    /// probe storm in one pass. Default
    /// [`crate::worker::DEFAULT_MAX_REPAIRS_PER_TICK`]. `0` takes no
    /// repair actions (equivalent to observe-only for that tick).
    pub max_repairs_per_tick: usize,
}

/// One configured 12.6.4 alert hook (the TOML shape of
/// [`crate::events::AlertHook`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AlertHookEntry {
    /// Event kind to match (snake_case, e.g. `"reconcile"`,
    /// `"config_change"`, `"auth"`, `"lifecycle"`, `"admin_action"`).
    /// `None`/omitted fires on every kind.
    pub kind: Option<String>,
    /// Executable + args to spawn. Empty commands are skipped.
    pub command: Vec<String>,
}

impl Default for MackesdConfig {
    fn default() -> Self {
        Self {
            // Single source of truth for the heartbeat default — the
            // 12.3.3 lock lives on the telemetry const, not duplicated.
            heartbeat_interval_secs: crate::telemetry::HEARTBEAT_INTERVAL_S,
            mesh_latency_sweep_secs: DEFAULT_MESH_LATENCY_SWEEP_SECS,
            alert_hooks: Vec::new(),
            // mackesd-03 — single source of truth for the repair
            // defaults is the worker module (not duplicated here).
            auto_repair: crate::worker::DEFAULT_AUTO_REPAIR,
            max_repairs_per_tick: crate::worker::DEFAULT_MAX_REPAIRS_PER_TICK,
        }
    }
}

impl MackesdConfig {
    /// Floor every cadence accessor clamps to, so a `0` (or any sub-second
    /// value) in the template can't spin a worker into a busy-loop.
    pub const MIN_INTERVAL_SECS: u64 = 1;

    /// The heartbeat write cadence as a [`Duration`], clamped to
    /// [`Self::MIN_INTERVAL_SECS`].
    #[must_use]
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval_secs.max(Self::MIN_INTERVAL_SECS))
    }

    /// The mesh-latency sweep cadence as a [`Duration`], clamped to
    /// [`Self::MIN_INTERVAL_SECS`].
    #[must_use]
    pub fn mesh_latency_sweep(&self) -> Duration {
        Duration::from_secs(self.mesh_latency_sweep_secs.max(Self::MIN_INTERVAL_SECS))
    }

    /// EFF-25 — resolve the configured [`AlertHookEntry`]s into the
    /// [`crate::events::AlertHook`]s `dispatch_alerts` consumes. An
    /// unrecognized `kind` string drops that hook with a warn (never
    /// silently widen a typo'd kind to fire-on-everything); an empty
    /// `command` is skipped.
    #[must_use]
    pub fn alert_hooks(&self) -> Vec<crate::events::AlertHook> {
        self.alert_hooks
            .iter()
            .filter(|e| !e.command.is_empty())
            .filter_map(|e| {
                let for_kind = match &e.kind {
                    None => None,
                    Some(s) => {
                        match serde_json::from_value::<crate::events::EventKind>(
                            serde_json::Value::String(s.clone()),
                        ) {
                            Ok(k) => Some(k),
                            Err(_) => {
                                tracing::warn!(
                                    kind = %s,
                                    "alert_hooks: unknown event kind; hook dropped",
                                );
                                return None;
                            }
                        }
                    }
                };
                Some(crate::events::AlertHook {
                    for_kind,
                    command: e.command.clone(),
                })
            })
            .collect()
    }
}

/// The canonical system config path: `/etc/mackesd/mackesd.toml`. The
/// E8 `mde-core` RPM ships the commented default template here; an
/// operator edits it + restarts `mackesd.service` to apply.
#[must_use]
pub fn default_config_path() -> PathBuf {
    PathBuf::from("/etc/mackesd/mackesd.toml")
}

/// Load the daemon config from [`default_config_path`], fail-open. See
/// the module-level fail-open contract.
#[must_use]
pub fn load() -> MackesdConfig {
    load_from(&default_config_path())
}

/// Load the daemon config from `path`, fail-open.
///
/// A missing file → defaults silently (the un-templated box); a malformed
/// / unreadable file → defaults + a logged warning. Never returns an error
/// — the daemon must boot regardless of operator config typos.
#[must_use]
pub fn load_from(path: &Path) -> MackesdConfig {
    match std::fs::read_to_string(path) {
        Ok(raw) => match parse(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "mackesd config: malformed, falling back to defaults",
                );
                MackesdConfig::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => MackesdConfig::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mackesd config: unreadable, falling back to defaults",
            );
            MackesdConfig::default()
        }
    }
}

/// Strict parse of a config-file body. Public for tests; `mackesd`'s
/// startup uses [`load`] / [`load_from`] (which swallow this error).
///
/// # Errors
/// Returns the TOML parse error message on malformed input.
pub fn parse(raw: &str) -> Result<MackesdConfig, String> {
    toml::from_str(raw).map_err(|e| format!("parse: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_locked_cadences() {
        let d = MackesdConfig::default();
        // The heartbeat default is the 12.3.3 lock, sourced from the
        // telemetry const (not duplicated here).
        assert_eq!(
            d.heartbeat_interval_secs,
            crate::telemetry::HEARTBEAT_INTERVAL_S
        );
        assert_eq!(d.heartbeat_interval_secs, 10);
        assert_eq!(d.mesh_latency_sweep_secs, 30);
    }

    #[test]
    fn parse_full_config_reads_both_knobs() {
        let cfg = parse(
            r#"
heartbeat_interval_secs = 30
mesh_latency_sweep_secs = 120
"#,
        )
        .unwrap();
        assert_eq!(cfg.heartbeat_interval_secs, 30);
        assert_eq!(cfg.mesh_latency_sweep_secs, 120);
        assert_eq!(cfg.heartbeat_interval(), Duration::from_secs(30));
        assert_eq!(cfg.mesh_latency_sweep(), Duration::from_secs(120));
    }

    #[test]
    fn parse_partial_config_fills_missing_from_defaults() {
        // Only one knob set — the other falls back to its locked default.
        let cfg = parse("heartbeat_interval_secs = 5").unwrap();
        assert_eq!(cfg.heartbeat_interval_secs, 5);
        assert_eq!(cfg.mesh_latency_sweep_secs, 30);
    }

    #[test]
    fn parse_empty_config_is_all_defaults() {
        assert_eq!(parse("").unwrap(), MackesdConfig::default());
    }

    #[test]
    fn parse_ignores_unknown_fields() {
        // Forward-compat: a newer template's extra key doesn't fail an
        // older mackesd — it's ignored, known fields still apply.
        let cfg = parse(
            r#"
heartbeat_interval_secs = 7
some_future_knob = "ignored"
"#,
        )
        .unwrap();
        assert_eq!(cfg.heartbeat_interval_secs, 7);
    }

    #[test]
    fn accessor_clamps_zero_to_the_floor() {
        // A 0 in the template must not become a 0 s busy-loop.
        let cfg = MackesdConfig {
            heartbeat_interval_secs: 0,
            mesh_latency_sweep_secs: 0,
            ..MackesdConfig::default()
        };
        assert_eq!(
            cfg.heartbeat_interval(),
            Duration::from_secs(MackesdConfig::MIN_INTERVAL_SECS)
        );
        assert_eq!(
            cfg.mesh_latency_sweep(),
            Duration::from_secs(MackesdConfig::MIN_INTERVAL_SECS)
        );
    }

    #[test]
    fn load_missing_file_is_defaults() {
        let cfg = load_from(Path::new("/nonexistent/etc/mackesd/mackesd.toml"));
        assert_eq!(cfg, MackesdConfig::default());
    }

    #[test]
    fn load_malformed_file_fails_open_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mackesd.toml");
        std::fs::write(&path, "heartbeat_interval_secs = not a number =\n").unwrap();
        // Fail-open: the daemon still boots with the defaults.
        assert_eq!(load_from(&path), MackesdConfig::default());
    }

    #[test]
    fn alert_hooks_parse_and_resolve() {
        // EFF-25 — [[alert_hooks]] TOML → events::AlertHook, with kind
        // matching, typo'd-kind drop, and empty-command skip.
        let cfg = parse(
            "[[alert_hooks]]\n\
             kind = \"reconcile\"\n\
             command = [\"/usr/bin/notify-ops\", \"--mesh\"]\n\
             [[alert_hooks]]\n\
             command = [\"/usr/bin/log-all\"]\n\
             [[alert_hooks]]\n\
             kind = \"not_a_kind\"\n\
             command = [\"/usr/bin/never\"]\n\
             [[alert_hooks]]\n\
             kind = \"auth\"\n\
             command = []\n",
        )
        .expect("parses");
        assert_eq!(cfg.alert_hooks.len(), 4);
        let hooks = cfg.alert_hooks();
        // typo'd kind dropped (never widened to fire-on-everything),
        // empty command skipped.
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].for_kind, Some(crate::events::EventKind::Reconcile));
        assert_eq!(hooks[0].command, vec!["/usr/bin/notify-ops", "--mesh"]);
        assert_eq!(hooks[1].for_kind, None, "omitted kind fires on every event");
    }

    #[test]
    fn alert_hooks_default_empty_keeps_dispatch_a_noop() {
        assert!(MackesdConfig::default().alert_hooks().is_empty());
    }

    #[test]
    fn load_well_formed_file_round_trips_off_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mackesd.toml");
        std::fs::write(
            &path,
            "heartbeat_interval_secs = 15\nmesh_latency_sweep_secs = 45\n",
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.heartbeat_interval_secs, 15);
        assert_eq!(cfg.mesh_latency_sweep_secs, 45);
    }

    #[test]
    fn round_trip_serialize_then_parse() {
        let cfg = MackesdConfig {
            heartbeat_interval_secs: 20,
            mesh_latency_sweep_secs: 90,
            ..MackesdConfig::default()
        };
        let body = toml::to_string(&cfg).unwrap();
        assert_eq!(parse(&body).unwrap(), cfg);
    }

    #[test]
    fn default_path_is_etc_mackesd() {
        assert_eq!(
            default_config_path(),
            PathBuf::from("/etc/mackesd/mackesd.toml")
        );
    }

    #[test]
    fn repair_defaults_track_the_worker_consts() {
        // mackesd-03 — the repair knobs default to the worker's locked
        // consts (single source of truth), so an un-templated box keeps
        // auto-repair ON with the default per-tick cap.
        let d = MackesdConfig::default();
        assert_eq!(d.auto_repair, crate::worker::DEFAULT_AUTO_REPAIR);
        assert_eq!(
            d.max_repairs_per_tick,
            crate::worker::DEFAULT_MAX_REPAIRS_PER_TICK
        );
        assert!(d.auto_repair, "auto-repair is opt-out by default");
    }

    #[test]
    fn repair_knobs_parse_from_toml() {
        // mackesd-03 — an operator turns auto-repair OFF (observe-only)
        // and/or retunes the blast-radius cap without a code change.
        let cfg = parse("auto_repair = false\nmax_repairs_per_tick = 4\n").unwrap();
        assert!(!cfg.auto_repair);
        assert_eq!(cfg.max_repairs_per_tick, 4);
        // Unset knobs still fall back to their locked defaults.
        assert_eq!(cfg.heartbeat_interval_secs, 10);
    }

    #[test]
    fn repair_knobs_partial_config_keeps_other_defaults() {
        // Only auto_repair set → the cap stays at its default.
        let cfg = parse("auto_repair = false\n").unwrap();
        assert!(!cfg.auto_repair);
        assert_eq!(
            cfg.max_repairs_per_tick,
            crate::worker::DEFAULT_MAX_REPAIRS_PER_TICK
        );
    }

    // The local mesh-latency default must track the worker's own const —
    // gated on the feature that compiles that worker so the two can't
    // silently drift (§2.2 ground-truth-pinned-in-tests).
    #[cfg(feature = "async-services")]
    #[test]
    fn mesh_latency_default_tracks_the_worker_const() {
        assert_eq!(
            DEFAULT_MESH_LATENCY_SWEEP_SECS,
            crate::workers::mesh_latency::DEFAULT_SWEEP_INTERVAL.as_secs()
        );
    }
}
