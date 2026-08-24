//! KVM-HEALTH (MV-2) — per-node KVM virtualization stack health worker.
//!
//! The Fedora + KVM successor to the retired `xcpng_health` worker. Runs on
//! **every** mesh node — the KVM stack is universal (one identical libvirt +
//! Podman set on Lighthouse and Workstation alike;
//! `docs/design/mesh-virt-management.md`: "same stack on every machine"), so
//! unlike `xcpng_health` (pinned to the dead `Xcpng` role) it is gated at the
//! `run_serve` spawn site through the rank-0-default worker resolver, i.e. it
//! runs everywhere. Each tick it probes every service in the canonical
//! [`crate::kvm::KVM_SERVICES`] catalog (`systemctl is-active <unit>`, with
//! Fedora modular-unit alternatives) and
//! publishes a whole-host health summary to the [`SERVICES_TOPIC`]
//! (`event/kvm/services`) Mackes-Bus topic, so the Workbench Datacenter view +
//! the alert lane see the live stack state without each consumer re-probing.
//!
//! The decision is the pure [`decide`] fn folding the catalog + a
//! [`ServiceProbe`] into a [`KvmHealth`] summary — unit-tested with a fake
//! probe. `tick_once` is the thin shell: the production [`SystemctlProbe`] seam
//! plus a fresh in-process Bus publication transaction, so the tested core
//! never touches systemd or the Bus.
//!
//! perf-10: the publish path used to fork+exec the `mde-bus` CLI once per tick
//! (a whole process + a fresh SQLite open + a [`crate::proc_reap`] reaper
//! thread). It now writes directly through [`Persist`] — byte-identical stored
//! rows, no spawn, no reaper, and no process-lifetime failure latch.

#![cfg(feature = "async-services")]

use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::kvm::{KvmService, KVM_SERVICES};

use super::{ShutdownToken, Worker};

/// 30 s tick — the virtualization stack is slow-changing (a daemon down is a
/// rare, operator-visible event), and a 30 s summary keeps the probe cheap (one
/// bounded `systemctl is-active` per catalog service) while staying fresh enough
/// for a host panel.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Bus topic the whole-host KVM stack health summary is published to.
pub const SERVICES_TOPIC: &str = "event/kvm/services";

/// Credential-free readiness projection consumed by the Workers virtualization provider.
pub const VIRTUALIZATION_PROVIDER_TOPIC: &str = "event/provider/virtualization";

