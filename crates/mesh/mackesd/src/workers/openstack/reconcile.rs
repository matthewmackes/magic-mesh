//! QC-2 — the reconcile core: desired services vs running containers →
//! converge, plus the `state/openstack/<node>` mirror fold.
//!
//! Everything here is pure over the two seams ([`FleetStateSource`] +
//! [`PodmanRunner`]): [`plan_converge`] is the no-I/O decision (start
//! missing / restart killed / stop extra), [`converge_cycle`] composes one
//! whole worker tick over injected seams (headless-testable, no tokio), and
//! [`OpenStackState`] is the honest mirror body — doctrine-gated, runtime-
//! unavailable, image-gated, and config-gated states are all first-class,
//! named rows, never fabricated successes (§7).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::workers::container::{Container, ContainerState};

use super::catalog::ServiceKind;
use super::fleet::{desired_services, FleetStateSource};
use super::podman::{config_rendered, KollaServiceSpec, PodmanRunner};

// ─────────────────────────── converge plan ───────────────────────────

/// The pure converge decision for one tick: what to start, restart, stop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConvergePlan {
    /// Desired services with no container at all → `podman run` (gated on
    /// image + rendered config before the runner is touched).
    pub start: Vec<ServiceKind>,
    /// Desired services whose container exists but isn't running (a killed/
    /// exited container) → `podman start` (the QC-2 "a killed container
    /// restarts" acceptance).
    pub restart: Vec<ServiceKind>,
    /// Catalog-managed container names present but not desired → stop +
    /// remove. An operator's unrelated container never appears here (only
    /// names that reverse-map through the catalog are ours to converge).
    pub stop: Vec<String>,
}

impl ConvergePlan {
    /// Whether the plan mutates anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start.is_empty() && self.restart.is_empty() && self.stop.is_empty()
    }
}

/// Fold the converge decision: `desired` vs the observed `podman ps` roster.
/// Deterministic (catalog order throughout); unmanaged containers are
/// invisible to it.
#[must_use]
pub fn plan_converge(desired: &BTreeSet<ServiceKind>, observed: &[Container]) -> ConvergePlan {
    // kind → state for the managed subset of the roster.
    let managed: BTreeMap<ServiceKind, ContainerState> = observed
        .iter()
        .filter_map(|c| ServiceKind::from_container_name(&c.name).map(|k| (k, c.state_kind())))
        .collect();
    let mut plan = ConvergePlan::default();
    for kind in desired {
        match managed.get(kind) {
            None => plan.start.push(*kind),
            Some(ContainerState::Running) => {}
            Some(_) => plan.restart.push(*kind),
        }
    }
    plan.stop = managed
        .keys()
        .filter(|k| !desired.contains(k))
        .map(|k| k.container_name().to_string())
        .collect();
    plan
}

// ─────────────────────────── mirror model ───────────────────────────

/// The doctrine leg of the mirror — what the fleet state said this tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DoctrineStatus {
    /// The cloud is declared; this node converges on its service set.
    Enabled {
        /// This node holds the leader lease (hosts `MariaDB` — Q15).
        leader: bool,
        /// The pinned Kolla release the doctrine names (Q69).
        kolla_release: String,
    },
    /// The fleet state explicitly declares no cloud — converge to zero
    /// managed services (the Q72 hard-cutover direction rides this too).
    Disabled,
    /// The doctrine couldn't be read — the typed reason (today: the QC-4
    /// record doesn't exist yet, so the read is integration-gated). No
    /// converge happens against an unknown desired state.
    Gated {
        /// The typed [`super::fleet::FleetStateError`] rendered for the
        /// mirror.
        reason: String,
    },
}

/// The container-runtime leg of the mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeStatus {
    /// Podman answered.
    Available,
    /// Podman is absent/unreachable — the typed reason; no mutation was
    /// attempted this tick.
    Unavailable {
        /// The typed [`super::podman::RunnerError`] rendered for the mirror.
        reason: String,
    },
}

