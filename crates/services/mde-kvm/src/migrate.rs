//! Live VM migration (E12-10, lock 48) over cloud-hypervisor's migration API.
//!
//! cloud-hypervisor migrates a running guest VMM-to-VMM: the **target** VMM
//! (started empty, `cloud-hypervisor --api-socket …`) is told to listen with
//! `PUT vm.receive-migration` (`ReceiveMigrationData.receiver_url`), then the
//! **source** VMM is told to stream the guest with `PUT vm.send-migration`
//! (`SendMigrationData.destination_url`); on success the guest runs on the
//! target and the source VMM ends. Both URLs are `unix:<path>` (same-host) or
//! `tcp:<ip>:<port>` (cross-host).
//!
//! This module follows the crate's pattern — a **pure planning core** plus a
//! seam-driven executor:
//!
//! - [`plan_migration`] turns a [`MigrateRequest`] into a [`MigrationPlan`]:
//!   the ordered typed steps [`MigrationStep::PrepareReceive`] →
//!   [`MigrationStep::SendMigration`] → [`MigrationStep::VerifyRunning`], with
//!   the exact request bodies as pure, unit-tested JSON.
//! - [`run_migration`] drives the plan through two [`Vm`] handles (target +
//!   source), each behind the existing [`ChTransport`](crate::ChTransport)
//!   seam — so ordering and every error path are unit-tested with recording
//!   mocks. Each failure is a typed [`MigrateError`] naming the step that
//!   died and the underlying [`KvmError`] (which, live, names exactly what is
//!   missing — e.g. `Connect(<socket>, …)` when a VMM api-socket isn't
//!   there).
//!
//! ## Over the mesh (Nebula)
//!
//! Cross-host migration rides the **overlay**: the target host's
//! vm-lifecycle worker starts the empty VMM and calls `PrepareReceive` with a
//! `tcp:` listen URL; the source host's worker calls `SendMigration` with
//! `destination_url = tcp:<target's Nebula overlay IP>:<port>`
//! ([`MigrateRequest::over_mesh`]). The migration stream thus flows inside
//! the encrypted mesh like any other peer traffic — no extra firewall
//! surface, and it works across NAT via the normal Nebula hole-punch/relay
//! path. mackesd coordinates the two halves (§9 typed verbs, no push-SSH);
//! this crate supplies the per-VMM steps + the same-host executor.
//!
//! ## Integration-gated: the live migration
//!
//! Actually moving a guest needs two live VMMs with a booted guest and a
//! reachable network path — none on the build farm — so the live run is the
//! `#[ignore]`d gate in `tests/live_boot.rs` (`MDE_KVM_TEST_MIGRATE_*`). The
//! executor itself is fully implemented; a missing prerequisite surfaces as
//! the typed step error, never a fake success (§7).

use std::fmt;
use std::path::PathBuf;

use serde_json::{json, Value};
use thiserror::Error;

use crate::transport::ChTransport;
use crate::vm::Vm;
use crate::KvmError;

/// The conventional cross-host migration listen port (the cloud-hypervisor
/// docs' example port). One migration per port at a time; a fan-out picks
/// distinct ports.
pub const DEFAULT_MIGRATION_PORT: u16 = 4444;

/// A migration failure, typed per step.
///
/// The caller knows exactly which side and verb died — and, live, what was
/// missing: a `Connect` source names the absent VMM api-socket; a `tcp:`
/// dial failure names the network path.
#[derive(Debug, Error)]
pub enum MigrateError {
    /// A migration step failed on its VMM.
    #[error("migration of '{vm}' failed at {step} (on the {side} VMM): {source}")]
    Step {
        /// The VM being migrated.
        vm: String,
        /// The step that failed.
        step: MigrationStep,
        /// Which VMM the step was talking to.
        side: MigrationSide,
        /// The underlying API/transport failure.
        #[source]
        source: KvmError,
    },
    /// Every API call succeeded but the target VMM does not report the guest
    /// `Running` — the migration did not actually deliver a live guest, and
    /// saying otherwise would be a fake success.
    #[error(
        "migration of '{vm}' completed its API calls but the target reports state \
         '{state}', not Running — do not tear down the source until this is resolved"
    )]
    NotRunningOnTarget {
        /// The VM that was migrated.
        vm: String,
        /// The state the target VMM reports.
        state: String,
    },
}

