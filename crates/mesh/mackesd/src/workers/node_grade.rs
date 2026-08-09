//! System and Mesh Health authority.
//!
//! The historical worker name is retained only as the supervisor identity. Its
//! output is the versioned health contract in `mackes-mesh-types`; there is no
//! second grade shape or score ledger in this crate.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::device_inventory::{self, DeviceInventory, DeviceStatus};
use mackes_mesh_types::health::{
    action_result_topic, fold_snapshot, node_health_topic, GradeFactors, HealthAction,
    HealthActionOutcome, HealthActionRequest, HealthActionResult, HealthComponent, HealthCondition,
    HealthEvidence, HealthRemediation, HealthScope, HealthSeverity, NodeGrade, NodeHealthState,
    RequirementClass, SystemMeshHealthSnapshot, ACTION_TOPIC, CRITICAL_NOTIFY_TOPIC,
    HEALTH_SCHEMA_VERSION, SNAPSHOT_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(10);
// Keep identical daemons from starting their first expensive health sample in
// lockstep. The phase is stable across restarts and short enough that it cannot
// consume a meaningful part of either the node-row or folded-snapshot freshness
// window.
const MAX_INITIAL_PHASE_MS: u64 = 1_500;
// Node rows cross the mesh through Syncthing. Its normal fallback rescan is 60s
// on seats where the shared mount cannot deliver a reliable watcher event, so
// a validity shorter than one scan produces periodic phantom missing-publisher
// warnings even while every daemon is publishing every 10s. Cover two scans;
// the row still carries its exact observation timestamp and fails stale within
// two minutes when a publisher genuinely disappears.
const PUBLICATION_VALIDITY_MS: u64 = 120_000;
const SNAPSHOT_VALIDITY_MS: u64 = 30_000;
const SUSTAINED_SAMPLES: usize = 3;
const MAX_HEALTH_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MESH_STATUS_PATH: &str = "/run/mde/mesh-status.json";
const DEVICE_INVENTORY_VALIDITY_MS: u64 = 10 * 60 * 1_000;
const AUDIO_PROOF_PATH: &str = "/var/lib/mackesd/health/audio-proof.json";
// Audio discovery shells out through runuser and several user-session tools.
// Keep the health contract fresh without repeating that fork-heavy probe on
// every 10-second node-grade sample. A failed probe is cached too, preventing
// a broken user session from becoming a synchronized process storm.
const AUDIO_PROBE_TTL: Duration = Duration::from_secs(60);

// Keep the user-session audio evidence probe to one runuser/PAM transition.
// The individual commands remain real providers, but launching each one
// through runuser separately made every workstation spend a short burst of
// CPU in the health worker on every cache refresh.
const AUDIO_PROBE_SCRIPT: &str = r#"
graph=0
if systemctl --user is-active --quiet pipewire.service && pw-cli ls Node >/dev/null 2>&1; then graph=1; fi
wireplumber=0
if systemctl --user is-active --quiet wireplumber.service && wpctl status >/dev/null 2>&1; then wireplumber=1; fi
pulse=0
if systemctl --user is-active --quiet pipewire-pulse.service && pactl info 2>/dev/null | grep -qi pipewire; then pulse=1; fi
playback=0
if pactl list short sinks 2>/dev/null | grep -q '[^[:space:]]'; then playback=1; fi
capture=0
if pactl list short sources 2>/dev/null | grep -vi monitor | grep -q '[^[:space:]]'; then capture=1; fi
printf 'graph=%s pulse=%s wireplumber=%s playback=%s capture=%s\n' "$graph" "$pulse" "$wireplumber" "$playback" "$capture"
"#;

/// Derive a stable, bounded startup phase from the node identity. A pure
/// function keeps the scheduling contract testable without starting a worker or
/// touching the health substrate.
fn initial_phase_for(hostname: &str) -> Duration {
    if hostname.is_empty() {
        return Duration::ZERO;
    }
    let hash = hostname.bytes().fold(2_166_136_261_u32, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    });
    Duration::from_millis(u64::from(hash) % (MAX_INITIAL_PHASE_MS + 1))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ResourceObservation {
    cpu_load_ratio: Option<f32>,
    memory_available_pct: Option<f32>,
    root_used_pct: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuCounters {
    busy: u64,
    total: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct MeshNode {
    hostname: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    presence: Option<String>,
    #[serde(default)]
    services: BTreeMap<String, bool>,
    /// Optional services become required only when provisioning assigns the
    /// matching capability to this node. The alias accepts the concise field
    /// used by older roster publishers without creating a second wire shape.
    #[serde(default, alias = "capabilities")]
    assigned_capabilities: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct MeshStatus {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    nodes: Vec<MeshNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AudioProof {
    boot_id: String,
    observed_at_ms: u64,
    playback: bool,
    capture_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct AudioObservation {
    #[serde(default)]
    pipewire_graph: bool,
    #[serde(default)]
    pipewire_pulse: bool,
    #[serde(default)]
    wireplumber_policy: bool,
    #[serde(default)]
    playback: bool,
    #[serde(default)]
    capture: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct HealthObservations {
    role: String,
    roster_revision: String,
    canonical_nodes: BTreeSet<String>,
    services: BTreeMap<String, bool>,
    assigned_capabilities: BTreeSet<String>,
    resources: ResourceObservation,
    audio: Option<AudioObservation>,
    mesh_snapshot_present: bool,
    overlay_up: bool,
    reachable_lighthouses: usize,
    firmware_refresh_failed: bool,
    device_inventory: Option<DeviceInventory>,
}

trait HealthSampler: Send {
    fn sample(&self) -> HealthObservations;
}

struct SystemSampler {
    host: String,
    role: String,
    workgroup_root: PathBuf,
    audio_cache: Mutex<Option<(Instant, Option<AudioObservation>)>>,
}

impl SystemSampler {
    #[must_use]
    fn new(host: String, workgroup_root: PathBuf, role_rank: u8) -> Self {
        Self {
            host,
            workgroup_root,
            role: if role_rank == 0 {
                "lighthouse"
            } else {
                "workstation"
            }
            .into(),
            audio_cache: Mutex::new(None),
        }
    }

    fn read_mesh_status(&self) -> Option<MeshStatus> {
        let body = std::fs::read_to_string(MESH_STATUS_PATH).ok()?;
        serde_json::from_str(&body).ok()
    }

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let mut command = Command::new(program);
        command.args(args);
        let output =
            super::proc::output_with_timeout(command, super::proc::DEFAULT_CMD_TIMEOUT).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn resources() -> ResourceObservation {
        let cpu_load_ratio = cpu_busy_ratio(Duration::from_millis(250));
        let memory_available_pct = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|body| parse_mem_available_pct(&body));
        let root_used_pct = Self::run("df", &["-P", "/"])
            .as_deref()
            .and_then(parse_df_used_pct);
        ResourceObservation {
            cpu_load_ratio,
            memory_available_pct,
            root_used_pct,
        }
    }

    fn run_as_mm(program: &str, args: &[&str]) -> Option<String> {
        let uid = Self::run("id", &["-u", "mm"])?;
        let runtime = format!("XDG_RUNTIME_DIR=/run/user/{}", uid.trim());
        let mut command = Command::new("runuser");
        command.args(["-u", "mm", "--", "env", runtime.as_str(), program]);
        command.args(args);
        let output =
            super::proc::output_with_timeout(command, super::proc::DEFAULT_CMD_TIMEOUT).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn audio() -> Option<AudioObservation> {
        Self::run("id", &["-u", "mm"])?;
        let probe = Self::run_as_mm("bash", &["-lc", AUDIO_PROBE_SCRIPT])?;
        let mut observation = parse_audio_probe(&probe)?;
        let proof = read_current_audio_proof();
        observation.playback &= proof.as_ref().is_some_and(|proof| proof.playback);
        observation.capture &= proof
            .as_ref()
            .is_some_and(|proof| proof.capture_bytes >= 192_044);
        Some(observation)
    }

    fn audio_cached(&self) -> Option<AudioObservation> {
        if let Ok(cache) = self.audio_cache.lock() {
            if let Some((observed_at, value)) = cache.as_ref() {
                if observed_at.elapsed() < AUDIO_PROBE_TTL {
                    return value.clone();
                }
            }
        }

        let value = Self::audio();
        if let Ok(mut cache) = self.audio_cache.lock() {
            *cache = Some((Instant::now(), value.clone()));
        }
        value
    }
}

fn parse_audio_probe(body: &str) -> Option<AudioObservation> {
    let values = body
        .split_whitespace()
        .filter_map(|field| field.split_once('='))
        .collect::<BTreeMap<_, _>>();
    let admitted = |name: &str| match values.get(name).copied()? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    Some(AudioObservation {
        pipewire_graph: admitted("graph")?,
        pipewire_pulse: admitted("pulse")?,
        wireplumber_policy: admitted("wireplumber")?,
        playback: admitted("playback")?,
        capture: admitted("capture")?,
    })
}

fn cpu_busy_ratio(interval: Duration) -> Option<f32> {
    let before = parse_cpu_counters(&std::fs::read_to_string("/proc/stat").ok()?)?;
    std::thread::sleep(interval);
    let after = parse_cpu_counters(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let total = after.total.checked_sub(before.total)?;
    let busy = after.busy.checked_sub(before.busy)?;
    (total > 0).then(|| busy as f32 / total as f32)
}

fn parse_cpu_counters(body: &str) -> Option<CpuCounters> {
    let mut fields = body.lines().next()?.split_whitespace();
    (fields.next()? == "cpu").then_some(())?;
    let values: Vec<u64> = fields
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (values.len() >= 5).then_some(())?;
    let total = values
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))?;
    let idle = values[3].checked_add(values[4])?;
    Some(CpuCounters {
        busy: total.checked_sub(idle)?,
        total,
    })
}

fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_current_audio_proof() -> Option<AudioProof> {
    let proof: AudioProof = serde_json::from_slice(&std::fs::read(AUDIO_PROOF_PATH).ok()?).ok()?;
    (Some(proof.boot_id.as_str()) == boot_id().as_deref()).then_some(proof)
}

impl HealthSampler for SystemSampler {
    fn sample(&self) -> HealthObservations {
        let mesh = self.read_mesh_status();
        let local = mesh.as_ref().and_then(|snapshot| {
            snapshot
                .nodes
                .iter()
                .find(|node| node.hostname == self.host)
        });
        let canonical_nodes: BTreeSet<_> = mesh
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.role.as_deref() != Some("lighthouse"))
                    .map(|node| node.hostname.clone())
                    .collect()
            })
            .unwrap_or_else(|| BTreeSet::from([self.host.clone()]));
        let reachable_lighthouses = mesh.as_ref().map_or(0, |snapshot| {
            snapshot
                .nodes
                .iter()
                .filter(|node| {
                    node.role.as_deref() == Some("lighthouse")
                        && !matches!(node.presence.as_deref(), Some("offline" | "unreachable"))
                })
                .count()
        });
        HealthObservations {
            role: local
                .and_then(|node| node.role.clone())
                .unwrap_or_else(|| self.role.clone()),
            roster_revision: mesh
                .as_ref()
                .and_then(|snapshot| snapshot.revision.clone())
                .unwrap_or_else(|| "unavailable".into()),
            canonical_nodes,
            services: local.map(|node| node.services.clone()).unwrap_or_default(),
            assigned_capabilities: local
                .map(|node| node.assigned_capabilities.clone())
                .unwrap_or_default(),
            resources: Self::resources(),
            audio: (self.role == "workstation")
                .then(|| self.audio_cached())
                .flatten(),
            mesh_snapshot_present: mesh.is_some(),
            overlay_up: local.is_some_and(|node| {
                !matches!(node.presence.as_deref(), Some("offline" | "unreachable"))
            }),
            reachable_lighthouses,
            firmware_refresh_failed: Command::new("systemctl")
                .args(["is-failed", "--quiet", "fwupd-refresh.service"])
                .status()
                .is_ok_and(|status| status.success()),
            device_inventory: device_inventory::read_inventory(&self.workgroup_root, &self.host),
        }
    }
}

#[must_use]
fn parse_mem_available_pct(body: &str) -> Option<f32> {
    let value = |key: &str| {
        body.lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<f32>().ok())
    };
    let total = value("MemTotal:")?;
    let available = value("MemAvailable:").or_else(|| value("MemFree:"))?;
    (total > 0.0).then_some(available / total * 100.0)
}

#[must_use]
fn parse_df_used_pct(body: &str) -> Option<f32> {
    body.lines()
        .nth(1)?
        .split_whitespace()
        .nth(4)?
        .trim_end_matches('%')
        .parse()
        .ok()
}

#[derive(Debug, Default)]
struct PressureWindow {
    cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
}

impl PressureWindow {
    fn observe(&mut self, resources: ResourceObservation) {
        if let Some(value) = resources.cpu_load_ratio {
            push_bounded(&mut self.cpu, value);
        }
        if let Some(value) = resources.memory_available_pct {
            push_bounded(&mut self.memory, value);
        }
    }

    #[must_use]
    fn sustained_cpu(&self, threshold: f32) -> bool {
        self.cpu.len() == SUSTAINED_SAMPLES && self.cpu.iter().all(|value| *value >= threshold)
    }

    #[must_use]
    fn sustained_memory_below(&self, threshold: f32) -> bool {
        self.memory.len() == SUSTAINED_SAMPLES
            && self.memory.iter().all(|value| *value <= threshold)
    }
}

fn push_bounded(queue: &mut VecDeque<f32>, value: f32) {
    if queue.len() == SUSTAINED_SAMPLES {
        queue.pop_front();
    }
    queue.push_back(value);
}

#[must_use]
fn required_services(role: &str, assigned_capabilities: &BTreeSet<String>) -> Vec<&'static str> {
    let mut services = if role == "workstation" {
        vec![
            "mackesd",
            "nebula",
            "sync",
            "bus",
            "dns",
            "kdc",
            "workbench",
        ]
    } else {
        vec!["mackesd", "nebula", "sync", "bus", "dns"]
    };
    for (capability, service) in [("voice", "voice"), ("music", "music")] {
        if assigned_capabilities.contains(capability) {
            services.push(service);
        }
    }
    services
}

fn is_seat15(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "seat15" || host == "seat-15" || host.contains("basement")
}

fn evidence(provider: &str, summary: impl Into<String>, now_ms: u64) -> HealthEvidence {
    HealthEvidence {
        provider: provider.into(),
        summary: summary.into(),
        facts: BTreeMap::new(),
        observed_at_ms: now_ms,
    }
}

fn condition(
    host: &str,
    id: &str,
    component: HealthComponent,
    source: &str,
    severity: HealthSeverity,
    summary: impl Into<String>,
    now_ms: u64,
    remediation: Vec<HealthRemediation>,
) -> HealthCondition {
    HealthCondition {
        id: format!("{host}:{id}"),
        scope: HealthScope::Node { node: host.into() },
        component,
        source: source.into(),
        severity,
        requirement: RequirementClass::Required,
        evidence: evidence(source, summary, now_ms),
        active_since_ms: now_ms,
        last_observed_ms: now_ms,
        resolved_at_ms: None,
        acknowledged_at_ms: None,
        snoozed_until_ms: None,
        remediation,
    }
}

fn remediation(
    host: &str,
    action: HealthAction,
    generation: u64,
    impact: &str,
    confirmation_required: bool,
) -> HealthRemediation {
    HealthRemediation {
        action,
        target: HealthScope::Node { node: host.into() },
        expected_snapshot_generation: generation,
        impact: impact.into(),
        confirmation_required,
        workspace_route: None,
    }
}

fn restart_action(service: &str) -> Option<HealthAction> {
    match service {
        "mackesd" => Some(HealthAction::RestartMackesd),
        "nebula" => Some(HealthAction::RestartNebula),
        "sync" => Some(HealthAction::RestartSyncthing),
        "bus" => Some(HealthAction::RestartMeshBus),
        "dns" => Some(HealthAction::RestartDns),
        "kdc" => Some(HealthAction::RestartKdc),
        "workbench" => Some(HealthAction::RestartShell),
        _ => None,
    }
}

/// Pure, role-aware policy evaluation. Optional voice/music are never consulted.
#[must_use]
fn evaluate_conditions(
    host: &str,
    observations: &HealthObservations,
    pressure: &PressureWindow,
    generation: u64,
    now_ms: u64,
) -> Vec<HealthCondition> {
    let mut conditions = Vec::new();
    if !observations.mesh_snapshot_present {
        conditions.push(condition(
            host,
            "mesh-evidence-missing",
            HealthComponent::Evidence,
            "mesh-status",
            HealthSeverity::Warning,
            "The authoritative mesh-status provider has not published current evidence.",
            now_ms,
            vec![remediation(
                host,
                HealthAction::RefreshProvider,
                generation,
                "Refresh the bounded mesh-status provider.",
                false,
            )],
        ));
    }
    for service in required_services(&observations.role, &observations.assigned_capabilities) {
        if observations.services.get(service) != Some(&true) {
            let action = restart_action(service).into_iter().map(|action| {
                remediation(
                    host,
                    action,
                    generation,
                    &format!("Restart the required {service} service."),
                    true,
                )
            });
            conditions.push(condition(
                host,
                &format!("required-service-{service}"),
                HealthComponent::System,
                "mesh-status/services",
                if matches!(service, "mackesd" | "nebula") {
                    HealthSeverity::Critical
                } else {
                    HealthSeverity::Warning
                },
                format!("Required service {service} is not available."),
                now_ms,
                action.collect(),
            ));
        }
    }
    if !observations.overlay_up {
        conditions.push(condition(
            host,
            "overlay-unreachable",
            HealthComponent::Mesh,
            "mesh-status",
            HealthSeverity::Critical,
            "This node is not reachable on the current mesh overlay.",
            now_ms,
            vec![remediation(
                host,
                HealthAction::RestartNebula,
                generation,
                "Restart Nebula on this node.",
                true,
            )],
        ));
    } else if observations.reachable_lighthouses == 0 {
        conditions.push(condition(
            host,
            "lighthouse-unreachable",
            HealthComponent::Mesh,
            "mesh-status",
            HealthSeverity::Warning,
            "No current lighthouse is reachable from this node.",
            now_ms,
            vec![remediation(
                host,
                HealthAction::RefreshProvider,
                generation,
                "Refresh current mesh reachability evidence.",
                false,
            )],
        ));
    }
    if let Some(used) = observations.resources.root_used_pct {
        if used >= 85.0 {
            let critical = used >= 95.0;
            let actions = is_seat15(host)
                .then(|| remediation(host, HealthAction::ExpandSeat15Root, generation, "Grow the 15 GiB root LV to exactly 30 GiB, preserving the remaining VG reserve.", true))
                .into_iter()
                .collect();
            conditions.push(condition(
                host,
                "root-space",
                HealthComponent::Resources,
                "root-filesystem",
                if critical {
                    HealthSeverity::Critical
                } else {
                    HealthSeverity::Warning
                },
                format!("Root filesystem utilization is {used:.0}% (policy threshold 85%)."),
                now_ms,
                actions,
            ));
        }
    } else {
        conditions.push(condition(
            host,
            "root-evidence-missing",
            HealthComponent::Evidence,
            "root-filesystem",
            HealthSeverity::Warning,
            "Root filesystem evidence is unavailable.",
            now_ms,
            vec![remediation(
                host,
                HealthAction::RefreshProvider,
                generation,
                "Refresh bounded resource evidence.",
                false,
            )],
        ));
    }
    if pressure.sustained_cpu(0.95) {
        conditions.push(condition(
            host,
            "cpu-pressure",
            HealthComponent::Resources,
            "proc-stat",
            if pressure.sustained_cpu(0.99) {
                HealthSeverity::Critical
            } else {
                HealthSeverity::Warning
            },
            "CPU pressure breached policy for three consecutive observations.",
            now_ms,
            Vec::new(),
        ));
    }
    if pressure.sustained_memory_below(10.0) {
        conditions.push(condition(
            host,
            "memory-pressure",
            HealthComponent::Resources,
            "proc-meminfo",
            if pressure.sustained_memory_below(5.0) {
                HealthSeverity::Critical
            } else {
                HealthSeverity::Warning
            },
            "Available memory breached policy for three consecutive observations.",
            now_ms,
            Vec::new(),
        ));
    }
    if observations.role == "workstation" {
        let audio_healthy = observations.audio.as_ref().is_some_and(|audio| {
            audio.pipewire_graph
                && audio.pipewire_pulse
                && audio.wireplumber_policy
                && audio.playback
                && audio.capture
        });
        if !audio_healthy {
            conditions.push(condition(host, "workstation-audio", HealthComponent::Audio, "mm-user-session", HealthSeverity::Warning, "Workstation audio lacks current PipeWire, pipewire-pulse, WirePlumber, playback, or capture endpoint evidence from the mm user session.", now_ms, vec![remediation(host, HealthAction::RestoreWorkstationAudio, generation, "Restore the mm user-session audio graph, then re-run playback and capture probes.", true)]));
        }
    }
    if observations.firmware_refresh_failed {
        conditions.push(condition(host, "firmware-refresh", HealthComponent::Firmware, "fwupd-refresh.service", HealthSeverity::Warning, "Firmware metadata refresh failed; the failure remains active until a successful refresh is observed.", now_ms, vec![remediation(host, HealthAction::RefreshFirmwareMetadata, generation, "Preflight the firmware provider and refresh metadata; corrupt regenerable metadata may be quarantined.", true)]));
    }
    match observations.device_inventory.as_ref() {
        None => conditions.push(condition(
            host,
            "device-evidence-missing",
            HealthComponent::Evidence,
            "device-inventory",
            HealthSeverity::Warning,
            "Required device inventory evidence is unavailable.",
            now_ms,
            vec![remediation(
                host,
                HealthAction::RefreshProvider,
                generation,
                "Refresh the bounded hardware inventory provider.",
                false,
            )],
        )),
        Some(inventory)
            if now_ms.saturating_sub(inventory.published_at_ms) > DEVICE_INVENTORY_VALIDITY_MS =>
        {
            conditions.push(condition(
                host,
                "device-evidence-stale",
                HealthComponent::Evidence,
                "device-inventory",
                HealthSeverity::Warning,
                "Required device inventory evidence is stale.",
                now_ms,
                vec![remediation(
                    host,
                    HealthAction::RefreshProvider,
                    generation,
                    "Refresh the bounded hardware inventory provider.",
                    false,
                )],
            ));
        }
        Some(inventory) => {
            for category in &inventory.categories {
                for device in &category.devices {
                    if !matches!(
                        device.status,
                        DeviceStatus::Disabled | DeviceStatus::Degraded
                    ) {
                        continue;
                    }
                    let stable_key = device
                        .sysfs_path
                        .as_deref()
                        .unwrap_or(&device.name)
                        .replace(['/', ':', ' '], "-");
                    let mut health = condition(
                        host,
                        &format!("device-{stable_key}"),
                        HealthComponent::Devices,
                        "device-inventory",
                        if device.status == DeviceStatus::Degraded {
                            HealthSeverity::Critical
                        } else {
                            HealthSeverity::Warning
                        },
                        device.problem.as_deref().map_or_else(
                            || format!("{} has a corroborated platform fault.", device.name),
                            |reason| format!("{}: {reason}", device.name),
                        ),
                        now_ms,
                        Vec::new(),
                    );
                    health
                        .evidence
                        .facts
                        .insert("device".into(), device.name.clone());
                    health
                        .evidence
                        .facts
                        .insert("category".into(), category.key.clone());
                    health.remediation.push(HealthRemediation {
                        action: HealthAction::RefreshProvider,
                        target: HealthScope::Node { node: host.into() },
                        expected_snapshot_generation: generation,
                        impact:
                            "Refresh corroborated device evidence before taking hardware action."
                                .into(),
                        confirmation_required: false,
                        workspace_route: Some(format!("device-manager?device={stable_key}")),
                    });
                    conditions.push(health);
                }
            }
        }
    }
    conditions
}

fn factor_from_headroom(
    value: Option<f32>,
    good: f32,
    fair: f32,
    higher_is_better: bool,
) -> Option<u8> {
    value.map(|value| {
        let good_value = if higher_is_better {
            value >= good
        } else {
            value <= good
        };
        let fair_value = if higher_is_better {
            value >= fair
        } else {
            value <= fair
        };
        if good_value {
            100
        } else if fair_value {
            90
        } else {
            80
        }
    })
}

fn factors(observations: &HealthObservations) -> GradeFactors {
    GradeFactors {
        cpu: factor_from_headroom(observations.resources.cpu_load_ratio, 0.50, 0.75, false),
        memory: factor_from_headroom(
            observations.resources.memory_available_pct,
            30.0,
            20.0,
            true,
        ),
        disk: factor_from_headroom(observations.resources.root_used_pct, 70.0, 85.0, false),
        system: Some(
            if required_services(&observations.role, &observations.assigned_capabilities)
                .iter()
                .all(|service| observations.services.get(*service) == Some(&true))
            {
                100
            } else {
                80
            },
        ),
        mesh: Some(
            if observations.overlay_up && observations.reachable_lighthouses > 0 {
                100
            } else {
                80
            },
        ),
        devices: observations.device_inventory.as_ref().map(|inventory| {
            if inventory
                .categories
                .iter()
                .flat_map(|category| &category.devices)
                .any(|device| {
                    matches!(
                        device.status,
                        DeviceStatus::Disabled | DeviceStatus::Degraded
                    )
                })
            {
                80
            } else {
                100
            }
        }),
    }
}

fn capability_score(factors: GradeFactors) -> u8 {
    let values = [
        factors.cpu,
        factors.memory,
        factors.disk,
        factors.system,
        factors.mesh,
        factors.devices,
    ];
    let mut sum = 0_u32;
    let mut count = 0_u32;
    for value in values.into_iter().flatten() {
        sum += u32::from(value);
        count += 1;
    }
    if count == 0 {
        70
    } else {
        u8::try_from((sum + count / 2) / count).unwrap_or(100)
    }
}

fn health_root(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("system-mesh-health")
}

fn node_dir(workgroup_root: &Path) -> PathBuf {
    health_root(workgroup_root).join("nodes")
}

#[must_use]
fn snapshot_path(workgroup_root: &Path, observer: &str) -> PathBuf {
    health_root(workgroup_root)
        .join("snapshots")
        .join(format!("{observer}.json"))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let body = serde_json::to_vec_pretty(value)?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("health")
    ));
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)
}