/// One desired service's honest state in the mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ServiceStatus {
    /// The container is running (healthy at QC-2 depth — per-service API
    /// health checks land with QC-4's foundation work).
    Running,
    /// The container exists but isn't running (podman's raw state carried
    /// for the operator — a restart is planned next tick if still desired).
    NotRunning {
        /// The raw podman state (`exited`, `created`, `paused`, …).
        podman_state: String,
    },
    /// The start is honestly gated — image not mirrored yet (QC-3 lane) or
    /// Kolla config not rendered yet (QC-4 renderer). Named reason, no
    /// launch attempted.
    Gated {
        /// Exactly what's missing.
        reason: String,
    },
    /// A start/restart was attempted this tick and failed — the runner's
    /// typed error, also surfaced on the alert lane (→ chat).
    Failed {
        /// The failure detail.
        reason: String,
    },
    /// The service couldn't be observed (runtime unavailable / vanished
    /// mid-converge) — named, never guessed.
    Unknown {
        /// Why observation failed.
        reason: String,
    },
}

/// One mirror row: a desired service and its honest status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRow {
    /// The Kolla container name (`nova_api`, `mariadb`, …).
    pub service: String,
    /// The honest per-service state.
    pub status: ServiceStatus,
}

/// The body published to `state/openstack/<node>` — the same per-node mirror
/// idiom the storage (`state/storage/<node>`) and chat workers use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenStackState {
    /// Publishing node id.
    pub host: String,
    /// What the fleet doctrine said this tick.
    pub doctrine: DoctrineStatus,
    /// Whether podman answered this tick.
    pub runtime: RuntimeStatus,
    /// One row per desired service, catalog order. Empty when the doctrine
    /// is gated (desired unknown) or disabled (nothing desired).
    pub services: Vec<ServiceRow>,
    /// Catalog-managed containers observed but NOT desired (pre-stop
    /// leftovers / a stop that failed) — the honest "extra" set.
    pub extras: Vec<String>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

impl OpenStackState {
    /// Equality ignoring the publish timestamp — the worker's
    /// publish-on-change gate.
    #[must_use]
    pub fn same_ignoring_time(&self, other: &Self) -> bool {
        self.host == other.host
            && self.doctrine == other.doctrine
            && self.runtime == other.runtime
            && self.services == other.services
            && self.extras == other.extras
    }
}

// ─────────────────────────── one whole tick ───────────────────────────

/// Everything one converge tick produced: the mirror state, whether any
/// mutation ran, and the alert-lane lines for `[!]`-grade failures.
#[derive(Debug)]
pub struct CycleOutcome {
    /// The folded mirror body for `state/openstack/<node>`.
    pub state: OpenStackState,
    /// Whether any container mutation succeeded this tick.
    pub acted: bool,
    /// Alert-lane lines (the worker logs each on `mackesd::alert`, which the
    /// chat worker folds into the mesh chat — NOTIFY-CHAT lock 11).
    pub alerts: Vec<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Fold the mirror rows for `desired` against the (post-converge) roster +
/// the per-service gate/failure notes gathered during the tick.
fn fold_rows(
    desired: &BTreeSet<ServiceKind>,
    observed: &[Container],
    notes: &BTreeMap<ServiceKind, ServiceStatus>,
) -> Vec<ServiceRow> {
    let by_kind: BTreeMap<ServiceKind, &Container> = observed
        .iter()
        .filter_map(|c| ServiceKind::from_container_name(&c.name).map(|k| (k, c)))
        .collect();
    desired
        .iter()
        .map(|kind| {
            let status = notes.get(kind).cloned().unwrap_or_else(|| {
                by_kind.get(kind).map_or_else(
                    || ServiceStatus::Unknown {
                        reason: "container not observed after converge".to_string(),
                    },
                    |c| {
                        if c.state_kind() == ContainerState::Running {
                            ServiceStatus::Running
                        } else {
                            ServiceStatus::NotRunning {
                                podman_state: c.state.clone(),
                            }
                        }
                    },
                )
            });
            ServiceRow {
                service: kind.container_name().to_string(),
                status,
            }
        })
        .collect()
}

/// The managed-but-not-desired container names in `observed` (mirror
/// `extras`), name-sorted.
fn fold_extras(desired: &BTreeSet<ServiceKind>, observed: &[Container]) -> Vec<String> {
    let mut extras: Vec<String> = observed
        .iter()
        .filter_map(|c| ServiceKind::from_container_name(&c.name))
        .filter(|k| !desired.contains(k))
        .map(|k| k.container_name().to_string())
        .collect();
    extras.sort_unstable();
    extras.dedup();
    extras
}

/// The per-tick converge scratchpad the plan-application helpers write into.
struct CycleNotes {
    /// Per-service gate/failure notes for the mirror rows.
    notes: BTreeMap<ServiceKind, ServiceStatus>,
    /// Alert-lane lines for `[!]`-grade failures.
    alerts: Vec<String>,
    /// Whether any container mutation succeeded.
    acted: bool,
}

impl CycleNotes {
    const fn new() -> Self {
        Self {
            notes: BTreeMap::new(),
            alerts: Vec::new(),
            acted: false,
        }
    }

