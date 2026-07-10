//! KVM-HEALTH (MV-2) — per-node KVM virtualization stack health worker.
//!
//! The Fedora + KVM successor to the retired `xcpng_health` worker. Runs on
//! **every** mesh node — the KVM stack is universal (one identical libvirt +
//! Podman set on Lighthouse and Workstation alike;
//! `docs/design/mesh-virt-management.md`: "same stack on every machine"), so
//! unlike `xcpng_health` (pinned to the dead `Xcpng` role) it is gated at the
//! `run_serve` spawn site through the rank-0-default worker resolver, i.e. it
//! runs everywhere. Each tick it probes every service in the canonical
//! [`crate::kvm::KVM_SERVICES`] catalog (`systemctl is-active <unit>`) and
//! publishes a whole-host health summary to the [`SERVICES_TOPIC`]
//! (`event/kvm/services`) Mackes-Bus topic, so the Workbench Datacenter view +
//! the alert lane see the live stack state without each consumer re-probing.
//!
//! The decision is the pure [`decide`] fn folding the catalog + a
//! [`ServiceProbe`] into a [`KvmHealth`] summary — unit-tested with a fake
//! probe. `tick_once` is the thin shell: the production [`SystemctlProbe`] seam
//! + the `mde-bus` publish (the same fire-and-reap path
//! [`compute_registry::publish_event`](super::compute_registry::publish_event)
//! uses), so the tested core never touches systemd or the bus.

#![cfg(feature = "async-services")]

use std::process::Command;
use std::time::Duration;

use crate::kvm::{KvmService, KVM_SERVICES};

use super::{ShutdownToken, Worker};

/// 30 s tick — the virtualization stack is slow-changing (a daemon down is a
/// rare, operator-visible event), and a 30 s summary keeps the probe cheap (one
/// bounded `systemctl is-active` per catalog service) while staying fresh enough
/// for a host panel.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Bus topic the whole-host KVM stack health summary is published to.
pub const SERVICES_TOPIC: &str = "event/kvm/services";

/// Injectable seam over the per-unit `systemctl is-active` probe, so the pure
/// [`decide`] core is unit-testable without a live systemd. Production wires
/// [`SystemctlProbe`]; tests pass a fake.
pub trait ServiceProbe {
    /// Whether the systemd `unit` is currently active (`systemctl is-active`
    /// exit 0). A missing/failed/unknown unit reads as `false` (not active).
    fn is_active(&self, unit: &str) -> bool;
}

/// Production [`ServiceProbe`]: `systemctl is-active --quiet <unit>`, bounded by
/// the EFF-20 timeout so a wedged systemd can't pin the tick. Any spawn error /
/// timeout / non-zero exit reads as inactive.
pub struct SystemctlProbe;

impl ServiceProbe for SystemctlProbe {
    fn is_active(&self, unit: &str) -> bool {
        let mut cmd = Command::new("systemctl");
        cmd.args(["is-active", "--quiet", unit]);
        crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// One KVM service's liveness, as carried in the published summary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceHealth {
    /// Canonical service id ([`KvmService::id`]).
    pub id: String,
    /// The systemd unit probed ([`KvmService::unit`]).
    pub unit: String,
    /// `true` when `systemctl is-active` reported the unit active.
    pub active: bool,
}

/// Whole-host KVM virtualization stack health summary — the body published to
/// [`SERVICES_TOPIC`]. `serde` so a consumer (Workbench Datacenter view) reads
/// one row per node off the bus without re-probing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KvmHealth {
    /// Publishing node identity (the node id this summary describes).
    pub host: String,
    /// Per-service liveness in catalog (management-brain-first) order.
    pub services: Vec<ServiceHealth>,
    /// Count of active services.
    pub active: usize,
    /// Total services in the probed catalog.
    pub total: usize,
    /// `true` iff every catalog service is active (and the catalog is non-empty).
    pub all_healthy: bool,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

impl KvmHealth {
    /// One-line status for logs / a panel header, e.g.
    /// `"all 6 KVM services active"` or `"4/6 KVM services active (2 down)"`.
    #[must_use]
    pub fn status_line(&self) -> String {
        if self.all_healthy {
            format!("all {} KVM services active", self.total)
        } else {
            let down = self.total.saturating_sub(self.active);
            format!(
                "{}/{} KVM services active ({down} down)",
                self.active, self.total,
            )
        }
    }

