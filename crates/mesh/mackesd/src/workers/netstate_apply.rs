//! PLANES-15 (W66/W77/W78) — the netstate engine mount.
//!
//! The runtime-reachable side of [`magic_fleet::netstate`]: on a cadence
//! (and right after a fleet nudge would have landed a new revision) this
//! worker reads the **elected** fleet revision's `netstate` desired-state
//! and converges the box's network to it — but ALWAYS through the
//! checkpoint-guarded apply ([`apply_with_self_test`]) so a bad
//! address/route can never strand the node off its own overlay (W77/W78).
//!
//! The reachability self-test targets are derived live from the roster
//! mirror: the lighthouse's overlay IP plus one other peer's (never this
//! box). If after apply the node can't still reach BOTH, the nmstate
//! checkpoint rolls it back and the worker logs the rollback loudly. With
//! no `netstate` declared (the common case) this is a cheap no-op.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mackes_mesh_types::health::{
    NodeAddressFamily, NodeAvailabilityState, NodeConnectionType, NodeConnectivitySummary,
    NodeDeviceClass, MAX_NODE_CONNECTIVITY_INTERFACE_BYTES,
};
use magic_fleet::netstate::{
    apply_with_self_test, ApplyOutcome, LinkState, NetInterface, NetOps, NetState, SystemNetOps,
};

use super::node_availability::{
    runtime_availability_path, RuntimeAvailabilityPublisher, RuntimeAvailabilityRequest,
};
use super::{ShutdownToken, Worker};

/// Converge cadence — paced with `fleet_reconcile`'s full tick.
pub const CADENCE: Duration = Duration::from_secs(900);

const ADAPTER_EXPECTED_RETURN: Duration = Duration::from_secs(5 * 60);

/// The netstate engine mount worker.
pub struct NetstateApplyWorker {
    workgroup_root: PathBuf,
    store_db: Option<PathBuf>,
    hostname: String,
    bus_root: Option<PathBuf>,
    availability_durable_path: PathBuf,
    #[cfg(test)]
    probe_targets_override: Option<Vec<String>>,
}