    fn fail(&mut self, kind: ServiceKind, verb: &str, detail: &str) {
        self.alerts.push(format!(
            "openstack: {verb} {} failed — {detail}",
            kind.container_name()
        ));
        self.notes.insert(
            kind,
            ServiceStatus::Failed {
                reason: detail.to_string(),
            },
        );
    }
}

/// The start leg for one missing service: gate on the mirrored image + the
/// rendered Kolla config, then `podman run`. Gates answer with named
/// reasons; a failed start is alerted + mirrored `Failed`.
fn converge_start(
    runner: &dyn PodmanRunner,
    config_root: &Path,
    release: &str,
    kind: ServiceKind,
    out: &mut CycleNotes,
) {
    let spec = KollaServiceSpec::for_service(kind, release, config_root);
    match runner.image_present(&spec.image) {
        Ok(false) => {
            out.notes.insert(
                kind,
                ServiceStatus::Gated {
                    reason: format!(
                        "kolla image {} not loaded locally — Kolla archives arrive \
                         operator-mirrored over the Syncthing lane + `podman load` \
                         (QC-3; design Q18: no registry pull on the airgapped fleet)",
                        spec.image
                    ),
                },
            );
        }
        Ok(true) if config_rendered(config_root, kind) => match runner.run_service(&spec) {
            Ok(()) => out.acted = true,
            Err(e) => out.fail(kind, "start", &e.to_string()),
        },
        Ok(true) => {
            out.notes.insert(
                kind,
                ServiceStatus::Gated {
                    reason: format!(
                        "kolla config not rendered at {} — the one-state → Kolla \
                         config renderer lands with QC-4 (foundation services)",
                        super::podman::kolla_config_dir(config_root, kind)
                            .join("config.json")
                            .display()
                    ),
                },
            );
        }
        Err(e) => out.fail(kind, "image check for", &e.to_string()),
    }
}

/// Apply a whole [`ConvergePlan`] over the runner: start missing (gated),
/// restart killed, stop + remove extras.
fn apply_plan(
    runner: &dyn PodmanRunner,
    config_root: &Path,
    release: &str,
    plan: ConvergePlan,
    out: &mut CycleNotes,
) {
    for kind in plan.start {
        converge_start(runner, config_root, release, kind, out);
    }
    for kind in plan.restart {
        match runner.start_existing(kind.container_name()) {
            Ok(()) => out.acted = true,
            Err(e) => out.fail(kind, "restart", &e.to_string()),
        }
    }
    for name in plan.stop {
        match runner.stop(&name).and_then(|()| runner.remove(&name)) {
            Ok(()) => out.acted = true,
            Err(e) => out
                .alerts
                .push(format!("openstack: stop/remove {name} failed — {e}")),
        }
    }
}

/// The runtime-unavailable outcome: nothing was mutated, every desired row
/// is honestly `Unknown`, and (when services were actually desired) the
/// failure rides the alert lane.
fn runtime_unavailable_outcome(
    host: &str,
    doctrine: DoctrineStatus,
    desired: &BTreeSet<ServiceKind>,
    reason: String,
    mut alerts: Vec<String>,
) -> CycleOutcome {
    if !desired.is_empty() {
        alerts.push(format!(
            "openstack: container runtime unavailable with {} service(s) desired — {reason}",
            desired.len()
        ));
    }
    let notes: BTreeMap<ServiceKind, ServiceStatus> = desired
        .iter()
        .map(|k| {
            (
                *k,
                ServiceStatus::Unknown {
                    reason: reason.clone(),
                },
            )
        })
        .collect();
    CycleOutcome {
        state: OpenStackState {
            host: host.to_string(),
            doctrine,
            runtime: RuntimeStatus::Unavailable { reason },
            services: fold_rows(desired, &[], &notes),
            extras: Vec::new(),
            published_at_ms: now_ms(),
        },
        acted: false,
        alerts,
    }
}

/// One whole worker tick over the injected seams.
///
/// Read the doctrine, observe the runtime, converge (start missing /
/// restart killed / stop extra — each start gated on the mirrored image +
/// the rendered Kolla config), and fold the honest mirror. Synchronous and
/// seam-pure — the worker drives it on a blocking task; tests drive it
/// directly with fakes.
///
/// Honesty invariants (§7):
/// - a gated/failed doctrine read converges NOTHING (never against an
///   unknown desired state) and mirrors the typed reason;
/// - a podman-less host mutates nothing and mirrors the typed reason;
/// - an absent image / unrendered config gates that service's start with a
///   named reason instead of launching a doomed container.
pub fn converge_cycle(
    fleet: &dyn FleetStateSource,
    runner: &dyn PodmanRunner,
    config_root: &Path,
    host: &str,
) -> CycleOutcome {
    // 1 — doctrine.
    let doctrine_read = fleet.read();
    let (doctrine, desired, release) = match &doctrine_read {
        Ok(view) if view.enabled => (
            DoctrineStatus::Enabled {
                leader: view.leader,
                kolla_release: view.kolla_release.clone(),
            },
            desired_services(view),
            view.kolla_release.clone(),
        ),
        Ok(_) => (DoctrineStatus::Disabled, BTreeSet::new(), String::new()),
        Err(e) => (
            DoctrineStatus::Gated {
                reason: e.to_string(),
            },
            BTreeSet::new(),
            String::new(),
        ),
    };
    let doctrine_known = doctrine_read.is_ok();

    // 2 — observe the runtime (even when the doctrine is gated: the mirror
    // still reports what's running here).
    let observed = match runner.list() {
        Ok(list) => list,
        Err(e) => {
            return runtime_unavailable_outcome(host, doctrine, &desired, e.to_string(), vec![])
        }
    };

    // 3 — converge (only against a KNOWN doctrine; the desired set is the
    // whole truth, so a Disabled doctrine stops every managed container).
    let mut out = CycleNotes::new();
    if doctrine_known {
        apply_plan(
            runner,
            config_root,
            &release,
            plan_converge(&desired, &observed),
            &mut out,
        );
    }
    let CycleNotes {
        notes,
        alerts,
        acted,
    } = out;

    // 4 — re-observe after mutations so the mirror carries post-converge
    // truth (best-effort: on a re-list failure the pre-converge roster
    // stands and the next tick corrects).
    let observed = if acted {
        runner.list().unwrap_or(observed)
    } else {
        observed
    };

    CycleOutcome {
        state: OpenStackState {
            host: host.to_string(),
            doctrine,
            runtime: RuntimeStatus::Available,
            services: fold_rows(&desired, &observed, &notes),
            extras: fold_extras(&desired, &observed),
            published_at_ms: now_ms(),
        },
        acted,
        alerts,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fleet::CloudDesired;
    use super::super::testkit::{FakeFleet, FakeRunner};
    use super::*;

    fn container(name: &str, state: &str) -> Container {
        Container {
            id: "-".into(),
            name: name.into(),
            image: "img".into(),
            state: state.into(),
        }
    }

    fn enabled_view(leader: bool) -> CloudDesired {
        CloudDesired {
            enabled: true,
            leader,
            kolla_release: "2024.1".into(),
        }
    }

    // ── plan_converge (the pure decision) ──

    #[test]
    fn plan_starts_everything_on_an_empty_node() {
        let desired = desired_services(&enabled_view(false));
        let plan = plan_converge(&desired, &[]);
        assert_eq!(plan.start.len(), desired.len());
        assert!(plan.restart.is_empty());
        assert!(plan.stop.is_empty());
    }

    #[test]
    fn plan_restarts_a_killed_container_and_keeps_running_ones() {
        // QC-2 acceptance: a killed container restarts.
        let desired: BTreeSet<ServiceKind> = [ServiceKind::Keystone, ServiceKind::NovaApi].into();
        let observed = vec![
            container("keystone", "running"),
            container("nova_api", "exited"),
        ];
        let plan = plan_converge(&desired, &observed);
        assert!(plan.start.is_empty());
        assert_eq!(plan.restart, vec![ServiceKind::NovaApi]);
        assert!(plan.stop.is_empty());
    }

    #[test]
    fn plan_stops_managed_extras_but_never_operator_containers() {
        // Only catalog-managed names converge; an operator's container is
        // invisible to the plan.
        let desired: BTreeSet<ServiceKind> = [ServiceKind::Keystone].into();
        let observed = vec![
            container("keystone", "running"),
            container("nova_api", "running"), // managed, not desired → stop
            container("mcnf-navidrome", "running"), // unmanaged → untouched
        ];
        let plan = plan_converge(&desired, &observed);
        assert!(plan.start.is_empty());
        assert!(plan.restart.is_empty());
        assert_eq!(plan.stop, vec!["nova_api".to_string()]);
    }

    #[test]
    fn empty_desired_stops_all_managed() {
        // A Disabled doctrine (or the Q72 cutover) converges to zero.
        let observed = vec![
            container("keystone", "running"),
            container("mariadb", "exited"),
        ];
        let plan = plan_converge(&BTreeSet::new(), &observed);
        // Catalog order (Mariadb is the first catalog entry), deterministic.
        assert_eq!(
            plan.stop,
            vec!["mariadb".to_string(), "keystone".to_string()]
        );
        assert!(!plan.is_empty());
        assert!(plan.start.is_empty());
        assert!(plan.restart.is_empty());
    }

    // ── converge_cycle over the seams ──

    #[test]
    fn gated_doctrine_converges_nothing_and_mirrors_the_reason() {
        let fleet = FakeFleet::gated("needs the QC-4 record");
        let runner = FakeRunner::new();
        runner.seed_container("keystone", ContainerState::Running);
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        // No mutations against an unknown desired state.
        assert!(runner.calls().iter().all(|c| c.starts_with("list")));
        assert!(!out.acted);
        let DoctrineStatus::Gated { reason } = &out.state.doctrine else {
            unreachable!("wrong doctrine: {:?}", out.state.doctrine);
        };
        assert!(reason.contains("QC-4"), "{reason}");
        assert!(out.state.services.is_empty(), "desired is unknown");
        // The observed managed container still shows, honestly, as an extra.
        assert_eq!(out.state.extras, vec!["keystone".to_string()]);
        assert_eq!(out.state.runtime, RuntimeStatus::Available);
    }

    #[test]
    fn podman_absent_mutates_nothing_and_is_typed_in_the_mirror() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::absent();
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        assert!(!out.acted);
        let RuntimeStatus::Unavailable { reason } = &out.state.runtime else {
            unreachable!("wrong runtime: {:?}", out.state.runtime);
        };
        assert!(reason.contains("podman absent"), "{reason}");
        // Every desired row is honestly Unknown (never guessed), and the
        // failure made the alert lane (→ chat).
        assert!(!out.state.services.is_empty());
        assert!(out
            .state
            .services
            .iter()
            .all(|r| matches!(&r.status, ServiceStatus::Unknown { reason } if reason.contains("podman absent"))));
        assert_eq!(out.alerts.len(), 1);
        assert!(
            out.alerts[0].contains("runtime unavailable"),
            "{}",
            out.alerts[0]
        );
    }

    #[test]
    fn missing_image_gates_the_start_with_the_airgap_reason() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::new(); // no images seeded
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        assert!(!out.acted, "no start may run without the image");
        assert!(runner.calls().iter().all(|c| !c.starts_with("run:")));
        let keystone = out
            .state
            .services
            .iter()
            .find(|r| r.service == "keystone")
            .expect("keystone row");
        let ServiceStatus::Gated { reason } = &keystone.status else {
            unreachable!("wrong status: {:?}", keystone.status);
        };
        assert!(reason.contains("QC-3"), "{reason}");
        assert!(
            reason.contains("quay.io/openstack.kolla/keystone:2024.1"),
            "{reason}"
        );
    }