fn read_state(path: &Path) -> Option<NodeHealthState> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_HEALTH_FILE_BYTES
    {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[must_use]
fn read_canonical_states(workgroup_root: &Path) -> Vec<NodeHealthState> {
    let Ok(entries) = std::fs::read_dir(node_dir(workgroup_root)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.starts_with('.') || name.contains("sync-conflict") || !name.ends_with(".json") {
                return None;
            }
            let state = read_state(&entry.path())?;
            (name == format!("{}.json", state.publisher)).then_some(state)
        })
        .collect()
}

fn merge_lifecycle(
    previous: Option<&NodeHealthState>,
    current: &mut [HealthCondition],
    now_ms: u64,
) -> Vec<HealthCondition> {
    let mut resolved = previous.map_or_else(Vec::new, |state| state.resolved_conditions.clone());
    // Own the ids so the lifecycle pass can mutate `current` while it compares
    // the previous active set. Borrowing `&str` here would keep an immutable
    // borrow of the entire slice alive through `iter_mut()`.
    let current_ids: BTreeSet<_> = current
        .iter()
        .map(|condition| condition.id.clone())
        .collect();
    if let Some(previous) = previous {
        for condition in current.iter_mut() {
            if let Some(old) = previous
                .active_conditions
                .iter()
                .find(|old| old.id == condition.id)
            {
                condition.active_since_ms = old.active_since_ms;
                condition.acknowledged_at_ms = old.acknowledged_at_ms;
                condition.snoozed_until_ms = old.snoozed_until_ms;
            }
        }
        for old in &previous.active_conditions {
            if !current_ids.contains(old.id.as_str()) {
                let mut closed = old.clone();
                closed.resolved_at_ms = Some(now_ms);
                closed.last_observed_ms = now_ms;
                resolved.push(closed);
            }
        }
    }
    resolved.sort_by(|left, right| right.resolved_at_ms.cmp(&left.resolved_at_ms));
    resolved.truncate(128);
    resolved
}