impl NetstateApplyWorker {
    /// Create the worker. `store_db` is the roster mirror used to derive
    /// post-apply self-test probe targets (lighthouse + one peer).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, store_db: Option<PathBuf>, hostname: String) -> Self {
        let availability_durable_path = runtime_availability_path(&workgroup_root, &hostname);
        Self {
            workgroup_root,
            store_db,
            hostname,
            bus_root: mde_bus::default_data_dir(),
            availability_durable_path,
            #[cfg(test)]
            probe_targets_override: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    fn with_availability(mut self, bus_root: PathBuf, durable_path: PathBuf) -> Self {
        self.bus_root = Some(bus_root);
        self.availability_durable_path = durable_path;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_probe_targets(mut self, targets: Vec<String>) -> Self {
        self.probe_targets_override = Some(targets);
        self
    }

    /// The overlay IPs the post-apply self-test must still reach: the
    /// lighthouse (role `host`) and one other peer, never this box. An
    /// empty list (e.g. a lone lighthouse) means "no peers to lose" — the
    /// self-test then trivially passes, which is correct: there is no
    /// overlay path to sever.
    fn probe_targets(&self) -> Vec<String> {
        #[cfg(test)]
        if let Some(targets) = &self.probe_targets_override {
            return targets.clone();
        }
        let Some(db) = &self.store_db else {
            return Vec::new();
        };
        let Ok(conn) =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            return Vec::new();
        };
        let Ok(rows) = crate::nebula_roster::export_roster(&conn) else {
            return Vec::new();
        };
        let others: Vec<&crate::nebula_roster::RosterRow> = rows
            .iter()
            .filter(|r| r.name != self.hostname && !r.overlay_ip.is_empty())
            .collect();
        let mut targets = Vec::new();
        // The lighthouse first (groups carries the role; `host` = lighthouse).
        if let Some(lh) = others.iter().find(|r| r.groups.contains("host")) {
            targets.push(lh.overlay_ip.clone());
        }
        // Plus one more distinct peer.
        if let Some(peer) = others.iter().find(|r| !targets.contains(&r.overlay_ip)) {
            targets.push(peer.overlay_ip.clone());
        }
        targets
    }

    /// One converge pass. Returns the outcome (for tests / logging).
    #[cfg(test)]
    fn converge(&self, ops: &dyn NetOps) -> ApplyOutcome {
        let dir = magic_fleet::store::revisions_dir(&self.workgroup_root);
        let Some(head) = magic_fleet::store::elect_head(&dir) else {
            return ApplyOutcome::NoChange;
        };
        if head.spec.netstate.is_empty() {
            return ApplyOutcome::NoChange;
        }
        apply_with_self_test(ops, &head.spec.netstate, &self.probe_targets())
    }

    fn availability_publisher(&self) -> Result<RuntimeAvailabilityPublisher, String> {
        let bus_root = self
            .bus_root
            .clone()
            .ok_or_else(|| "shared Bus root is unavailable".to_string())?;
        Ok(RuntimeAvailabilityPublisher::new(
            self.hostname.clone(),
            self.hostname.clone(),
            NodeDeviceClass::Unknown,
            bus_root,
            self.availability_durable_path.clone(),
        ))
    }

    /// Production converge path: announce an adapter/address transition before
    /// checkpointed mutation, then publish `returned` only after the existing
    /// overlay self-test and checkpoint commit prove stabilization.
    fn converge_with_availability(&self, ops: &dyn NetOps) -> ApplyOutcome {
        let dir = magic_fleet::store::revisions_dir(&self.workgroup_root);
        let Some(head) = magic_fleet::store::elect_head(&dir) else {
            return ApplyOutcome::NoChange;
        };
        let desired = &head.spec.netstate;
        if desired.is_empty() {
            return ApplyOutcome::NoChange;
        }
        let actual = ops.read_actual();
        let targets = self.probe_targets();
        let publisher = match self.availability_publisher() {
            Ok(publisher) => publisher,
            Err(error) => return ApplyOutcome::Failed { error },
        };
        if let Err(error) = publisher.correct_forward() {
            return ApplyOutcome::Failed {
                error: format!("availability corrected-forward retry: {error}"),
            };
        }

        if desired.diff(&actual).is_empty() {
            if let Ok(Some(current)) = publisher.current_intent() {
                if current.state == NodeAvailabilityState::AdapterMigration
                    && !targets.is_empty()
                    && ops.unreachable(&targets).is_empty()
                {
                    let old = current.old_connectivity.clone();
                    let new = current.new_connectivity.clone();
                    if let (Some(old), Some(mut new)) = (old, new) {
                        new.reachable = true;
                        if let Err(error) = publisher.publish(
                            RuntimeAvailabilityRequest::lifecycle(
                                NodeAvailabilityState::Returned,
                                "netstate-apply",
                                "managed network stabilized",
                                None,
                            )
                            .with_connectivity(old, new),
                        ) {
                            return ApplyOutcome::Failed {
                                error: format!("availability returned publication: {error}"),
                            };
                        }
                    }
                }
            }
            return ApplyOutcome::NoChange;
        }

        let old = connectivity_summary(&actual, false);
        let new = connectivity_summary(desired, false);
        let report_transition = old != new;
        if report_transition {
            let request = RuntimeAvailabilityRequest::lifecycle(
                NodeAvailabilityState::AdapterMigration,
                "netstate-apply",
                "managed adapter or address transition",
                Some(ADAPTER_EXPECTED_RETURN),
            )
            .with_connectivity(old.clone(), new.clone());
            if let Err(error) = publisher.publish(request) {
                return ApplyOutcome::Failed {
                    error: format!("availability transition publication: {error}"),
                };
            }
        }

        let outcome = apply_with_self_test(ops, desired, &targets);
        if outcome == ApplyOutcome::Committed && report_transition && !targets.is_empty() {
            let mut stabilized = new;
            stabilized.reachable = true;
            if let Err(error) = publisher.publish(
                RuntimeAvailabilityRequest::lifecycle(
                    NodeAvailabilityState::Returned,
                    "netstate-apply",
                    "managed network stabilized",
                    None,
                )
                .with_connectivity(old, stabilized),
            ) {
                tracing::warn!(
                    %error,
                    "netstate_apply: network committed but returned publication remains retryable"
                );
            }
        }
        outcome
    }

    fn tick(&self) {
        let outcome = self.converge_with_availability(&SystemNetOps);
        match &outcome {
            ApplyOutcome::NoChange => {}
            ApplyOutcome::Committed => {
                tracing::info!("netstate_apply: network converged + self-test passed (PLANES-15)");
            }
            ApplyOutcome::RolledBack { unreachable } => {
                tracing::warn!(
                    ?unreachable,
                    "netstate_apply: self-test FAILED — checkpoint reverted the box (W78)"
                );
            }
            ApplyOutcome::Failed { error } => {
                tracing::warn!(%error, "netstate_apply: apply errored — reverted");
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for NetstateApplyWorker {
    fn name(&self) -> &'static str {
        "netstate_apply"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            // nmstatectl + ping are blocking; keep them off the scheduler.
            let this = NetstateApplyWorker {
                workgroup_root: self.workgroup_root.clone(),
                store_db: self.store_db.clone(),
                hostname: self.hostname.clone(),
                bus_root: self.bus_root.clone(),
                availability_durable_path: self.availability_durable_path.clone(),
                #[cfg(test)]
                probe_targets_override: self.probe_targets_override.clone(),
            };
            let _ = tokio::task::spawn_blocking(move || this.tick()).await;
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(CADENCE) => {}
            }
        }
    }
}

fn connectivity_summary(state: &NetState, reachable: bool) -> NodeConnectivitySummary {
    let interface = state
        .interfaces
        .iter()
        .filter(|interface| interface.state == LinkState::Up)
        .filter(|interface| interface_address_family(interface) != NodeAddressFamily::None)
        .min_by(|left, right| left.name.cmp(&right.name))
        .or_else(|| {
            state
                .interfaces
                .iter()
                .filter(|interface| interface.state == LinkState::Up)
                .min_by(|left, right| left.name.cmp(&right.name))
        });

    let Some(interface) = interface else {
        return NodeConnectivitySummary {
            connection_type: NodeConnectionType::Disconnected,
            interface_id: None,
            address_family: NodeAddressFamily::None,
            reachable: false,
        };
    };
    let address_family = interface_address_family(interface);
    NodeConnectivitySummary {
        connection_type: connection_type(interface),
        interface_id: valid_interface_id(&interface.name).then(|| interface.name.clone()),
        address_family,
        reachable: reachable && address_family != NodeAddressFamily::None,
    }
}

fn interface_address_family(interface: &NetInterface) -> NodeAddressFamily {
    let configured = |config: &Option<magic_fleet::netstate::IpConfig>| {
        config
            .as_ref()
            .is_some_and(|config| config.enabled && (config.dhcp || !config.addresses.is_empty()))
    };
    match (configured(&interface.ipv4), configured(&interface.ipv6)) {
        (true, true) => NodeAddressFamily::DualStack,
        (true, false) => NodeAddressFamily::Ipv4,
        (false, true) => NodeAddressFamily::Ipv6,
        (false, false) => NodeAddressFamily::None,
    }
}

fn connection_type(interface: &NetInterface) -> NodeConnectionType {
    match interface.iface_type.to_ascii_lowercase().as_str() {
        "ethernet" | "bond" | "vlan" => NodeConnectionType::Ethernet,
        "wifi" | "wireless" => NodeConnectionType::Wifi,
        "cellular" | "wwan" | "gsm" => NodeConnectionType::Cellular,
        "nebula" | "mesh" | "wireguard" => NodeConnectionType::Mesh,
        _ => NodeConnectionType::Unknown,
    }
}

fn valid_interface_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NODE_CONNECTIVITY_INTERFACE_BYTES
        && value.is_ascii()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use magic_fleet::netstate::{IpAddress, IpConfig, LinkState, NetInterface, NetState};
    use magic_fleet::store::{revisions_dir, write_revision};
    use magic_fleet::{BaselineSpec, Revision};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A mock that reports an empty actual state and a fixed reachability
    /// verdict — exercises the worker's converge() without root or NICs.
    struct Mock {
        reachable: bool,
    }
    impl NetOps for Mock {
        fn read_actual(&self) -> NetState {
            NetState::default()
        }
        fn checkpoint(&self) -> Result<String, String> {
            Ok("cp".into())
        }
        fn apply(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn unreachable(&self, t: &[String]) -> Vec<String> {
            if self.reachable {
                Vec::new()
            } else {
                t.to_vec()
            }
        }
        fn commit(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn rollback(&self, _: &str) {}
    }

    struct OrderingMock {
        reachable: bool,
        bus_root: PathBuf,
        saw_intent_before_apply: Arc<AtomicBool>,
    }

    impl NetOps for OrderingMock {
        fn read_actual(&self) -> NetState {
            NetState::default()
        }

        fn checkpoint(&self) -> Result<String, String> {
            Ok("cp".into())
        }

        fn apply(&self, _: &str) -> Result<(), String> {
            let persist = mde_bus::persist::Persist::open(self.bus_root.clone())
                .map_err(|error| format!("open ordering Bus: {error}"))?;
            let has_intent = persist
                .list_since(&mackes_mesh_types::health::node_health_topic("pine"), None)
                .map_err(|error| format!("read ordering Bus: {error}"))?
                .into_iter()
                .any(|message| {
                    message.body.as_deref().is_some_and(|body| {
                        serde_json::from_str::<mackes_mesh_types::health::NodeAvailabilityIntent>(
                            body,
                        )
                        .is_ok_and(|intent| intent.state == NodeAvailabilityState::AdapterMigration)
                    })
                });
            self.saw_intent_before_apply
                .store(has_intent, Ordering::SeqCst);
            Ok(())
        }

        fn unreachable(&self, targets: &[String]) -> Vec<String> {
            if self.reachable {
                Vec::new()
            } else {
                targets.to_vec()
            }
        }

        fn commit(&self, _: &str) -> Result<(), String> {
            Ok(())
        }

        fn rollback(&self, _: &str) {}
    }

    fn seed_revision_with_netstate(root: &std::path::Path) {
        let mut spec = BaselineSpec::default();
        spec.netstate = NetState {
            interfaces: vec![NetInterface {
                name: "eth0".into(),
                iface_type: "ethernet".into(),
                state: LinkState::Up,
                ipv4: Some(IpConfig {
                    enabled: true,
                    dhcp: false,
                    addresses: vec![IpAddress {
                        ip: "10.42.0.7".into(),
                        prefix_len: 24,
                    }],
                }),
                ipv6: None,
            }],
            ..Default::default()
        };
        let dir = revisions_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        write_revision(
            &dir,
            &Revision {
                version: 1,
                author: "peer:oak".into(),
                at: 100,
                spec,
            },
        )
        .unwrap();
    }

    #[test]
    fn no_revision_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: true }),
            ApplyOutcome::NoChange
        );
    }

    #[test]
    fn netstate_revision_converges_when_self_test_passes() {
        let tmp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(tmp.path());
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: true }),
            ApplyOutcome::Committed
        );
    }

    #[test]
    fn no_probe_targets_means_nothing_to_lose_so_it_commits() {
        // No store_db → empty roster → no probe targets. The W78
        // "no overlay path to sever" case: even with the mock reporting
        // everything unreachable, an EMPTY target list yields an empty
        // unreachable set, so the apply commits. (The rollback path with
        // real targets is pinned in the engine's own tests.)
        let tmp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(tmp.path());
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: false }),
            ApplyOutcome::Committed
        );
    }

    #[test]
    fn managed_network_transition_is_announced_before_apply_and_returned_after_stabilization() {
        let temp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(temp.path());
        let bus_root = temp.path().join("bus");
        let saw_intent_before_apply = Arc::new(AtomicBool::new(false));
        let worker = NetstateApplyWorker::new(temp.path().to_path_buf(), None, "pine".into())
            .with_availability(
                bus_root.clone(),
                temp.path().join("availability/current.json"),
            )
            .with_probe_targets(vec!["10.42.0.1".into()]);
        let ops = OrderingMock {
            reachable: true,
            bus_root: bus_root.clone(),
            saw_intent_before_apply: Arc::clone(&saw_intent_before_apply),
        };

        assert_eq!(
            worker.converge_with_availability(&ops),
            ApplyOutcome::Committed
        );
        assert!(saw_intent_before_apply.load(Ordering::SeqCst));

        let persist = mde_bus::persist::Persist::open(bus_root).unwrap();
        let intents = persist
            .list_since(&mackes_mesh_types::health::node_health_topic("pine"), None)
            .unwrap()
            .into_iter()
            .map(|message| {
                serde_json::from_str::<mackes_mesh_types::health::NodeAvailabilityIntent>(
                    message.body.as_deref().unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(intents.len(), 2);
        assert_eq!(intents[0].state, NodeAvailabilityState::AdapterMigration);
        assert_eq!(intents[1].state, NodeAvailabilityState::Returned);
        assert_eq!((intents[0].generation, intents[1].generation), (1, 2));
        assert_eq!(
            intents[1]
                .new_connectivity
                .as_ref()
                .map(|summary| summary.reachable),
            Some(true)
        );
        let wire = serde_json::to_string(&intents).unwrap();
        assert!(
            !wire.contains("10.42.0.7"),
            "raw addresses must not be published"
        );
    }

    #[test]
    fn failed_network_stabilization_never_publishes_returned() {
        let temp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(temp.path());
        let bus_root = temp.path().join("bus");
        let worker = NetstateApplyWorker::new(temp.path().to_path_buf(), None, "pine".into())
            .with_availability(
                bus_root.clone(),
                temp.path().join("availability/current.json"),
            )
            .with_probe_targets(vec!["10.42.0.1".into()]);
        let ops = OrderingMock {
            reachable: false,
            bus_root: bus_root.clone(),
            saw_intent_before_apply: Arc::new(AtomicBool::new(false)),
        };

        assert_eq!(
            worker.converge_with_availability(&ops),
            ApplyOutcome::RolledBack {
                unreachable: vec!["10.42.0.1".into()]
            }
        );

        let persist = mde_bus::persist::Persist::open(bus_root).unwrap();
        let intents = persist
            .list_since(&mackes_mesh_types::health::node_health_topic("pine"), None)
            .unwrap();
        assert_eq!(intents.len(), 1);
        let intent: mackes_mesh_types::health::NodeAvailabilityIntent =
            serde_json::from_str(intents[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(intent.state, NodeAvailabilityState::AdapterMigration);
        assert_eq!(intent.generation, 1);
    }
}