/// Where a migration socket lives — cloud-hypervisor's two URL forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationUrl {
    /// `tcp:<host>:<port>` — cross-host; over the mesh the host is the
    /// target's **Nebula overlay IP**, so the stream rides the encrypted
    /// overlay.
    Tcp {
        /// Host/IP (`0.0.0.0` to listen on all interfaces).
        host: String,
        /// TCP port (see [`DEFAULT_MIGRATION_PORT`]).
        port: u16,
    },
    /// `unix:<path>` — same-host migration via a unix socket.
    Unix(PathBuf),
}

impl MigrationUrl {
    /// A `tcp:` URL (cross-host; over the mesh, `host` = the overlay IP).
    #[must_use]
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Self::Tcp {
            host: host.into(),
            port,
        }
    }

    /// A `unix:` URL (same-host).
    #[must_use]
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        Self::Unix(path.into())
    }
}

impl fmt::Display for MigrationUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp { host, port } => write!(f, "tcp:{host}:{port}"),
            Self::Unix(path) => write!(f, "unix:{}", path.display()),
        }
    }
}

/// Which VMM a [`MigrationStep`] talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationSide {
    /// The VMM currently running the guest.
    Source,
    /// The (initially empty) VMM the guest moves to.
    Target,
}

impl fmt::Display for MigrationSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Source => "source",
            Self::Target => "target",
        })
    }
}

/// The ordered migration steps.
///
/// cloud-hypervisor requires the receiver to be listening **before** the
/// sender streams, and a §7-honest broker verifies the guest actually runs
/// on the target before calling the move done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStep {
    /// `PUT vm.receive-migration` on the **target** VMM: listen on the plan's
    /// `listen` URL for the incoming guest.
    PrepareReceive,
    /// `PUT vm.send-migration` on the **source** VMM: stream the guest to the
    /// plan's `destination` URL.
    SendMigration,
    /// `GET vm.info` on the **target** VMM: the guest must report `Running`.
    VerifyRunning,
}

impl MigrationStep {
    /// The API endpoint this step hits (relative to `/api/v1`).
    #[must_use]
    pub const fn endpoint(self) -> &'static str {
        match self {
            Self::PrepareReceive => "/vm.receive-migration",
            Self::SendMigration => "/vm.send-migration",
            Self::VerifyRunning => "/vm.info",
        }
    }

    /// Which VMM this step talks to.
    #[must_use]
    pub const fn side(self) -> MigrationSide {
        match self {
            Self::PrepareReceive | Self::VerifyRunning => MigrationSide::Target,
            Self::SendMigration => MigrationSide::Source,
        }
    }
}

impl fmt::Display for MigrationStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PrepareReceive => "PrepareReceive (vm.receive-migration)",
            Self::SendMigration => "SendMigration (vm.send-migration)",
            Self::VerifyRunning => "VerifyRunning (vm.info)",
        })
    }
}

/// What to migrate and where the stream flows.
///
/// `listen` is what the **target** VMM binds (`tcp:0.0.0.0:<port>` /
/// `unix:<path>`); `destination` is what the **source** VMM dials — the two
/// differ cross-host (listen on all interfaces, dial the overlay IP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateRequest {
    /// The VM being moved (for reporting; the api-socket *is* the VM to CH).
    pub vm_name: String,
    /// The URL the target VMM listens on (`ReceiveMigrationData`'s
    /// `receiver_url`).
    pub listen: MigrationUrl,
    /// The URL the source VMM streams to (`SendMigrationData`'s
    /// `destination_url`).
    pub destination: MigrationUrl,
}

impl MigrateRequest {
    /// A request with explicit listen + destination URLs.
    #[must_use]
    pub fn new(
        vm_name: impl Into<String>,
        listen: MigrationUrl,
        destination: MigrationUrl,
    ) -> Self {
        Self {
            vm_name: vm_name.into(),
            listen,
            destination,
        }
    }

    /// The cross-host mesh case: the target listens on every interface at
    /// `port`, the source dials the target's **Nebula overlay IP** — so the
    /// guest streams inside the encrypted mesh.
    #[must_use]
    pub fn over_mesh(
        vm_name: impl Into<String>,
        target_overlay_ip: impl Into<String>,
        port: u16,
    ) -> Self {
        Self::new(
            vm_name,
            MigrationUrl::tcp("0.0.0.0", port),
            MigrationUrl::tcp(target_overlay_ip, port),
        )
    }
}

/// A planned migration: the request plus the fixed, ordered steps. Built by
/// [`plan_migration`] (pure); executed by [`run_migration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    request: MigrateRequest,
    steps: [MigrationStep; 3],
}