    /// Ids of the catalog services that are NOT active — the operator's
    /// punch-list, in catalog order.
    #[must_use]
    pub fn down_ids(&self) -> Vec<&str> {
        self.services
            .iter()
            .filter(|s| !s.active)
            .map(|s| s.id.as_str())
            .collect()
    }
}

/// Pure health decision: probe each catalog service via the injected
/// [`ServiceProbe`] and fold the results into a [`KvmHealth`] summary. The probe
/// is the only seam to the outside, so this is fully unit-testable with a fake
/// probe — no systemd, no bus, no clock (`now_ms` is passed in).
#[must_use]
pub fn decide(
    host: &str,
    catalog: &[KvmService],
    probe: &dyn ServiceProbe,
    now_ms: u64,
) -> KvmHealth {
    let services: Vec<ServiceHealth> = catalog
        .iter()
        .map(|s| ServiceHealth {
            id: s.id.to_string(),
            unit: s.unit.to_string(),
            active: probe.is_active(s.unit),
        })
        .collect();
    let total = services.len();
    let active = services.iter().filter(|s| s.active).count();
    KvmHealth {
        host: host.to_string(),
        // An empty catalog is not "healthy" — there is nothing to be healthy
        // about, and reporting `all_healthy` on zero services would mask a
        // mis-wired catalog.
        all_healthy: total > 0 && active == total,
        active,
        total,
        services,
        published_at_ms: now_ms,
    }
}

/// Publish a health summary to [`SERVICES_TOPIC`] via the `mde-bus` CLI — the
/// same fire-and-reap path the other tick-publishers use
/// ([`compute_registry::publish_event`](super::compute_registry::publish_event)).
/// Best-effort: a missing `mde-bus` binary (pre-RPM dev box) is swallowed, and
/// the detached reaper prevents a zombie pile.
fn publish(health: &KvmHealth) {
    let Ok(body) = serde_json::to_string(health) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", SERVICES_TOPIC, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The KVM-HEALTH worker.
pub struct KvmHealthWorker {
    /// Publishing node identity, stamped into every summary's `host`.
    host: String,
    /// Probed catalog — the canonical [`KVM_SERVICES`] in production,
    /// overridable in tests via [`Self::with_catalog`].
    catalog: &'static [KvmService],
    /// The injectable systemctl seam (production: [`SystemctlProbe`]).
    probe: Box<dyn ServiceProbe + Send + Sync>,
    /// Tick cadence (default [`DEFAULT_TICK_INTERVAL`]).
    tick: Duration,
}

impl KvmHealthWorker {
    /// Construct with production defaults: the canonical catalog, the live
    /// `systemctl` probe, and a 30 s tick. `host` is the publishing node
    /// identity stamped into each summary.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            catalog: KVM_SERVICES,
            probe: Box::new(SystemctlProbe),
            tick: DEFAULT_TICK_INTERVAL,
        }
    }

    /// Override the tick cadence — used by tests to avoid 30 s waits.
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Inject a probe (tests). Production uses the [`SystemctlProbe`] default.
    #[must_use]
    pub fn with_probe(mut self, probe: Box<dyn ServiceProbe + Send + Sync>) -> Self {
        self.probe = probe;
        self
    }

    /// Override the probed catalog (tests). Production uses [`KVM_SERVICES`].
    #[must_use]
    pub fn with_catalog(mut self, catalog: &'static [KvmService]) -> Self {
        self.catalog = catalog;
        self
    }

    /// One tick: probe the catalog, log a degraded stack on the alert lane, and
    /// publish the summary.
    fn tick_once(&self) {
        let health = decide(&self.host, self.catalog, self.probe.as_ref(), now_ms());
        if !health.all_healthy {
            // Repeated every tick while degraded — a log-pipeline window alert
            // keeps firing until the stack recovers (mirrors the
            // metrics_exporter alert convention).
            tracing::warn!(
                target: "mackesd::alert",
                down = ?health.down_ids(),
                "ALERT (warn): KVM virtualization stack degraded — {}",
                health.status_line(),
            );
        }
        publish(&health);
    }
}