    #[test]
    fn unrendered_config_gates_the_start_with_the_qc4_reason() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::new();
        runner.seed_image(&ServiceKind::Keystone.image_ref("2024.1"));
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        let keystone = out
            .state
            .services
            .iter()
            .find(|r| r.service == "keystone")
            .expect("keystone row");
        let ServiceStatus::Gated { reason } = &keystone.status else {
            unreachable!("wrong status: {:?}", keystone.status);
        };
        assert!(reason.contains("QC-4"), "{reason}");
        assert!(reason.contains("keystone"), "{reason}");
        assert!(runner.calls().iter().all(|c| !c.starts_with("run:")));
    }

    #[test]
    fn image_and_config_present_starts_the_service() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::new();
        let dir = tempfile::tempdir().unwrap();
        // Render every desired service's config + mirror every image so the
        // whole set starts.
        for kind in desired_services(&enabled_view(false)) {
            runner.seed_image(&kind.image_ref("2024.1"));
            let d = dir.path().join(kind.container_name());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("config.json"), "{}").unwrap();
        }
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        assert!(out.acted);
        assert!(out.alerts.is_empty(), "{:?}", out.alerts);
        assert!(runner.calls().iter().any(|c| c == "run:keystone"));
        // Post-converge re-observation shows the rows Running.
        assert!(
            out.state
                .services
                .iter()
                .all(|r| r.status == ServiceStatus::Running),
            "{:?}",
            out.state.services
        );
        assert!(out.state.extras.is_empty());
    }

    #[test]
    fn killed_container_restarts_and_extras_stop() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::new();
        // Everything desired already exists; nova_api was killed; mariadb is
        // an extra (this node lost the leader lease).
        for kind in desired_services(&enabled_view(false)) {
            runner.seed_container(kind.container_name(), ContainerState::Running);
        }
        runner.seed_container("nova_api", ContainerState::Exited);
        runner.seed_container("mariadb", ContainerState::Running);
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        assert!(out.acted);
        let calls = runner.calls();
        assert!(calls.iter().any(|c| c == "start:nova_api"), "{calls:?}");
        assert!(calls.iter().any(|c| c == "stop:mariadb"), "{calls:?}");
        assert!(calls.iter().any(|c| c == "remove:mariadb"), "{calls:?}");
        // Post-converge: all rows Running, no extras left.
        assert!(out
            .state
            .services
            .iter()
            .all(|r| r.status == ServiceStatus::Running));
        assert!(out.state.extras.is_empty());
    }

    #[test]
    fn start_failure_is_alerted_and_mirrored_failed() {
        let fleet = FakeFleet::fixed(enabled_view(false));
        let runner = FakeRunner::new();
        runner.fail_next_run("keystone", "exit 125: oci runtime error");
        for kind in desired_services(&enabled_view(false)) {
            runner.seed_image(&kind.image_ref("2024.1"));
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("keystone");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{}").unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        let keystone = out
            .state
            .services
            .iter()
            .find(|r| r.service == "keystone")
            .expect("keystone row");
        assert!(
            matches!(&keystone.status, ServiceStatus::Failed { reason } if reason.contains("oci runtime error")),
            "{:?}",
            keystone.status
        );
        assert!(
            out.alerts
                .iter()
                .any(|a| a.contains("start keystone failed")),
            "{:?}",
            out.alerts
        );
    }

    #[test]
    fn disabled_doctrine_stops_the_managed_world() {
        let fleet = FakeFleet::fixed(CloudDesired {
            enabled: false,
            leader: true,
            kolla_release: "2024.1".into(),
        });
        let runner = FakeRunner::new();
        runner.seed_container("keystone", ContainerState::Running);
        runner.seed_container("mcnf-navidrome", ContainerState::Running); // unmanaged
        let dir = tempfile::tempdir().unwrap();
        let out = converge_cycle(&fleet, &runner, dir.path(), "node-a");
        assert!(out.acted);
        assert_eq!(out.state.doctrine, DoctrineStatus::Disabled);
        let calls = runner.calls();
        assert!(calls.iter().any(|c| c == "stop:keystone"), "{calls:?}");
        assert!(
            calls
                .iter()
                .all(|c| !c.contains("mcnf-navidrome") || c.starts_with("list")),
            "operator containers are never touched: {calls:?}"
        );
        assert!(out.state.services.is_empty());
        assert!(out.state.extras.is_empty(), "stopped + removed");
    }

    // ── mirror body ──

    #[test]
    fn state_round_trips_json_and_change_gate_ignores_time() {
        let state = OpenStackState {
            host: "node-a".into(),
            doctrine: DoctrineStatus::Enabled {
                leader: true,
                kolla_release: "2024.1".into(),
            },
            runtime: RuntimeStatus::Available,
            services: vec![ServiceRow {
                service: "keystone".into(),
                status: ServiceStatus::Running,
            }],
            extras: vec![],
            published_at_ms: 42,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: OpenStackState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, state);
        // Same content, different stamp → not a change.
        let mut later = state.clone();
        later.published_at_ms = 43;
        assert!(state.same_ignoring_time(&later));
        // A row change IS a change.
        later.services[0].status = ServiceStatus::NotRunning {
            podman_state: "exited".into(),
        };
        assert!(!state.same_ignoring_time(&later));
    }
}