/// The one place the step ordering lives: receiver listens, then the source
/// streams, then the target must prove the guest runs.
pub const MIGRATION_STEPS: [MigrationStep; 3] = [
    MigrationStep::PrepareReceive,
    MigrationStep::SendMigration,
    MigrationStep::VerifyRunning,
];

impl MigrationPlan {
    /// The ordered steps (always [`MIGRATION_STEPS`] — carried on the plan so
    /// callers can render progress).
    #[must_use]
    pub const fn steps(&self) -> &[MigrationStep; 3] {
        &self.steps
    }

    /// The underlying request.
    #[must_use]
    pub const fn request(&self) -> &MigrateRequest {
        &self.request
    }

    /// The exact `vm.receive-migration` body (`ReceiveMigrationData`): the
    /// URL the target binds.
    #[must_use]
    pub fn receive_body(&self) -> Value {
        json!({ "receiver_url": self.request.listen.to_string() })
    }

    /// The exact `vm.send-migration` body (`SendMigrationData`): the URL the
    /// source dials. `local`/`timeout_s`/… ride cloud-hypervisor's defaults.
    #[must_use]
    pub fn send_body(&self) -> Value {
        json!({ "destination_url": self.request.destination.to_string() })
    }
}

/// Plan a migration — the pure core: a [`MigrateRequest`] → the ordered typed
/// steps + their exact API bodies, unit-tested without any VMM.
#[must_use]
pub const fn plan_migration(request: MigrateRequest) -> MigrationPlan {
    MigrationPlan {
        request,
        steps: MIGRATION_STEPS,
    }
}