const MAX_PROBE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualizationReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualizationProviderSnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub observed_unix_ms: u64,
    pub readiness: VirtualizationReadiness,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KvmDeviceFact {
    CharacterDevice,
    Missing,
    Substituted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitFact {
    Active,
    Inactive,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceFact {
    Active,
    Inactive,
    Missing,
}

fn classify_virtualization(
    device: Option<KvmDeviceFact>,
    kernel_module: Option<bool>,
    libvirt: Option<UnitFact>,
    connection: Option<bool>,
    network: Option<ResourceFact>,
    pool: Option<ResourceFact>,
) -> (VirtualizationReadiness, &'static str) {
    let (
        Some(device),
        Some(kernel_module),
        Some(libvirt),
        Some(connection),
        Some(network),
        Some(pool),
    ) = (device, kernel_module, libvirt, connection, network, pool)
    else {
        return (
            VirtualizationReadiness::Unknown,
            "virtualization facts unavailable or malformed",
        );
    };
    if device == KvmDeviceFact::Substituted {
        return (
            VirtualizationReadiness::Unknown,
            "/dev/kvm is not a character device",
        );
    }
    if (device == KvmDeviceFact::CharacterDevice) != kernel_module {
        return (
            VirtualizationReadiness::Unknown,
            "kernel KVM facts contradict each other",
        );
    }
    if libvirt != UnitFact::Active && connection {
        return (
            VirtualizationReadiness::Unknown,
            "inactive libvirt unit contradicts a live connection",
        );
    }
    if !connection && matches!(network, ResourceFact::Active) {
        return (
            VirtualizationReadiness::Unknown,
            "active libvirt network contradicts connection state",
        );
    }
    if !connection && matches!(pool, ResourceFact::Active) {
        return (
            VirtualizationReadiness::Unknown,
            "active libvirt pool contradicts connection state",
        );
    }
    if device == KvmDeviceFact::Missing
        && !kernel_module
        && libvirt == UnitFact::Disabled
        && !connection
        && network == ResourceFact::Missing
        && pool == ResourceFact::Missing
    {
        return (
            VirtualizationReadiness::Disabled,
            "KVM and libvirt are explicitly unavailable",
        );
    }
    if device == KvmDeviceFact::CharacterDevice
        && libvirt == UnitFact::Active
        && connection
        && network == ResourceFact::Active
        && pool == ResourceFact::Active
    {
        return (
            VirtualizationReadiness::Ready,
            "KVM, libvirt, network, and storage facts agree",
        );
    }
    (
        VirtualizationReadiness::Disconnected,
        "virtualization is present but not fully connected",
    )
}

fn bounded_output(command: Command) -> Option<String> {
    let output = crate::workers::proc::output_with_timeout(
        command,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() || output.stdout.len() > MAX_PROBE_BYTES {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    (!text.contains('\0')).then_some(text)
}

fn parse_unit(raw: &str) -> Option<UnitFact> {
    let mut active = None;
    let mut enabled = None;
    for line in raw.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "ActiveState" if active.replace(value).is_none() => {}
            "UnitFileState" if enabled.replace(value).is_none() => {}
            _ => return None,
        }
    }
    match (active?, enabled?) {
        ("active", _) => Some(UnitFact::Active),
        ("inactive" | "failed", "disabled" | "masked") => Some(UnitFact::Disabled),
        ("inactive" | "failed", "enabled" | "static" | "indirect") => Some(UnitFact::Inactive),
        _ => None,
    }
}

fn parse_resource(raw: &str) -> Option<ResourceFact> {
    match raw.trim() {
        "active" => Some(ResourceFact::Active),
        "inactive" => Some(ResourceFact::Inactive),
        "missing" => Some(ResourceFact::Missing),
        _ => None,
    }
}

fn probe_resource(kind: &str) -> Option<ResourceFact> {
    probe_named_resource(kind, "default")
}

/// Storage pools the provider accepts. `mde-vms` is the managed node-virt
/// pool; `default` and `images` are the libvirt/dir compatibility names.
const STORAGE_POOL_CANDIDATES: &[&str] = &["mde-vms", "default", "images"];

fn probe_storage_pool() -> Option<ResourceFact> {
    let mut best = None;
    for name in STORAGE_POOL_CANDIDATES {
        match probe_named_resource("pool-info", name) {
            Some(ResourceFact::Active) => return Some(ResourceFact::Active),
            Some(other) => best = Some(other),
            None => {}
        }
    }
    best
}

fn probe_named_resource(kind: &str, name: &str) -> Option<ResourceFact> {
    let mut command = Command::new("virsh");
    command.args(["-c", "qemu:///system", kind, name]);
    let output = crate::workers::proc::output_with_timeout(
        command,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .ok()?;
    if output.stdout.len() > MAX_PROBE_BYTES || output.stderr.len() > MAX_PROBE_BYTES {
        return None;
    }
    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).ok()?;
        return (stderr.contains("not found")
            || stderr.contains("no network")
            || stderr.contains("no storage pool"))
        .then_some(ResourceFact::Missing);
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let state = text.lines().find_map(|line| {
        line.split_once(':').and_then(|(key, value)| {
            let key = key.trim();
            let value = value.trim();
            (key == "Active" || key == "State").then_some(if value == "yes" || value == "running" {
                "active"
            } else if value == "no" || value == "inactive" {
                "inactive"
            } else {
                "malformed"
            })
        })
    })?;
    parse_resource(state)
}

fn probe_unit_show(unit: &str) -> Option<UnitFact> {
    let mut systemctl = Command::new("systemctl");
    systemctl.args(["show", "--property=ActiveState,UnitFileState", unit]);
    bounded_output(systemctl).as_deref().and_then(parse_unit)
}

/// Fold monolithic `libvirtd` and Fedora modular `virtqemud` facts. Any active
/// candidate is enough; an enabled-but-down unit beats a disabled compatibility
/// unit so a virtqemud host is not reported Disabled just because `libvirtd`
/// stays masked.
fn fold_libvirt_unit_facts(facts: impl IntoIterator<Item = Option<UnitFact>>) -> Option<UnitFact> {
    let mut saw = false;
    let mut best = UnitFact::Disabled;
    for fact in facts.into_iter().flatten() {
        saw = true;
        match fact {
            UnitFact::Active => return Some(UnitFact::Active),
            UnitFact::Inactive => best = UnitFact::Inactive,
            UnitFact::Disabled => {}
        }
    }
    saw.then_some(best)
}

fn gather_libvirt_unit() -> Option<UnitFact> {
    let service = crate::kvm::find_by_id("libvirtd")?;
    fold_libvirt_unit_facts(service.probe_units().map(probe_unit_show))
}

fn gather_virtualization(node_id: &str, now_ms: u64) -> VirtualizationProviderSnapshot {
    let device = match std::fs::symlink_metadata("/dev/kvm") {
        Ok(metadata) if metadata.file_type().is_char_device() => {
            Some(KvmDeviceFact::CharacterDevice)
        }
        Ok(_) => Some(KvmDeviceFact::Substituted),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(KvmDeviceFact::Missing),
        Err(_) => None,
    };
    let kernel_module = Some(std::path::Path::new("/sys/module/kvm").is_dir());
    let libvirt = gather_libvirt_unit();
    let mut virsh = Command::new("virsh");
    virsh.args(["-c", "qemu:///system", "uri"]);
    let connection = bounded_output(virsh).and_then(|raw| match raw.trim() {
        "qemu:///system" => Some(true),
        _ => None,
    });
    let network = probe_resource("net-info");
    let pool = probe_storage_pool();
    let (readiness, reason) =
        classify_virtualization(device, kernel_module, libvirt, connection, network, pool);
    VirtualizationProviderSnapshot {
        schema_version: 1,
        node_id: node_id.to_owned(),
        observed_unix_ms: now_ms,
        readiness,
        reason: reason.to_owned(),
    }
}

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
    /// The active systemd unit selected from the catalog's probe candidates, or
    /// the primary unit when all candidates are inactive.
    pub unit: String,
    /// `true` when `systemctl is-active` reported a primary or alternative unit
    /// active.
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

/// Pure health decision: probe each catalog service's primary unit, then its
/// Fedora modular alternatives when needed, via the injected [`ServiceProbe`]
/// and fold the results into a [`KvmHealth`] summary. The probe is the only seam
/// to the outside, so this is fully unit-testable with a fake probe — no
/// systemd, no bus, no clock (`now_ms` is passed in).
#[must_use]
pub fn decide(
    host: &str,
    catalog: &[KvmService],
    probe: &dyn ServiceProbe,
    now_ms: u64,
) -> KvmHealth {
    let services: Vec<ServiceHealth> = catalog
        .iter()
        .map(|s| {
            let active_unit = s.probe_units().find(|unit| probe.is_active(unit));
            ServiceHealth {
                // `id` is the stable published identity. The selected systemd
                // unit may vary by libvirt packaging layout.
                id: s.id.to_string(),
                unit: active_unit.unwrap_or(s.unit).to_string(),
                active: active_unit.is_some(),
            }
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

/// Publish a health summary to [`SERVICES_TOPIC`] in-process (perf-10), byte-identical to the
/// `mde-bus publish <topic> --body-flag <json>` this worker used to fork+exec.
/// Failures remain visible to the run loop and retry on its bounded cadence.
fn publish(persist: &Persist, health: &KvmHealth) -> Result<(), String> {
    let body = serde_json::to_string(health).map_err(|error| error.to_string())?;
    persist
        .write(SERVICES_TOPIC, Priority::Default, None, Some(&body))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn publish_virtualization(
    persist: &Persist,
    snapshot: &VirtualizationProviderSnapshot,
) -> Result<(), String> {
    let body = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    persist
        .write(
            VIRTUALIZATION_PROVIDER_TOPIC,
            Priority::Default,
            None,
            Some(&body),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
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
    /// Explicit Bus root override. Production resolves current user/service
    /// storage on every transaction and falls back to the system spool.
    bus_root_override: Option<PathBuf>,
    bus_disabled: bool,
}

impl KvmHealthWorker {
    /// Construct with production defaults: the canonical catalog, the live
    /// `systemctl` probe, a 30 s tick, and the CLI-equivalent bus root. `host`
    /// is the publishing node identity stamped into each summary.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            catalog: KVM_SERVICES,
            probe: Box::new(SystemctlProbe),
            tick: DEFAULT_TICK_INTERVAL,
            bus_root_override: None,
            bus_disabled: false,
        }
    }

    /// Override the bus root (tests) — `None` makes the publish a no-op so a
    /// test never writes into the real `~/.local/share/mde/bus` store.
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_disabled = bus_root.is_none();
        self.bus_root_override = bus_root;
        self
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
    /// publish the summary through the (optional) long-lived bus handle.
    fn bus_root(&self) -> Option<PathBuf> {
        if self.bus_disabled {
            return None;
        }
        Some(
            self.bus_root_override
                .clone()
                .or_else(crate::bus_publish::default_bus_root)
                .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)),
        )
    }

    fn tick_once(&self) -> Result<(), String> {
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
        let Some(root) = self.bus_root() else {
            return Ok(());
        };
        let persist = Persist::open(root).map_err(|error| error.to_string())?;
        publish(&persist, &health)?;
        publish_virtualization(
            &persist,
            &gather_virtualization(&self.host, health.published_at_ms),
        )
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
        if let Err(error) = self.tick_once() {
            tracing::debug!(target: "mackesd::bus_publish", %error, "kvm_health publication deferred");
        }
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.tick) => {
                    if let Err(error) = self.tick_once() {
                        tracing::debug!(target: "mackesd::bus_publish", %error, "kvm_health publication deferred");
                    }
                },
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

    #[test]
    fn hostile_virtualization_facts_fail_unknown() {
        let substituted = classify_virtualization(
            Some(KvmDeviceFact::Substituted),
            Some(true),
            Some(UnitFact::Active),
            Some(true),
            Some(ResourceFact::Active),
            Some(ResourceFact::Active),
        );
        assert_eq!(substituted.0, VirtualizationReadiness::Unknown);

        let contradictory_kernel = classify_virtualization(
            Some(KvmDeviceFact::CharacterDevice),
            Some(false),
            Some(UnitFact::Active),
            Some(true),
            Some(ResourceFact::Active),
            Some(ResourceFact::Active),
        );
        assert_eq!(contradictory_kernel.0, VirtualizationReadiness::Unknown);

        let contradictory_service = classify_virtualization(
            Some(KvmDeviceFact::CharacterDevice),
            Some(true),
            Some(UnitFact::Disabled),
            Some(true),
            Some(ResourceFact::Active),
            Some(ResourceFact::Active),
        );
        assert_eq!(contradictory_service.0, VirtualizationReadiness::Unknown);

        let malformed = parse_unit("ActiveState=active\nUnitFileState=enabled\nInjected=x\n");
        assert_eq!(malformed, None);
        assert_eq!(parse_resource("active\nsecret"), None);

        let ready = classify_virtualization(
            Some(KvmDeviceFact::CharacterDevice),
            Some(true),
            Some(UnitFact::Active),
            Some(true),
            Some(ResourceFact::Active),
            Some(ResourceFact::Active),
        );
        assert_eq!(ready.0, VirtualizationReadiness::Ready);
        assert_eq!(
            fold_libvirt_unit_facts([
                Some(UnitFact::Disabled),
                Some(UnitFact::Active),
                Some(UnitFact::Inactive),
            ]),
            Some(UnitFact::Active),
            "Fedora virtqemud must satisfy the provider even when libvirtd is disabled"
        );
        assert_eq!(
            fold_libvirt_unit_facts([Some(UnitFact::Disabled), Some(UnitFact::Inactive)]),
            Some(UnitFact::Inactive)
        );
        assert_eq!(
            fold_libvirt_unit_facts([Some(UnitFact::Disabled), None]),
            Some(UnitFact::Disabled)
        );
        assert_eq!(fold_libvirt_unit_facts([None, None]), None);
        let disabled = classify_virtualization(
            Some(KvmDeviceFact::Missing),
            Some(false),
            Some(UnitFact::Disabled),
            Some(false),
            Some(ResourceFact::Missing),
            Some(ResourceFact::Missing),
        );
        assert_eq!(disabled.0, VirtualizationReadiness::Disabled);
    }
    use std::cell::RefCell;
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

    /// Probe fake that records candidate order, proving fallback stops at the
    /// first active unit instead of probing every alias unconditionally.
    struct RecordingProbe {
        active: BTreeSet<String>,
        calls: RefCell<Vec<String>>,
    }

    impl RecordingProbe {
        fn with(units: &[&str]) -> Self {
            Self {
                active: units.iter().map(|u| (*u).to_string()).collect(),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ServiceProbe for RecordingProbe {
        fn is_active(&self, unit: &str) -> bool {
            self.calls.borrow_mut().push(unit.to_string());
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
    fn decide_falls_back_to_modular_units_and_keeps_canonical_ids() {
        let probe = RecordingProbe::with(&[
            "virtqemud.service",
            "podman.socket",
            "NetworkManager.service",
            "virtnetworkd.service",
            "virtstoraged.service",
        ]);
        let h = decide("fedora-modular", KVM_SERVICES, &probe, 8);

        assert!(h.all_healthy);
        assert_eq!(
            h.services.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec![
                "libvirtd",
                "podman",
                "network-manager",
                "libvirt-network",
                "libvirt-storage",
            ]
        );
        assert_eq!(
            h.services
                .iter()
                .map(|s| s.unit.as_str())
                .collect::<Vec<_>>(),
            vec![
                "virtqemud.service",
                "podman.socket",
                "NetworkManager.service",
                "virtnetworkd.service",
                "virtstoraged.service",
            ]
        );
        assert_eq!(
            probe.calls.into_inner(),
            vec![
                "libvirtd.service",
                "virtqemud.service",
                "podman.socket",
                "NetworkManager.service",
                "libvirtd.service",
                "virtnetworkd.service",
                "libvirtd.service",
                "virtstoraged.service",
            ]
        );
    }

    #[test]
    fn decide_accepts_socket_activated_modular_libvirt() {
        let probe = RecordingProbe::with(&[
            "virtqemud.socket",
            "virtnetworkd.socket",
            "virtstoraged.socket",
        ]);
        let health = decide("fedora-socket-activated", KVM_SERVICES, &probe, 9);

        assert_eq!(health.active, 3);
        assert_eq!(
            health
                .services
                .iter()
                .map(|service| (service.id.as_str(), service.unit.as_str(), service.active))
                .collect::<Vec<_>>(),
            vec![
                ("libvirtd", "virtqemud.socket", true),
                ("podman", "podman.socket", false),
                ("network-manager", "NetworkManager.service", false),
                ("libvirt-network", "virtnetworkd.socket", true),
                ("libvirt-storage", "virtstoraged.socket", true),
            ]
        );
    }

    #[test]
    fn decide_prefers_legacy_unit_when_both_layouts_are_active() {
        let probe = RecordingProbe::with(&["libvirtd.service", "virtqemud.service"]);
        let service = crate::kvm::find_by_id("libvirtd").expect("present in catalog");
        let h = decide("legacy-preferred", std::slice::from_ref(service), &probe, 9);

        assert!(h.all_healthy);
        assert_eq!(h.services[0].id, "libvirtd");
        assert_eq!(h.services[0].unit, "libvirtd.service");
        assert_eq!(probe.calls.into_inner(), vec!["libvirtd.service"]);
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
        // no systemd, no bus store (`with_bus_root(None)` makes publish a
        // swallowed no-op so the test never touches the real store).
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = KvmHealthWorker::new("node".to_string())
            .with_probe(Box::new(FakeProbe::all()))
            .with_tick(Duration::from_millis(10))
            .with_bus_root(None);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn tick_publishes_cli_equivalent_row_in_process() {
        // perf-10 — drive the worker against a temp bus root and confirm the
        // in-process publish stored EXACTLY the row a
        // `mde-bus publish event/kvm/services --body-flag <json>` would: the
        // topic, default priority, no title/actions/reply, and a body that is
        // the compact `serde_json` of the summary (what `--body-flag` carried).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = KvmHealthWorker::new("node-z".to_string())
            .with_probe(Box::new(FakeProbe::all()))
            // Long tick so only the immediate start publish fires — exactly one row.
            .with_tick(Duration::from_secs(3600))
            .with_bus_root(Some(root.clone()));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        tx.send(true).expect("signal shutdown");
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

        // Read the stored row back through a fresh handle, as any consumer does.
        let persist = Persist::open(root).expect("reopen bus");
        let rows = persist.list_since(SERVICES_TOPIC, None).expect("list");
        assert_eq!(
            rows.len(),
            1,
            "exactly the start-of-run summary was published"
        );
        let row = &rows[0];
        assert_eq!(row.topic, SERVICES_TOPIC);
        assert_eq!(row.priority, "default");
        assert!(row.title.is_none());
        assert!(row.actions.is_empty());
        assert!(row.reply_to.is_none());

        // The stored body is byte-identical to the compact serialization a CLI
        // publish would carry, and decodes back to the expected summary.
        let body = row.body.as_deref().expect("body present");
        let summary: KvmHealth = serde_json::from_str(body).expect("decode summary");
        assert_eq!(summary.host, "node-z");
        assert!(summary.all_healthy);
        assert_eq!(summary.active, summary.total);
        assert_eq!(summary.total, KVM_SERVICES.len());
        // Re-serializing the decoded summary reproduces the stored body exactly —
        // proving the row carries the same compact JSON `--body-flag` did.
        assert_eq!(serde_json::to_string(&summary).unwrap(), body);
    }

    #[tokio::test]
    async fn worker_recovers_late_and_replaced_bus_without_restart() {
        let fixture = tempfile::tempdir().expect("KVM Bus recovery fixture");
        let root = fixture.path().join("bus");
        std::fs::write(&root, b"block initial Bus open").expect("block Bus");
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut worker = KvmHealthWorker::new("node-recovery".to_string())
            .with_probe(Box::new(FakeProbe::all()))
            .with_tick(Duration::from_millis(10))
            .with_bus_root(Some(root.clone()));
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !task.is_finished(),
            "late Bus must not terminate the worker"
        );
        std::fs::remove_file(&root).expect("remove blocker");
        let late = Persist::open(root.clone()).expect("install late Bus");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if late
                    .read_latest(SERVICES_TOPIC)
                    .expect("read late health")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("late Bus receives health");
        drop(late);

        let replacement_root = fixture.path().join("replacement");
        Persist::open(replacement_root.clone()).expect("prepare replacement");
        std::fs::rename(&root, fixture.path().join("retired")).expect("retire Bus");
        std::fs::rename(&replacement_root, &root).expect("install replacement");
        let replacement = Persist::open(root).expect("open replacement");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if replacement
                    .read_latest(SERVICES_TOPIC)
                    .expect("read replacement health")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("replacement Bus receives health");

        tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("prompt shutdown")
            .expect("join")
            .expect("worker");
    }
}