#[async_trait::async_trait]
impl Worker for KvmHealthWorker {
    fn name(&self) -> &'static str {
        "kvm_health"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Publish an immediate summary on start so a panel doesn't wait a full
        // tick for the first health row.
        self.tick_once();
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.tick) => self.tick_once(),
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Fake probe: a unit is active iff it's in the `active` set. Lets the pure
    /// [`decide`] core be driven over the real catalog without systemd.
    struct FakeProbe {
        active: BTreeSet<String>,
    }

    impl FakeProbe {
        fn with(units: &[&str]) -> Self {
            Self {
                active: units.iter().map(|u| (*u).to_string()).collect(),
            }
        }
        /// Every unit in the catalog active.
        fn all() -> Self {
            Self {
                active: KVM_SERVICES.iter().map(|s| s.unit.to_string()).collect(),
            }
        }
        /// No unit active.
        fn none() -> Self {
            Self {
                active: BTreeSet::new(),
            }
        }
    }

    impl ServiceProbe for FakeProbe {
        fn is_active(&self, unit: &str) -> bool {
            self.active.contains(unit)
        }
    }

    #[test]
    fn decide_all_healthy_when_every_unit_active() {
        let h = decide("node-a", KVM_SERVICES, &FakeProbe::all(), 100);
        assert!(h.all_healthy);
        assert_eq!(h.active, h.total);
        assert_eq!(h.total, KVM_SERVICES.len());
        assert!(h.down_ids().is_empty());
        assert_eq!(
            h.status_line(),
            format!("all {} KVM services active", h.total)
        );
        assert_eq!(h.host, "node-a");
        assert_eq!(h.published_at_ms, 100);
    }

    #[test]
    fn decide_marks_down_services_and_counts() {
        // Only libvirtd active — and because the default network + storage pool
        // share libvirtd's unit, they come up with it; podman and
        // NetworkManager are down.
        let h = decide(
            "node-b",
            KVM_SERVICES,
            &FakeProbe::with(&["libvirtd.service"]),
            7,
        );
        assert!(!h.all_healthy);
        assert_eq!(h.active, 3, "libvirtd + libvirt-network + libvirt-storage");
        assert_eq!(h.total, KVM_SERVICES.len());
        let down = h.down_ids();
        assert!(down.contains(&"podman"));
        assert!(down.contains(&"network-manager"));
        assert!(!down.contains(&"libvirtd"));
        assert!(!down.contains(&"libvirt-network"));
        assert!(!down.contains(&"libvirt-storage"));
        assert_eq!(down.len(), h.total - 3);
        assert_eq!(
            h.status_line(),
            format!("3/{} KVM services active ({} down)", h.total, h.total - 3)
        );
    }

    #[test]
    fn decide_all_down_is_not_healthy() {
        let h = decide("node-c", KVM_SERVICES, &FakeProbe::none(), 1);
        assert!(!h.all_healthy);
        assert_eq!(h.active, 0);
        assert_eq!(h.down_ids().len(), h.total);
    }

    #[test]
    fn decide_empty_catalog_is_not_healthy() {
        // A mis-wired (empty) catalog must NOT read as all-healthy.
        let h = decide("node-d", &[], &FakeProbe::all(), 1);
        assert!(!h.all_healthy);
        assert_eq!(h.total, 0);
        assert_eq!(h.active, 0);
        assert!(h.services.is_empty());
    }

    #[test]
    fn decide_preserves_catalog_order_and_pairs_id_to_unit() {
        let h = decide("node-e", KVM_SERVICES, &FakeProbe::all(), 0);
        assert_eq!(h.services.len(), KVM_SERVICES.len());
        for (got, want) in h.services.iter().zip(KVM_SERVICES.iter()) {
            assert_eq!(got.id, want.id);
            assert_eq!(got.unit, want.unit);
            assert!(got.active);
        }
    }

    #[test]
    fn health_round_trips_json_as_the_bus_body() {
        let h = decide(
            "node-f",
            KVM_SERVICES,
            &FakeProbe::with(&["podman.socket"]),
            42,
        );
        let json = serde_json::to_string(&h).expect("serialize");
        let back: KvmHealth = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, h);
    }

    #[test]
    fn topic_is_event_namespaced() {
        assert_eq!(SERVICES_TOPIC, "event/kvm/services");
        // `event/*` topics are auditable + panel-routable; the namespace matters.
        assert!(SERVICES_TOPIC.starts_with("event/"));
    }

    #[test]
    fn worker_name_matches_module() {
        let w = KvmHealthWorker::new("node".to_string());
        assert_eq!(w.name(), "kvm_health");
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        // The worker drives over the real catalog with an injected fake probe
        // (all active) + a short tick, and exits promptly when shutdown fires —
        // no systemd, no bus binary needed (publish is a swallowed no-op here).
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = KvmHealthWorker::new("node".to_string())
            .with_probe(Box::new(FakeProbe::all()))
            .with_tick(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