/// Execute a [`MigrationPlan`] against the source + target VMMs, in step
/// order: `PrepareReceive` (target) → `SendMigration` (source) →
/// `VerifyRunning` (target).
///
/// Both [`Vm`] handles ride the [`ChTransport`](crate::ChTransport) seam, so
/// this is unit-tested with recording mocks. **Live**, the caller must be
/// able to reach both api-sockets: same-host migration reaches both locally;
/// cross-host each half runs on its own host's vm-lifecycle worker (mackesd
/// coordinates — see the module docs), with the guest streaming over the
/// mesh via the plan's `tcp:` overlay URL.
///
/// # Errors
/// [`MigrateError::Step`] naming the failed step/side and the underlying
/// [`KvmError`] (a missing VMM socket surfaces as its `Connect` error), or
/// [`MigrateError::NotRunningOnTarget`] when the API calls succeed but the
/// target does not report a `Running` guest.
pub fn run_migration<S: ChTransport, T: ChTransport>(
    source: &Vm<S>,
    target: &Vm<T>,
    plan: &MigrationPlan,
) -> Result<(), MigrateError> {
    let vm = &plan.request().vm_name;
    let step_err = |step: MigrationStep| {
        move |source: KvmError| MigrateError::Step {
            vm: vm.clone(),
            step,
            side: step.side(),
            source,
        }
    };

    // 1. The target must be listening before the source streams.
    target
        .put(
            MigrationStep::PrepareReceive.endpoint(),
            Some(&plan.receive_body().to_string()),
        )
        .map_err(step_err(MigrationStep::PrepareReceive))?;

    // 2. Stream the guest from the source to the destination URL.
    source
        .put(
            MigrationStep::SendMigration.endpoint(),
            Some(&plan.send_body().to_string()),
        )
        .map_err(step_err(MigrationStep::SendMigration))?;

    // 3. §7: verify the guest actually runs on the target — API 2xx alone is
    // not a delivered desktop.
    let info = target
        .info()
        .map_err(step_err(MigrationStep::VerifyRunning))?;
    if info.is_running() {
        Ok(())
    } else {
        Err(MigrateError::NotRunningOnTarget {
            vm: vm.clone(),
            state: info.state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ChResponse;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// One recorded call, tagged with which VMM took it — so step ORDER
    /// across the two transports is assertable from one shared log.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Event {
        side: &'static str,
        method: String,
        path: String,
        body: Option<String>,
    }

    /// A scripted transport: appends to the shared log and replays its queued
    /// responses in order (so the target can 204 the receive and then 200 the
    /// info).
    struct Scripted {
        side: &'static str,
        log: Rc<RefCell<Vec<Event>>>,
        responses: RefCell<VecDeque<ChResponse>>,
    }

    impl Scripted {
        fn new(
            side: &'static str,
            log: &Rc<RefCell<Vec<Event>>>,
            responses: Vec<(u16, &str)>,
        ) -> Self {
            Self {
                side,
                log: Rc::clone(log),
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(status, body)| ChResponse {
                            status,
                            body: body.to_string(),
                        })
                        .collect(),
                ),
            }
        }
    }

    impl ChTransport for Scripted {
        fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<&str>,
        ) -> Result<ChResponse, KvmError> {
            self.log.borrow_mut().push(Event {
                side: self.side,
                method: method.to_string(),
                path: path.to_string(),
                body: body.map(str::to_string),
            });
            Ok(self
                .responses
                .borrow_mut()
                .pop_front()
                .expect("scripted transport ran out of responses"))
        }
    }

    fn mesh_plan() -> MigrationPlan {
        plan_migration(MigrateRequest::over_mesh("web1", "10.42.0.7", 4444))
    }

    // ---- the pure planning core ----

    #[test]
    fn plan_orders_the_steps_receive_send_verify() {
        // THE ordering acceptance: receiver first, then the stream, then proof.
        let plan = mesh_plan();
        assert_eq!(
            plan.steps(),
            &[
                MigrationStep::PrepareReceive,
                MigrationStep::SendMigration,
                MigrationStep::VerifyRunning,
            ]
        );
        // each step knows its side + endpoint (the typed model).
        assert_eq!(MigrationStep::PrepareReceive.side(), MigrationSide::Target);
        assert_eq!(MigrationStep::SendMigration.side(), MigrationSide::Source);
        assert_eq!(MigrationStep::VerifyRunning.side(), MigrationSide::Target);
        assert_eq!(
            MigrationStep::PrepareReceive.endpoint(),
            "/vm.receive-migration"
        );
        assert_eq!(
            MigrationStep::SendMigration.endpoint(),
            "/vm.send-migration"
        );
        assert_eq!(MigrationStep::VerifyRunning.endpoint(), "/vm.info");
    }

    #[test]
    fn bodies_are_the_exact_ch_migration_payloads() {
        let plan = mesh_plan();
        // ReceiveMigrationData: the target binds every interface on the port.
        assert_eq!(
            plan.receive_body(),
            serde_json::json!({ "receiver_url": "tcp:0.0.0.0:4444" })
        );
        // SendMigrationData: the source dials the target's OVERLAY IP — the
        // stream rides the Nebula mesh.
        assert_eq!(
            plan.send_body(),
            serde_json::json!({ "destination_url": "tcp:10.42.0.7:4444" })
        );
    }

    #[test]
    fn migration_urls_render_in_ch_syntax() {
        assert_eq!(
            MigrationUrl::tcp("10.42.0.7", 4444).to_string(),
            "tcp:10.42.0.7:4444"
        );
        assert_eq!(
            MigrationUrl::unix("/run/mde-kvm/web1/mig.sock").to_string(),
            "unix:/run/mde-kvm/web1/mig.sock"
        );
    }

    #[test]
    fn same_host_request_takes_explicit_unix_urls() {
        let sock = MigrationUrl::unix("/tmp/mig.sock");
        let req = MigrateRequest::new("web1", sock.clone(), sock);
        let plan = plan_migration(req);
        assert_eq!(
            plan.receive_body()["receiver_url"],
            serde_json::json!("unix:/tmp/mig.sock")
        );
        assert_eq!(
            plan.send_body()["destination_url"],
            serde_json::json!("unix:/tmp/mig.sock")
        );
    }

    // ---- the executor over the transport seam ----

    #[test]
    fn run_migration_drives_the_steps_in_order_across_both_vmms() {
        // THE executor acceptance: target listens, source streams, target
        // proves Running — in that order, with the exact bodies.
        let log = Rc::new(RefCell::new(Vec::new()));
        let source = Vm::with_transport(Scripted::new("source", &log, vec![(204, "")]));
        let target = Vm::with_transport(Scripted::new(
            "target",
            &log,
            vec![(204, ""), (200, r#"{"state":"Running"}"#)],
        ));
        run_migration(&source, &target, &mesh_plan()).expect("migration");

        let events = log.borrow();
        assert_eq!(events.len(), 3);
        // 1. target: vm.receive-migration with the listen URL.
        assert_eq!(events[0].side, "target");
        assert_eq!(events[0].method, "PUT");
        assert_eq!(events[0].path, "/api/v1/vm.receive-migration");
        let body: Value =
            serde_json::from_str(events[0].body.as_ref().expect("body")).expect("json");
        assert_eq!(
            body,
            serde_json::json!({ "receiver_url": "tcp:0.0.0.0:4444" })
        );
        // 2. source: vm.send-migration with the overlay destination.
        assert_eq!(events[1].side, "source");
        assert_eq!(events[1].method, "PUT");
        assert_eq!(events[1].path, "/api/v1/vm.send-migration");
        let body: Value =
            serde_json::from_str(events[1].body.as_ref().expect("body")).expect("json");
        assert_eq!(
            body,
            serde_json::json!({ "destination_url": "tcp:10.42.0.7:4444" })
        );
        // 3. target: vm.info proves the guest runs.
        assert_eq!(events[2].side, "target");
        assert_eq!(events[2].method, "GET");
        assert_eq!(events[2].path, "/api/v1/vm.info");
    }

    #[test]
    fn a_failed_receive_stops_before_the_source_is_ever_told_to_stream() {
        // if the receiver can't listen, streaming anyway could wedge the
        // guest — the executor must stop at step 1.
        let log = Rc::new(RefCell::new(Vec::new()));
        let source = Vm::with_transport(Scripted::new("source", &log, vec![]));
        let target = Vm::with_transport(Scripted::new(
            "target",
            &log,
            vec![(500, "vmm: cannot bind")],
        ));
        let err = run_migration(&source, &target, &mesh_plan()).expect_err("must fail");
        assert!(
            matches!(
                &err,
                MigrateError::Step {
                    vm,
                    step: MigrationStep::PrepareReceive,
                    side: MigrationSide::Target,
                    ..
                } if vm == "web1"
            ),
            "{err:?}"
        );
        // exactly one call happened — the source VMM was never touched.
        let events = log.borrow();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].side, "target");
        // the rendered error names the step, the side, and the VMM's reason.
        let msg = format!(
            "{err}: {}",
            std::error::Error::source(&err).expect("source")
        );
        assert!(msg.contains("PrepareReceive"), "{msg}");
        assert!(msg.contains("target"), "{msg}");
        assert!(msg.contains("cannot bind"), "{msg}");
    }

    #[test]
    fn a_failed_send_is_typed_to_the_source_side_and_skips_verification() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let source = Vm::with_transport(Scripted::new(
            "source",
            &log,
            vec![(500, "vmm: destination unreachable")],
        ));
        let target = Vm::with_transport(Scripted::new("target", &log, vec![(204, "")]));
        let err = run_migration(&source, &target, &mesh_plan()).expect_err("must fail");
        assert!(
            matches!(
                &err,
                MigrateError::Step {
                    step: MigrationStep::SendMigration,
                    side: MigrationSide::Source,
                    ..
                }
            ),
            "{err:?}"
        );
        // receive + send happened; vm.info was never asked.
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn a_target_that_is_not_running_is_a_typed_failure_not_a_fake_success() {
        // all three calls 2xx, but the guest sits in Created on the target —
        // the executor must refuse to call that a migration.
        let log = Rc::new(RefCell::new(Vec::new()));
        let source = Vm::with_transport(Scripted::new("source", &log, vec![(204, "")]));
        let target = Vm::with_transport(Scripted::new(
            "target",
            &log,
            vec![(204, ""), (200, r#"{"state":"Created"}"#)],
        ));
        let err = run_migration(&source, &target, &mesh_plan()).expect_err("must fail");
        assert!(
            matches!(
                &err,
                MigrateError::NotRunningOnTarget { vm, state } if vm == "web1" && state == "Created"
            ),
            "{err:?}"
        );
        // the message warns the operator not to tear down the source.
        assert!(err.to_string().contains("do not tear down"), "{err}");
    }

    #[test]
    fn live_executor_without_vmm_sockets_names_the_missing_socket() {
        // the gated-live story: connecting the real transports to sockets
        // that don't exist fails at step 1 with a typed Connect error naming
        // the exact missing api-socket path — what's missing, spelled out.
        let source = Vm::connect("/run/mde-kvm/no-such-vm/api.sock");
        let target = Vm::connect("/run/mde-kvm/no-such-target/api.sock");
        let err = run_migration(&source, &target, &mesh_plan()).expect_err("no live VMMs");
        assert!(
            matches!(
                &err,
                MigrateError::Step {
                    step: MigrationStep::PrepareReceive,
                    side: MigrationSide::Target,
                    source: KvmError::Connect(path, _),
                    ..
                } if path == &PathBuf::from("/run/mde-kvm/no-such-target/api.sock")
            ),
            "expected a Connect step error naming the missing socket, got {err:?}"
        );
    }
}