fn emit_json<T: Serialize>(persist: &Persist, topic: &str, value: &T) {
    if let Ok(body) = serde_json::to_string(value) {
        let _ = persist.write(topic, Priority::Default, None, Some(&body));
    }
}

fn emit_critical(persist: &Persist, condition: &HealthCondition, host: &str) {
    let body = serde_json::json!({
        "severity": "critical",
        "source": "system-mesh-health",
        "summary": condition.evidence.summary,
        "condition_id": condition.id,
        "host": host,
        "ts_unix_ms": condition.last_observed_ms,
    });
    let _ = persist.write(
        CRITICAL_NOTIFY_TOPIC,
        Priority::Default,
        None,
        Some(&body.to_string()),
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionDecision {
    Apply,
    Refuse(&'static str),
    Stale,
}

#[must_use]
fn authorize_action(
    request: &HealthActionRequest,
    host: &str,
    snapshot: &SystemMeshHealthSnapshot,
) -> ActionDecision {
    if request.schema_version != HEALTH_SCHEMA_VERSION || request.request_id.is_empty() {
        return ActionDecision::Refuse("invalid schema or request id");
    }
    if request.requester != host || request.authorization != "local-seat" {
        return ActionDecision::Refuse("health remediation requires local-seat authorization");
    }
    if request.expected_snapshot_generation != snapshot.generation {
        return ActionDecision::Stale;
    }
    if !matches!(&request.target, HealthScope::Node { node } if node == host) {
        return ActionDecision::Refuse("target is not this node");
    }
    let Some(condition) = snapshot.active_conditions.iter().find(|condition| {
        condition.id == request.condition_id && condition.scope == request.target
    }) else {
        return ActionDecision::Refuse("condition is not active for target");
    };
    let offered = matches!(
        request.action,
        HealthAction::Acknowledge | HealthAction::SnoozeOneHour
    ) || condition
        .remediation
        .iter()
        .any(|action| action.action == request.action);
    if !offered {
        return ActionDecision::Refuse("action is not offered for this condition");
    }
    let confirmation_required = condition
        .remediation
        .iter()
        .find(|action| action.action == request.action)
        .is_some_and(|action| action.confirmation_required);
    if confirmation_required && request.confirmation.as_deref() != Some("CONFIRM") {
        return ActionDecision::Refuse("explicit confirmation is required");
    }
    ActionDecision::Apply
}

fn unit_for_action(action: HealthAction) -> Option<&'static str> {
    match action {
        HealthAction::RestartMackesd => Some("mackesd.service"),
        HealthAction::RestartNebula => Some("nebula.service"),
        HealthAction::RestartSyncthing => Some("syncthing.service"),
        HealthAction::RestartMeshBus => Some("mesh-broker-setup.service"),
        HealthAction::RestartDns => Some("mackesd.service"),
        HealthAction::RestartKdc => Some("mackesd.service"),
        HealthAction::RestartShell => Some("mde-shell-egui.service"),
        _ => None,
    }
}

fn execute_action(
    action: HealthAction,
    host: &str,
    source: &str,
    workgroup_root: &Path,
) -> Result<String, String> {
    if let Some(unit) = unit_for_action(action) {
        let status = Command::new("systemctl")
            .args(["restart", unit])
            .status()
            .map_err(|error| error.to_string())?;
        return status
            .success()
            .then(|| format!("restarted {unit}"))
            .ok_or_else(|| format!("restart of {unit} failed"));
    }
    match action {
        HealthAction::RefreshProvider if source == "device-inventory" => {
            super::device_inventory::publish_system(workgroup_root, host)
                .map(|_| "hardware inventory refreshed".into())
                .map_err(|error| error.to_string())
        }
        HealthAction::RefreshProvider => run_checked(
            "systemctl",
            &["start", "mesh-status.service"],
            "mesh-status provider refreshed",
        ),
        HealthAction::RestoreWorkstationAudio => restore_workstation_audio(),
        HealthAction::RefreshFirmwareMetadata => {
            run_checked(
                "fwupdmgr",
                &["get-remotes"],
                "firmware provider preflight passed",
            )?;
            run_checked(
                "fwupdmgr",
                &["refresh", "--force"],
                "firmware metadata refresh completed",
            )?;
            run_checked(
                "systemctl",
                &["reset-failed", "fwupd-refresh.service"],
                "firmware refresh failure cleared after successful refresh",
            )
        }
        HealthAction::ExpandSeat15Root => expand_seat15_root(host),
        HealthAction::Acknowledge | HealthAction::SnoozeOneHour => {
            Ok("condition state updated".into())
        }
        _ => Err("action has no executor".into()),
    }
}

fn restore_workstation_audio() -> Result<String, String> {
    let uid = SystemSampler::run("id", &["-u", "mm"])
        .map(|value| value.trim().to_string())
        .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
        .ok_or_else(|| "mm user id is unavailable".to_string())?;
    let runtime = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    let status = Command::new("runuser")
        .args([
            "-u",
            "mm",
            "--",
            "env",
            &runtime,
            "systemctl",
            "--user",
            "enable",
            "--now",
            "pipewire.service",
            "pipewire-pulse.service",
            "wireplumber.service",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("mm user-session audio enable/start failed".into());
    }
    prove_workstation_audio(&runtime, &uid)?;
    let healthy = SystemSampler::audio().is_some_and(|audio| {
        audio.pipewire_graph
            && audio.pipewire_pulse
            && audio.wireplumber_policy
            && audio.playback
            && audio.capture
    });
    healthy
        .then(|| "mm user-session audio services and playback/capture endpoints are current".into())
        .ok_or_else(|| "mm user-session audio endpoints remain unavailable after restart".into())
}

fn prove_workstation_audio(runtime: &str, uid: &str) -> Result<(), String> {
    let playback = Command::new("runuser")
        .args([
            "-u",
            "mm",
            "--",
            "env",
            runtime,
            "timeout",
            "12s",
            "speaker-test",
            "-D",
            "pipewire",
            "-t",
            "sine",
            "-f",
            "440",
            "-l",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if !playback.success() {
        return Err("direct PipeWire playback proof failed".into());
    }

    let (capture_id, capture_target) = discover_audio_capture_target(runtime)?;
    let set_default = Command::new("runuser")
        .args([
            "-u",
            "mm",
            "--",
            "env",
            runtime,
            "wpctl",
            "set-default",
            &capture_id,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if !set_default.success() {
        return Err("PipeWire capture source could not be selected".into());
    }

    let capture_path = format!(
        "/run/user/{uid}/mde-health-audio-capture-{}.wav",
        std::process::id()
    );
    let _capture_status = Command::new("runuser")
        .args([
            "-u",
            "mm",
            "--",
            "env",
            runtime,
            "timeout",
            "6s",
            "pw-record",
            "--media-category",
            "Capture",
            "--target",
            &capture_target,
            "-n",
            "96000",
            &capture_path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    let capture_bytes = std::fs::metadata(&capture_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let _ = std::fs::remove_file(&capture_path);
    if capture_bytes < 192_044 {
        return Err(format!(
            "direct PipeWire capture proof was incomplete ({capture_bytes} bytes)"
        ));
    }
    let proof = AudioProof {
        boot_id: boot_id().ok_or_else(|| "boot identity is unavailable".to_string())?,
        observed_at_ms: now_ms(),
        playback: true,
        capture_bytes,
    };
    write_json_atomic(Path::new(AUDIO_PROOF_PATH), &proof).map_err(|error| error.to_string())
}

fn discover_audio_capture_target(runtime: &str) -> Result<(String, String), String> {
    let output = Command::new("runuser")
        .args(["-u", "mm", "--", "env", runtime, "pw-dump"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("PipeWire graph could not be inspected".into());
    }
    capture_target_from_pw_dump(&output.stdout)
        .ok_or_else(|| "no PipeWire audio capture source is available".into())
}

fn capture_target_from_pw_dump(bytes: &[u8]) -> Option<(String, String)> {
    let graph: Vec<serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    graph
        .iter()
        .filter_map(|object| {
            let props = object.get("info")?.get("props")?;
            (props.get("media.class")?.as_str()? == "Audio/Source").then_some((
                object.get("id")?.as_u64()?,
                props.get("node.name")?.as_str()?,
            ))
        })
        .filter(|(_, name)| !name.ends_with(".monitor"))
        .min_by_key(|(id, _)| *id)
        .map(|(id, name)| (id.to_string(), name.to_string()))
}

fn run_checked(program: &str, args: &[&str], success: &str) -> Result<String, String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then(|| success.to_string())
        .ok_or_else(|| format!("{program} failed"))
}

fn expand_seat15_root(host: &str) -> Result<String, String> {
    if !is_seat15(host) {
        return Err("bounded root expansion is allowed only on seat 15".into());
    }
    let source = SystemSampler::run("findmnt", &["-n", "-o", "SOURCE", "/"])
        .map(|value| value.trim().to_string())
        .filter(|value| value.starts_with("/dev/mapper/") || value.starts_with("/dev/"))
        .ok_or_else(|| "root is not on an identifiable block device".to_string())?;
    let size = SystemSampler::run("findmnt", &["-b", "-n", "-o", "SIZE", "/"])
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or_else(|| "root size is unavailable".to_string())?;
    if !(14 * 1024_u64.pow(3)..=17 * 1024_u64.pow(3)).contains(&size) {
        return Err("root is not the expected 15 GiB pre-expansion volume".into());
    }
    let filesystem = SystemSampler::run("findmnt", &["-n", "-o", "FSTYPE", "/"])
        .map(|value| value.trim().to_string())
        .ok_or_else(|| "root filesystem type is unavailable".to_string())?;
    if filesystem != "xfs" {
        return Err("bounded root expansion requires the expected XFS filesystem".into());
    }
    let volume_group = SystemSampler::run("lvs", &["--noheadings", "-o", "vg_name", &source])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "root volume group is unavailable".to_string())?;
    let free = SystemSampler::run(
        "vgs",
        &[
            "--noheadings",
            "--units",
            "b",
            "--nosuffix",
            "-o",
            "vg_free",
            &volume_group,
        ],
    )
    .and_then(|value| value.trim().split('.').next()?.trim().parse::<u64>().ok())
    .ok_or_else(|| "volume-group reserve is unavailable".to_string())?;
    let target = 30 * 1024_u64.pow(3);
    let required = target.saturating_sub(size);
    let reserve = 8 * 1024_u64.pow(3);
    if free < required.saturating_add(reserve) {
        return Err("root expansion would consume the required 8 GiB VG reserve".into());
    }
    let status = Command::new("lvextend")
        .args(["-L", "30G", "--resizefs", &source])
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then(|| "root LV expanded online to 30 GiB".into())
        .ok_or_else(|| "lvextend failed".into())
}

/// Universal worker that publishes the condition-backed health authority.
pub struct NodeGradeWorker {
    host: String,
    workgroup_root: PathBuf,
    sampler: Box<dyn HealthSampler>,
    bus_root: Option<PathBuf>,
    poll: Duration,
    generation: u64,
    pressure: PressureWindow,
    action_cursor: Option<String>,
    last_snapshot: Option<SystemMeshHealthSnapshot>,
}

impl NodeGradeWorker {
    #[must_use]
    /// Construct the health worker for one node and its configured role rank.
    pub fn new(host: String, workgroup_root: PathBuf, role_rank: u8) -> Self {
        Self {
            sampler: Box::new(SystemSampler::new(
                host.clone(),
                workgroup_root.clone(),
                role_rank,
            )),
            host,
            workgroup_root,
            bus_root: mde_bus::default_data_dir(),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
        }
    }

    fn cycle(&mut self, persist: Option<&Persist>) -> SystemMeshHealthSnapshot {
        let now = now_ms();
        self.generation = self.generation.saturating_add(1);
        let observations = self.sampler.sample();
        self.pressure.observe(observations.resources);
        let path = node_dir(&self.workgroup_root).join(format!("{}.json", self.host));
        let previous = read_state(&path);
        let mut active = evaluate_conditions(
            &self.host,
            &observations,
            &self.pressure,
            self.generation,
            now,
        );
        let resolved = merge_lifecycle(previous.as_ref(), &mut active, now);
        let grade_factors = factors(&observations);
        let grade = NodeGrade::evaluate(
            &self.host,
            capability_score(grade_factors),
            grade_factors,
            &active,
            now,
        );
        let state = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: self.host.clone(),
            roster_revision: observations.roster_revision.clone(),
            generation: self.generation,
            published_at_ms: now,
            valid_until_ms: now.saturating_add(PUBLICATION_VALIDITY_MS),
            grade,
            active_conditions: active.clone(),
            resolved_conditions: resolved,
        };
        let previous_critical: BTreeSet<_> = previous
            .iter()
            .flat_map(|old| old.active_conditions.iter())
            .filter(|condition| {
                condition.requirement == RequirementClass::Required
                    && condition.severity == HealthSeverity::Critical
            })
            .map(|condition| condition.id.as_str())
            .collect();
        let _ = write_json_atomic(&path, &state);
        if let Some(persist) = persist {
            emit_json(persist, &node_health_topic(&self.host), &state);
            for condition in &active {
                if condition.requirement == RequirementClass::Required
                    && condition.severity == HealthSeverity::Critical
                    && !previous_critical.contains(condition.id.as_str())
                {
                    emit_critical(persist, condition, &self.host);
                }
            }
        }
        let snapshot = fold_snapshot(
            &self.host,
            observations.roster_revision,
            &observations.canonical_nodes,
            read_canonical_states(&self.workgroup_root),
            self.generation,
            now,
            SNAPSHOT_VALIDITY_MS,
            observations.reachable_lighthouses,
        );
        let _ = write_json_atomic(&snapshot_path(&self.workgroup_root, &self.host), &snapshot);
        if let Some(persist) = persist {
            emit_json(persist, SNAPSHOT_TOPIC, &snapshot);
        }
        self.last_snapshot = Some(snapshot.clone());
        snapshot
    }

    fn drain_actions(&mut self, persist: &Persist) {
        if self.last_snapshot.is_none() {
            return;
        }
        let Ok(messages) = persist.list_since(ACTION_TOPIC, self.action_cursor.as_deref()) else {
            return;
        };
        for message in messages {
            self.action_cursor = Some(message.ulid.clone());
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Ok(request) = serde_json::from_str::<HealthActionRequest>(body) else {
                continue;
            };
            let Some(snapshot) = self.last_snapshot.clone() else {
                break;
            };
            let decision = authorize_action(&request, &self.host, &snapshot);
            let condition_source = snapshot
                .active_conditions
                .iter()
                .find(|condition| condition.id == request.condition_id)
                .map_or("", |condition| condition.source.as_str());
            let (outcome, detail) = match decision {
                ActionDecision::Refuse(reason) => (HealthActionOutcome::Refused, reason.into()),
                ActionDecision::Stale => (
                    HealthActionOutcome::StaleGeneration,
                    "snapshot generation changed; refresh before applying".into(),
                ),
                ActionDecision::Apply => {
                    if matches!(
                        request.action,
                        HealthAction::Acknowledge | HealthAction::SnoozeOneHour
                    ) {
                        self.update_condition_state(&request, now_ms());
                    }
                    match execute_action(
                        request.action,
                        &self.host,
                        condition_source,
                        &self.workgroup_root,
                    ) {
                        Ok(detail) => (HealthActionOutcome::Applied, detail),
                        Err(detail) => (HealthActionOutcome::Failed, detail),
                    }
                }
            };
            let refreshed_snapshot =
                (outcome == HealthActionOutcome::Applied).then(|| self.cycle(Some(persist)));
            let result_generation = refreshed_snapshot
                .as_ref()
                .map_or(snapshot.generation, |snapshot| snapshot.generation);
            let refreshed_evidence = refreshed_snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .active_conditions
                    .iter()
                    .chain(snapshot.resolved_conditions.iter())
                    .find(|condition| condition.id == request.condition_id)
                    .map(|condition| condition.evidence.clone())
            });
            let result = HealthActionResult {
                schema_version: HEALTH_SCHEMA_VERSION,
                request_id: request.request_id.clone(),
                condition_id: request.condition_id,
                action: request.action,
                outcome,
                detail,
                audit_id: format!("health:{}:{}", self.host, message.ulid),
                completed_at_ms: now_ms(),
                snapshot_generation: result_generation,
                refreshed_evidence,
            };
            emit_json(persist, &action_result_topic(&request.request_id), &result);
        }
    }

    fn update_condition_state(&self, request: &HealthActionRequest, now: u64) {
        let path = node_dir(&self.workgroup_root).join(format!("{}.json", self.host));
        let Some(mut state) = read_state(&path) else {
            return;
        };
        if let Some(condition) = state
            .active_conditions
            .iter_mut()
            .find(|condition| condition.id == request.condition_id)
        {
            match request.action {
                HealthAction::Acknowledge => condition.acknowledged_at_ms = Some(now),
                HealthAction::SnoozeOneHour => {
                    condition.snoozed_until_ms = Some(now.saturating_add(3_600_000))
                }
                _ => {}
            }
            let _ = write_json_atomic(&path, &state);
        }
    }
}

#[async_trait::async_trait]
impl Worker for NodeGradeWorker {
    fn name(&self) -> &'static str {
        "node_grade"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let persist = self
            .bus_root
            .clone()
            .and_then(|root| Persist::open(root).ok());
        if let Some(persist) = persist.as_ref() {
            // Health remediations are mutations, not replayable events. Begin at
            // the current tail so a daemon restart can never reapply a retained
            // confirmation from an earlier boot.
            self.action_cursor = persist.latest_ulid(ACTION_TOPIC).ok().flatten();
        }

        // Anchor the existing 10-second interval at a deterministic per-host
        // phase. Shutdown remains prompt while the initial offset is pending;
        // every later cycle retains the original freshness and action cadence.
        tokio::select! {
            () = tokio::time::sleep(initial_phase_for(&self.host)) => {}
            () = shutdown.wait() => return Ok(()),
        }
        self.cycle(persist.as_ref());
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Some(persist) = persist.as_ref() { self.drain_actions(persist); }
                    self.cycle(persist.as_ref());
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct FixtureSampler(HealthObservations);

    impl HealthSampler for FixtureSampler {
        fn sample(&self) -> HealthObservations {
            self.0.clone()
        }
    }

    fn observations(role: &str) -> HealthObservations {
        let assigned_capabilities = BTreeSet::new();
        let services = required_services(role, &assigned_capabilities)
            .into_iter()
            .map(|name| (name.to_string(), true))
            .collect();
        HealthObservations {
            role: role.into(),
            roster_revision: "r1".into(),
            canonical_nodes: BTreeSet::from(["node".into()]),
            services,
            assigned_capabilities,
            resources: ResourceObservation {
                cpu_load_ratio: Some(0.2),
                memory_available_pct: Some(60.0),
                root_used_pct: Some(40.0),
            },
            audio: (role == "workstation").then_some(AudioObservation {
                pipewire_graph: true,
                pipewire_pulse: true,
                wireplumber_policy: true,
                playback: true,
                capture: true,
            }),
            mesh_snapshot_present: true,
            overlay_up: true,
            reachable_lighthouses: 3,
            firmware_refresh_failed: false,
            device_inventory: Some(DeviceInventory {
                host: "node".into(),
                published_at_ms: 100,
                summary: Default::default(),
                tools: Default::default(),
                categories: Vec::new(),
            }),
        }
    }

    #[test]
    fn publication_validity_covers_two_replica_rescan_windows() {
        const REPLICA_RESCAN_MS: u64 = 60_000;
        assert!(PUBLICATION_VALIDITY_MS >= REPLICA_RESCAN_MS * 2);
    }

    #[test]
    fn initial_phase_is_bounded_and_stable_per_host() {
        let phase = initial_phase_for("seat15");
        assert_eq!(phase, initial_phase_for("seat15"));
        assert!(phase <= Duration::from_millis(MAX_INITIAL_PHASE_MS));
        assert_ne!(phase, initial_phase_for("dell-laptop"));
        assert!(
            Duration::from_millis(MAX_INITIAL_PHASE_MS)
                < Duration::from_millis(SNAPSHOT_VALIDITY_MS)
        );
        assert_eq!(initial_phase_for(""), Duration::ZERO);
    }

    #[test]
    fn audio_probe_requires_all_bounded_provider_bits() {
        assert_eq!(
            parse_audio_probe("graph=1 pulse=0 wireplumber=1 playback=1 capture=0\n"),
            Some(AudioObservation {
                pipewire_graph: true,
                pipewire_pulse: false,
                wireplumber_policy: true,
                playback: true,
                capture: false,
            })
        );
        assert_eq!(parse_audio_probe("graph=1 pulse=1"), None);
        assert_eq!(
            parse_audio_probe("graph=1 pulse=1 wireplumber=1 playback=1 capture=2"),
            None
        );
    }

    #[test]
    fn optional_services_never_create_conditions() {
        let mut sample = observations("workstation");
        sample.services.insert("voice".into(), false);
        sample.services.insert("music".into(), false);
        assert!(
            evaluate_conditions("node", &sample, &PressureWindow::default(), 1, 100).is_empty()
        );
        assert_eq!(factors(&sample).system, Some(100));
    }

    #[test]
    fn role_capabilities_control_required_services() {
        let mut lighthouse = observations("lighthouse");
        lighthouse.services.remove("kdc");
        lighthouse.services.remove("workbench");
        assert!(
            evaluate_conditions("node", &lighthouse, &PressureWindow::default(), 1, 100).is_empty()
        );
        let mut workstation = observations("workstation");
        workstation.services.insert("kdc".into(), false);
        assert!(
            evaluate_conditions("node", &workstation, &PressureWindow::default(), 1, 100)
                .iter()
                .any(|condition| condition.id.ends_with("required-service-kdc"))
        );

        let mut assigned = observations("workstation");
        assigned.assigned_capabilities.insert("voice".into());
        assigned.services.insert("voice".into(), false);
        assert!(
            evaluate_conditions("node", &assigned, &PressureWindow::default(), 1, 100)
                .iter()
                .any(|condition| condition.id.ends_with("required-service-voice"))
        );
    }

    #[test]
    fn resource_pressure_must_be_sustained() {
        let mut sample = observations("workstation");
        sample.resources.cpu_load_ratio = Some(0.99);
        sample.resources.memory_available_pct = Some(7.0);
        let mut pressure = PressureWindow::default();
        pressure.observe(sample.resources);
        assert!(!evaluate_conditions("node", &sample, &pressure, 1, 100)
            .iter()
            .any(|condition| matches!(
                condition.id.as_str(),
                "node:cpu-pressure" | "node:memory-pressure"
            )));
        pressure.observe(sample.resources);
        pressure.observe(sample.resources);
        let conditions = evaluate_conditions("node", &sample, &pressure, 1, 100);
        assert!(conditions
            .iter()
            .any(|condition| condition.id == "node:cpu-pressure"));
        assert!(conditions
            .iter()
            .any(|condition| condition.id == "node:memory-pressure"));
    }

    #[test]
    fn cpu_pressure_uses_busy_time_instead_of_load_average() {
        assert_eq!(
            parse_cpu_counters("cpu  100 5 25 800 20 10 5 0 0 0\ncpu0 0 0 0 0"),
            Some(CpuCounters {
                busy: 145,
                total: 965,
            })
        );
        assert_eq!(parse_cpu_counters("intr 1 2 3"), None);
    }

    #[test]
    fn ordinary_headroom_never_creates_an_incident() {
        let mut sample = observations("workstation");
        sample.resources = ResourceObservation {
            cpu_load_ratio: Some(0.8),
            memory_available_pct: Some(15.0),
            root_used_pct: Some(80.0),
        };
        let mut pressure = PressureWindow::default();
        for _ in 0..3 {
            pressure.observe(sample.resources);
        }
        assert!(evaluate_conditions("node", &sample, &pressure, 1, 100).is_empty());
        assert_eq!(
            NodeGrade::evaluate("node", 70, factors(&sample), &[], 100)
                .grade
                .as_str(),
            "C"
        );
    }

    #[test]
    fn pipewire_capture_uses_a_real_source_instead_of_a_default_alias() {
        let dump = br#"[
            {"id":73,"info":{"props":{"media.class":"Audio/Source","node.name":"usb-input"}}},
            {"id":51,"info":{"props":{"media.class":"Audio/Sink","node.name":"speaker"}}},
            {"id":47,"info":{"props":{"media.class":"Audio/Source","node.name":"built-in-input"}}},
            {"id":49,"info":{"props":{"media.class":"Audio/Source","node.name":"speaker.monitor"}}}
        ]"#;
        assert_eq!(
            capture_target_from_pw_dump(dump),
            Some(("47".into(), "built-in-input".into()))
        );
        assert_eq!(capture_target_from_pw_dump(b"[]"), None);
    }

    fn snapshot() -> SystemMeshHealthSnapshot {
        fold_snapshot(
            "node",
            "r1",
            &BTreeSet::from(["node".into()]),
            Vec::new(),
            7,
            100,
            100,
            1,
        )
    }

    fn request(action: HealthAction) -> HealthActionRequest {
        HealthActionRequest {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: "req-1".into(),
            condition_id: "node:root-space".into(),
            action,
            target: HealthScope::Node {
                node: "node".into(),
            },
            expected_snapshot_generation: 7,
            requester: "node".into(),
            authorization: "local-seat".into(),
            confirmation: Some("CONFIRM".into()),
            requested_at_ms: 100,
        }
    }

    #[test]
    fn every_remediation_refusal_path_is_typed() {
        let snapshot = snapshot();
        assert!(matches!(
            authorize_action(&request(HealthAction::RestartMackesd), "node", &snapshot),
            ActionDecision::Refuse("condition is not active for target")
        ));
        let mut malformed = request(HealthAction::RestartMackesd);
        malformed.schema_version = 99;
        assert!(matches!(
            authorize_action(&malformed, "node", &snapshot),
            ActionDecision::Refuse(_)
        ));
        let mut unauthorized = request(HealthAction::RestartMackesd);
        unauthorized.authorization.clear();
        assert!(matches!(
            authorize_action(&unauthorized, "node", &snapshot),
            ActionDecision::Refuse(_)
        ));
        let mut stale = request(HealthAction::RestartMackesd);
        stale.expected_snapshot_generation = 6;
        assert_eq!(
            authorize_action(&stale, "node", &snapshot),
            ActionDecision::Stale
        );

        let mut actionable = snapshot.clone();
        actionable.active_conditions.push(condition(
            "node",
            "root-space",
            HealthComponent::Resources,
            "root-filesystem",
            HealthSeverity::Warning,
            "root threshold breached",
            100,
            vec![remediation(
                "node",
                HealthAction::ExpandSeat15Root,
                7,
                "bounded expansion",
                true,
            )],
        ));
        let mut wrong_target = request(HealthAction::ExpandSeat15Root);
        wrong_target.target = HealthScope::Node {
            node: "other".into(),
        };
        assert!(matches!(
            authorize_action(&wrong_target, "node", &actionable),
            ActionDecision::Refuse("target is not this node")
        ));
        let not_offered = request(HealthAction::RefreshFirmwareMetadata);
        assert!(matches!(
            authorize_action(&not_offered, "node", &actionable),
            ActionDecision::Refuse("action is not offered for this condition")
        ));
        let mut unconfirmed = request(HealthAction::ExpandSeat15Root);
        unconfirmed.confirmation = None;
        assert!(matches!(
            authorize_action(&unconfirmed, "node", &actionable),
            ActionDecision::Refuse("explicit confirmation is required")
        ));
        assert_eq!(
            authorize_action(
                &request(HealthAction::ExpandSeat15Root),
                "node",
                &actionable
            ),
            ActionDecision::Apply
        );
    }

    #[test]
    fn remediation_cannot_borrow_an_offer_from_another_node() {
        let mut snapshot = snapshot();
        snapshot.active_conditions.push(condition(
            "remote-node",
            "root-space",
            HealthComponent::Resources,
            "root-filesystem",
            HealthSeverity::Warning,
            "remote root threshold breached",
            100,
            vec![remediation(
                "remote-node",
                HealthAction::ExpandSeat15Root,
                7,
                "bounded expansion on the remote node",
                true,
            )],
        ));
        let mut cross_node = request(HealthAction::ExpandSeat15Root);
        cross_node.condition_id = "remote-node:root-space".into();

        assert_eq!(
            authorize_action(&cross_node, "node", &snapshot),
            ActionDecision::Refuse("condition is not active for target"),
            "a remote condition must not authorize mutation of the local target"
        );
    }

    #[test]
    fn conflict_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let nodes = node_dir(dir.path());
        std::fs::create_dir_all(&nodes).unwrap();
        let sample = observations("workstation");
        let state = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: "node".into(),
            roster_revision: "r1".into(),
            generation: 1,
            published_at_ms: 100,
            valid_until_ms: 200,
            grade: NodeGrade::evaluate("node", 100, factors(&sample), &[], 100),
            active_conditions: Vec::new(),
            resolved_conditions: Vec::new(),
        };
        write_json_atomic(&nodes.join("node.sync-conflict-1.json"), &state).unwrap();
        assert!(read_canonical_states(dir.path()).is_empty());
    }

    #[test]
    fn applied_actions_emit_audited_results_with_refreshed_evidence() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut sample = observations("workstation");
        sample.services.insert("workbench".into(), false);
        sample.device_inventory.as_mut().unwrap().published_at_ms = now_ms();
        let mut worker = NodeGradeWorker {
            host: "node".into(),
            workgroup_root: workgroup.path().to_path_buf(),
            sampler: Box::new(FixtureSampler(sample)),
            bus_root: Some(bus.path().to_path_buf()),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
        };
        let snapshot = worker.cycle(Some(&persist));
        let condition = snapshot
            .active_conditions
            .iter()
            .find(|condition| condition.id.ends_with("required-service-workbench"))
            .unwrap();
        let request = HealthActionRequest {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: "audit-request".into(),
            condition_id: condition.id.clone(),
            action: HealthAction::Acknowledge,
            target: HealthScope::Node {
                node: "node".into(),
            },
            expected_snapshot_generation: snapshot.generation,
            requester: "node".into(),
            authorization: "local-seat".into(),
            confirmation: None,
            requested_at_ms: now_ms(),
        };
        let mut same_generation_second = request.clone();
        same_generation_second.request_id = "same-generation-second".into();
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).unwrap()),
            )
            .unwrap();
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&same_generation_second).unwrap()),
            )
            .unwrap();

        worker.drain_actions(&persist);

        let messages = persist
            .list_since(&action_result_topic("audit-request"), None)
            .unwrap();
        let result: HealthActionResult =
            serde_json::from_str(messages.last().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(result.outcome, HealthActionOutcome::Applied);
        assert!(result.audit_id.starts_with("health:node:"));
        assert_eq!(result.snapshot_generation, 2);
        assert!(result.refreshed_evidence.is_some());
        let second_messages = persist
            .list_since(&action_result_topic("same-generation-second"), None)
            .unwrap();
        let second: HealthActionResult =
            serde_json::from_str(second_messages.last().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(
            second.outcome,
            HealthActionOutcome::StaleGeneration,
            "a second mutation from the old generation cannot apply in the same drain batch"
        );
        let state = read_state(&node_dir(workgroup.path()).join("node.json")).unwrap();
        assert!(state
            .active_conditions
            .iter()
            .find(|condition| condition.id == request.condition_id)
            .unwrap()
            .acknowledged_at_ms
            .is_some());
    }
}
