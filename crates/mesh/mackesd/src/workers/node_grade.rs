//! System and Mesh Health authority.
//!
//! The historical worker name is retained only as the supervisor identity. Its
//! output is the versioned health contract in `mackes-mesh-types`; there is no
//! second grade shape or score ledger in this crate.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::device_inventory::{self, DeviceInventory, DeviceStatus};
use mackes_mesh_types::health::{
    action_result_topic, fold_snapshot_with_availability, node_health_topic, GradeFactors,
    HealthAction, HealthActionOutcome, HealthActionRequest, HealthActionResult, HealthComponent,
    HealthCondition, HealthEvidence, HealthRemediation, HealthScope, HealthSeverity,
    NodeAvailabilityAssessment, NodeAvailabilityPolicy, NodeGrade, NodeHealthState,
    RequirementClass, SystemMeshHealthSnapshot, ACTION_TOPIC, CRITICAL_NOTIFY_TOPIC,
    HEALTH_SCHEMA_VERSION, MAX_NODE_HEALTH_CONDITIONS, SNAPSHOT_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::node_availability::read_runtime_availability_intent;
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
// Health observations are sampled every 10s, but unchanged condition state
// does not need a new retained Bus row on every sample. Keep the publication
// comfortably inside the node-row validity window so a healthy publisher still
// refreshes its authority even when its observations are steady.
const HEALTH_PUBLICATION_HEARTBEAT_MS: u64 = 60_000;
// Keep the folded snapshot file and Bus projection fresh without rewriting
// them for every 10-second sample. This remains below SNAPSHOT_VALIDITY_MS.
const SNAPSHOT_PUBLICATION_HEARTBEAT_MS: u64 = 15_000;
// Resolved conditions are history, not current materialized health. Keep the
// producer's durable row inside the fleet-wide privacy epoch so restart and
// Syncthing replication cannot republish an incident after its retention
// window. The inclusive boundary gives every record its complete six hours.
const HEALTH_HISTORY_RETENTION_MS: u64 = 6 * 60 * 60 * 1_000;
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
const DEFAULT_ACTION_STATE_ROOT: &str = "/var/lib/mackesd/node-grade-action-results";
const MAX_ACTION_JOURNAL_BYTES: u64 = 256 * 1024;
const MAX_PENDING_ACTION_RESULTS: usize = 128;

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

fn node_grade_bus_roots(explicit: Option<&Path>, current: Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(root) = explicit {
        return vec![root.to_path_buf()];
    }
    let mut roots = Vec::new();
    if let Some(root) = current {
        roots.push(root);
    }
    let system = PathBuf::from(mde_bus::SYSTEM_BUS_ROOT);
    if !roots.contains(&system) {
        roots.push(system);
    }
    roots
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

#[derive(Debug, Clone, Default)]
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
        Some(inventory) if inventory.host != host || inventory.published_at_ms > now_ms => {
            conditions.push(condition(
                host,
                "device-evidence-invalid",
                HealthComponent::Evidence,
                "device-inventory",
                HealthSeverity::Warning,
                "Required device inventory evidence has invalid node or time provenance.",
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
        Some(inventory) if now_ms - inventory.published_at_ms > DEVICE_INVENTORY_VALIDITY_MS => {
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

fn factors(host: &str, observations: &HealthObservations, now_ms: u64) -> GradeFactors {
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
        devices: observations
            .device_inventory
            .as_ref()
            .filter(|inventory| {
                inventory.host == host
                    && inventory.published_at_ms <= now_ms
                    && now_ms - inventory.published_at_ms <= DEVICE_INVENTORY_VALIDITY_MS
            })
            .map(|inventory| {
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

fn action_journal_path(state_root: &Path, source_ulid: &str) -> PathBuf {
    state_root.join(format!("{source_ulid}.json"))
}

fn valid_action_source_ulid(source_ulid: &str) -> bool {
    source_ulid.len() == 26
        && source_ulid
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum DurableHealthAction {
    Claimed {
        source_ulid: String,
        request: HealthActionRequest,
        snapshot_generation: u64,
        claimed_at_ms: u64,
    },
    Complete {
        source_ulid: String,
        result: HealthActionResult,
    },
}

#[derive(Debug, Clone)]
struct StagedActionResult {
    source_ulid: String,
    result: HealthActionResult,
    complete: Option<DurableHealthAction>,
    already_published: bool,
}

#[derive(Debug, Clone)]
enum StagedActionMessage {
    Terminal(String),
    Request {
        source_ulid: String,
        request: HealthActionRequest,
        result_already_published: bool,
    },
}

fn validate_action_state_root(state_root: &Path, trusted_uid: u32) -> Result<bool, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let metadata = match std::fs::symlink_metadata(state_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("action state metadata unavailable: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err("action state root has unsafe type, owner, or mode".into());
    }
    Ok(true)
}

fn sync_directory(path: &Path, label: &str) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("{label} directory sync failed: {error}"))
}

fn ensure_action_state_root(state_root: &Path, trusted_uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

    if validate_action_state_root(state_root, trusted_uid)? {
        return Ok(());
    }
    let parent = state_root
        .parent()
        .ok_or_else(|| "action state root has no parent".to_string())?;
    let parent_metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("action state parent metadata unavailable: {error}"))?;
    if parent_metadata.file_type().is_symlink()
        || !parent_metadata.is_dir()
        || parent_metadata.uid() != trusted_uid
        || parent_metadata.permissions().mode() & 0o022 != 0
    {
        return Err("action state parent is not a trusted local boundary".into());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    let created = match builder.create(state_root) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(format!("action state root creation failed: {error}")),
    };
    let valid = validate_action_state_root(state_root, trusted_uid)?;
    if created {
        sync_directory(parent, "action state parent")?;
    }
    valid
        .then_some(())
        .ok_or_else(|| "action state root disappeared after creation".to_string())
}

fn read_action_journal(
    state_root: &Path,
    path: &Path,
    trusted_uid: u32,
) -> Result<Option<DurableHealthAction>, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if path.parent() != Some(state_root) {
        return Err("health action journal escaped its local state root".into());
    }
    if !validate_action_state_root(state_root, trusted_uid)? {
        return Ok(None);
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() > MAX_ACTION_JOURNAL_BYTES
    {
        return Err("health action journal has unsafe type, owner, mode, or size".into());
    }
    let body = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn sync_action_state_root(state_root: &Path) -> Result<(), String> {
    sync_directory(state_root, "action state")
}

fn pending_action_journals(state_root: &Path, trusted_uid: u32) -> Result<Vec<PathBuf>, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if !validate_action_state_root(state_root, trusted_uid)? {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(state_root)
        .map_err(|error| format!("action state root read failed: {error}"))?;
    let mut admitted = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("action state entry failed: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "action state contains a non-UTF-8 entry".to_string())?;
        let is_journal = name
            .strip_suffix(".json")
            .is_some_and(valid_action_source_ulid);
        let is_temp = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".tmp"))
            .is_some_and(valid_action_source_ulid);
        if !is_journal && !is_temp {
            return Err("action state contains an unexpected entry".into());
        }
        if admitted.len() == MAX_PENDING_ACTION_RESULTS {
            return Err(format!(
                "action state exceeds its {}-entry bound",
                MAX_PENDING_ACTION_RESULTS
            ));
        }
        admitted.push((entry.path(), is_temp));
    }
    let mut paths = Vec::new();
    let mut removed_temp = false;
    for (path, is_temp) in admitted {
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("action state entry metadata failed: {error}"))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != trusted_uid
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.len() > MAX_ACTION_JOURNAL_BYTES
        {
            return Err("action state contains an unsafe journal or temporary entry".into());
        }
        if is_temp {
            std::fs::remove_file(&path)
                .map_err(|error| format!("stale action journal cleanup failed: {error}"))?;
            removed_temp = true;
        } else {
            paths.push(path);
        }
    }
    if removed_temp {
        sync_action_state_root(state_root)?;
    }
    paths.sort();
    Ok(paths)
}

fn cleanup_action_journal_temp(
    state_root: &Path,
    tmp: &Path,
    trusted_uid: u32,
) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let metadata = match std::fs::symlink_metadata(tmp) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("action journal temp metadata failed: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("refusing to clean an unsafe action journal temporary entry".into());
    }
    std::fs::remove_file(tmp)
        .map_err(|error| format!("action journal temp cleanup failed: {error}"))?;
    sync_action_state_root(state_root)
}

fn write_action_journal(
    state_root: &Path,
    source_ulid: &str,
    record: &DurableHealthAction,
    trusted_uid: u32,
) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

    if !valid_action_source_ulid(source_ulid) {
        return Err("health action source ULID is invalid".into());
    }
    ensure_action_state_root(state_root, trusted_uid)?;
    let path = action_journal_path(state_root, source_ulid);
    let pending = pending_action_journals(state_root, trusted_uid)?;
    if !path.exists() && pending.len() >= MAX_PENDING_ACTION_RESULTS {
        return Err(format!(
            "action state reached its {}-entry admission bound",
            MAX_PENDING_ACTION_RESULTS
        ));
    }
    if read_action_journal(state_root, &path, trusted_uid).is_err() && path.exists() {
        return Err("refusing to replace an unsafe action journal".into());
    }
    let body = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    if body.len() > usize::try_from(MAX_ACTION_JOURNAL_BYTES).unwrap_or(usize::MAX) {
        return Err("health action journal exceeds its size bound".into());
    }
    let tmp = state_root.join(format!(".{source_ulid}.tmp"));
    if let Ok(metadata) = std::fs::symlink_metadata(&tmp) {
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != trusted_uid
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err("health action journal temporary path is unsafe".into());
        }
        std::fs::remove_file(&tmp)
            .map_err(|error| format!("stale action journal cleanup failed: {error}"))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|error| format!("action journal temporary create failed: {error}"))?;
    if let Err(error) = file.write_all(&body) {
        drop(file);
        let _ = cleanup_action_journal_temp(state_root, &tmp, trusted_uid);
        return Err(format!("action journal write failed: {error}"));
    }
    if let Err(error) = file.sync_all() {
        drop(file);
        let _ = cleanup_action_journal_temp(state_root, &tmp, trusted_uid);
        return Err(format!("action journal sync failed: {error}"));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&tmp, &path) {
        let _ = cleanup_action_journal_temp(state_root, &tmp, trusted_uid);
        return Err(format!("action journal replace failed: {error}"));
    }
    sync_action_state_root(state_root)?;
    match read_action_journal(state_root, &path, trusted_uid)? {
        Some(_) => Ok(()),
        None => Err("action journal disappeared after replacement".into()),
    }
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

fn read_snapshot(path: &Path) -> Option<SystemMeshHealthSnapshot> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_HEALTH_FILE_BYTES
    {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Admit the producer's durable row only as its own restart generation.
///
/// The canonical directory is replicated, so the filename alone cannot prove
/// that a row belongs to this producer. Validate the complete row at its
/// publication instant (which still permits a truthful expired row after a
/// long restart), bind it to the expected publisher, and reject observations
/// from ahead of the local clock before they can seed generation or lifecycle
/// state.
fn read_restart_state(path: &Path, publisher: &str, now_ms: u64) -> Option<NodeHealthState> {
    let state = read_state(path)?;
    (state.publisher == publisher
        && state.published_at_ms <= now_ms
        && state.validate_at(state.published_at_ms).is_ok())
    .then_some(state)
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
            if name.starts_with('.')
                || name.contains("sync-conflict")
                || !std::path::Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                return None;
            }
            let state = read_state(&entry.path())?;
            (name == format!("{}.json", state.publisher)).then_some(state)
        })
        .collect()
}

fn availability_assessments(
    workgroup_root: &Path,
    canonical_nodes: &BTreeSet<String>,
    now_ms: u64,
) -> BTreeMap<String, NodeAvailabilityAssessment> {
    canonical_nodes
        .iter()
        .filter_map(|node| {
            let intent = read_runtime_availability_intent(workgroup_root, node)
                .ok()
                .flatten()?;
            let assessment = NodeAvailabilityPolicy::for_device_class(intent.device_class).assess(
                Some(&intent),
                now_ms,
                None,
            );
            matches!(
                assessment,
                NodeAvailabilityAssessment::ExpectedAbsence
                    | NodeAvailabilityAssessment::WarningMissedReturn
                    | NodeAvailabilityAssessment::CriticalMissedReturn
            )
            .then(|| (node.clone(), assessment))
        })
        .collect()
}

fn fold_health_snapshot(
    workgroup_root: &Path,
    observer: &str,
    roster_revision: String,
    canonical_nodes: &BTreeSet<String>,
    publications: Vec<NodeHealthState>,
    generation: u64,
    now_ms: u64,
    reachable_lighthouses: usize,
) -> SystemMeshHealthSnapshot {
    let availability = availability_assessments(workgroup_root, canonical_nodes, now_ms);
    fold_snapshot_with_availability(
        observer,
        roster_revision,
        canonical_nodes,
        publications,
        &availability,
        generation,
        now_ms,
        SNAPSHOT_VALIDITY_MS,
        reachable_lighthouses,
    )
}

fn merge_availability_lifecycle(
    snapshot: &mut SystemMeshHealthSnapshot,
    previous: Option<&SystemMeshHealthSnapshot>,
    now_ms: u64,
) {
    const SOURCE: &str = "health-availability-fold";
    let Some(previous) = previous else {
        return;
    };

    for current in snapshot
        .active_conditions
        .iter_mut()
        .filter(|condition| condition.source == SOURCE)
    {
        if let Some(prior) = previous.active_conditions.iter().find(|prior| {
            prior.source == SOURCE && prior.id == current.id && prior.scope == current.scope
        }) {
            current.active_since_ms = prior.active_since_ms;
            current.acknowledged_at_ms = prior.acknowledged_at_ms;
            current.snoozed_until_ms = prior.snoozed_until_ms;
        }
    }

    for prior in previous
        .active_conditions
        .iter()
        .filter(|condition| condition.source == SOURCE)
    {
        let remains_active = snapshot
            .active_conditions
            .iter()
            .any(|current| current.id == prior.id && current.scope == prior.scope);
        if remains_active {
            continue;
        }
        let mut resolved = prior.clone();
        resolved.resolved_at_ms = Some(now_ms);
        snapshot.resolved_conditions.push(resolved);
    }

    snapshot.resolved_conditions.retain(|condition| {
        condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
            resolved_at_ms <= now_ms
                && now_ms.saturating_sub(resolved_at_ms) <= HEALTH_HISTORY_RETENTION_MS
        })
    });
    snapshot.resolved_conditions.sort_by(|left, right| {
        right
            .resolved_at_ms
            .cmp(&left.resolved_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    snapshot
        .resolved_conditions
        .truncate(MAX_NODE_HEALTH_CONDITIONS);
}

fn merge_lifecycle(
    previous: Option<&NodeHealthState>,
    current: &mut [HealthCondition],
    now_ms: u64,
) -> Vec<HealthCondition> {
    let mut resolved = previous.map_or_else(Vec::new, |state| state.resolved_conditions.clone());
    resolved.retain(|condition| {
        condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
            resolved_at_ms <= now_ms
                && now_ms.saturating_sub(resolved_at_ms) <= HEALTH_HISTORY_RETENTION_MS
        })
    });
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
                // The recovery sample proves that the condition is absent; it
                // does not prove that the condition remained present until
                // this instant. Preserve the final positive observation so
                // history durations do not silently absorb the detection gap.
                resolved.push(closed);
            }
        }
    }
    resolved.sort_by(|left, right| right.resolved_at_ms.cmp(&left.resolved_at_ms));
    resolved.truncate(128);
    resolved
}

fn emit_json<T: Serialize>(persist: &mut Persist, topic: &str, value: &T) -> Result<(), String> {
    let body = serde_json::to_string(value).map_err(|error| error.to_string())?;
    persist.reopen_if_index_changed();
    persist
        .write(topic, Priority::Default, None, Some(&body))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn emit_critical(
    persist: &mut Persist,
    condition: &HealthCondition,
    host: &str,
) -> Result<(), String> {
    let body = serde_json::json!({
        "severity": "critical",
        "source": "system-mesh-health",
        "summary": condition.evidence.summary,
        "condition_id": condition.id,
        "host": host,
        "ts_unix_ms": condition.last_observed_ms,
    });
    persist.reopen_if_index_changed();
    persist
        .write(
            CRITICAL_NOTIFY_TOPIC,
            Priority::Default,
            None,
            Some(&body.to_string()),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
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
#[derive(Debug, Clone, PartialEq, Eq)]
struct BusIdentity {
    root: PathBuf,
    device: u64,
    inode: u64,
}

pub struct NodeGradeWorker {
    host: String,
    workgroup_root: PathBuf,
    sampler: Box<dyn HealthSampler>,
    bus_root_override: Option<PathBuf>,
    active_bus: Option<BusIdentity>,
    action_state_root: PathBuf,
    poll: Duration,
    generation: u64,
    pressure: PressureWindow,
    action_cursor: Option<String>,
    last_snapshot: Option<SystemMeshHealthSnapshot>,
    last_health_publication: Option<(NodeHealthState, u64)>,
    last_health_file_publication: Option<(NodeHealthState, u64)>,
    last_snapshot_file_publication: Option<(SystemMeshHealthSnapshot, u64)>,
    last_snapshot_bus_publication: Option<(SystemMeshHealthSnapshot, u64)>,
    #[cfg(test)]
    fail_terminal_result_writes: usize,
    #[cfg(test)]
    fail_snapshot_file_writes: usize,
    #[cfg(test)]
    action_execution_count: usize,
    #[cfg(test)]
    trusted_action_owner_uid: u32,
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
            bus_root_override: None,
            active_bus: None,
            action_state_root: PathBuf::from(DEFAULT_ACTION_STATE_ROOT),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
            last_health_publication: None,
            last_health_file_publication: None,
            last_snapshot_file_publication: None,
            last_snapshot_bus_publication: None,
            #[cfg(test)]
            fail_terminal_result_writes: 0,
            #[cfg(test)]
            fail_snapshot_file_writes: 0,
            #[cfg(test)]
            action_execution_count: 0,
            #[cfg(test)]
            trusted_action_owner_uid: 0,
        }
    }

    /// Override the Bus root. Production resolves current user/service storage
    /// and canonical system storage for each transaction.
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn trusted_action_owner_uid(&self) -> u32 {
        #[cfg(test)]
        {
            return self.trusted_action_owner_uid;
        }
        #[cfg(not(test))]
        {
            0
        }
    }

    fn open_bus(&self) -> Result<(Persist, BusIdentity), String> {
        let roots = node_grade_bus_roots(
            self.bus_root_override.as_deref(),
            mde_bus::default_data_dir(),
        );
        let mut last_error = "no Bus roots resolved".to_string();
        for root in roots {
            let mut persist = match Persist::open(root.clone()) {
                Ok(persist) => persist,
                Err(error) => {
                    last_error = format!("Bus {} open failed: {error}", root.display());
                    continue;
                }
            };
            persist.reopen_if_index_changed();
            let index = root.join("index.sqlite");
            let metadata = match std::fs::metadata(&index) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => {
                    last_error = format!("Bus index {} is not a file", index.display());
                    continue;
                }
                Err(error) => {
                    last_error = format!("Bus index {} metadata failed: {error}", index.display());
                    continue;
                }
            };
            return Ok((
                persist,
                BusIdentity {
                    root,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                },
            ));
        }
        Err(last_error)
    }

    fn store_action_journal(
        &mut self,
        source_ulid: &str,
        record: &DurableHealthAction,
    ) -> Result<(), String> {
        #[cfg(test)]
        if matches!(record, DurableHealthAction::Complete { .. })
            && self.fail_terminal_result_writes > 0
        {
            self.fail_terminal_result_writes -= 1;
            return Err("injected terminal result storage failure".into());
        }
        write_action_journal(
            &self.action_state_root,
            source_ulid,
            record,
            self.trusted_action_owner_uid(),
        )
    }

    fn action_result_published(
        persist: &mut Persist,
        result: &HealthActionResult,
    ) -> Result<bool, String> {
        let topic = action_result_topic(&result.request_id);
        persist.reopen_if_index_changed();
        let messages = persist
            .list_since(&topic, None)
            .map_err(|error| format!("action result lane {topic} unreadable: {error}"))?;
        let mut exact_result_found = false;
        for message in messages {
            let body = message
                .body
                .as_deref()
                .ok_or_else(|| format!("action result lane {topic} contains an empty row"))?;
            let published: HealthActionResult = serde_json::from_str(body)
                .map_err(|error| format!("action result lane {topic} is invalid: {error}"))?;
            published
                .validate_at(now_ms())
                .map_err(|error| format!("action result lane {topic} is invalid: {error}"))?;
            if published.audit_id == result.audit_id {
                if published != *result {
                    return Err(format!(
                        "action result lane {topic} contains a conflicting body for audit id {}",
                        result.audit_id
                    ));
                }
                exact_result_found = true;
            }
        }
        Ok(exact_result_found)
    }

    fn publish_action_result(
        &self,
        persist: &mut Persist,
        staged: &StagedActionResult,
    ) -> Result<(), String> {
        staged
            .result
            .validate_at(now_ms())
            .map_err(|error| format!("action result publication refused: {error}"))?;
        if !staged.already_published {
            let topic = action_result_topic(&staged.result.request_id);
            let body = serde_json::to_string(&staged.result).map_err(|error| error.to_string())?;
            persist.reopen_if_index_changed();
            persist
                .write(&topic, Priority::Default, None, Some(&body))
                .map_err(|error| format!("action result publication failed: {error}"))?;
        }
        let path = action_journal_path(&self.action_state_root, &staged.source_ulid);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if let Err(error) = sync_action_state_root(&self.action_state_root) {
                    tracing::warn!(
                        path = %path.display(),
                        error,
                        "node_grade: published action journal directory sync failed"
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %path.display(),
                error = %error,
                "node_grade: published action journal cleanup deferred"
            ),
        }
        Ok(())
    }

    fn interrupted_action_result(
        &self,
        source_ulid: &str,
        request: HealthActionRequest,
        snapshot_generation: u64,
    ) -> HealthActionResult {
        HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request.request_id,
            condition_id: request.condition_id,
            action: request.action,
            outcome: HealthActionOutcome::Failed,
            detail: "the prior remediation attempt ended after its durable execution claim; the action was not repeated, and current health must be refreshed before retrying".into(),
            audit_id: format!("health:{}:{source_ulid}", self.host),
            completed_at_ms: now_ms(),
            snapshot_generation,
            refreshed_evidence: None,
        }
    }

    fn stage_action_results(
        &self,
        persist: &mut Persist,
    ) -> Result<Vec<StagedActionResult>, String> {
        let paths =
            pending_action_journals(&self.action_state_root, self.trusted_action_owner_uid())?;
        let mut staged = Vec::new();
        for path in paths {
            let Some(record) = read_action_journal(
                &self.action_state_root,
                &path,
                self.trusted_action_owner_uid(),
            )?
            else {
                continue;
            };
            let (source_ulid, result, complete) = match record {
                DurableHealthAction::Complete {
                    source_ulid,
                    result,
                } => (source_ulid, result, None),
                DurableHealthAction::Claimed {
                    source_ulid,
                    request,
                    snapshot_generation,
                    ..
                } => {
                    let result =
                        self.interrupted_action_result(&source_ulid, request, snapshot_generation);
                    let complete = DurableHealthAction::Complete {
                        source_ulid: source_ulid.clone(),
                        result: result.clone(),
                    };
                    (source_ulid, result, Some(complete))
                }
            };
            result
                .validate_at(now_ms())
                .map_err(|error| format!("action result journal is invalid: {error}"))?;
            let already_published = Self::action_result_published(persist, &result)?;
            staged.push(StagedActionResult {
                source_ulid,
                result,
                complete,
                already_published,
            });
        }
        Ok(staged)
    }

    fn flush_staged_action_results(
        &mut self,
        persist: &mut Persist,
        staged: &[StagedActionResult],
    ) -> Result<(), String> {
        for result in staged {
            if let Some(complete) = result.complete.as_ref() {
                self.store_action_journal(&result.source_ulid, complete)?;
            }
            self.publish_action_result(persist, result)?;
            if self
                .action_cursor
                .as_ref()
                .is_none_or(|cursor| result.source_ulid > *cursor)
            {
                self.action_cursor = Some(result.source_ulid.clone());
            }
        }
        Ok(())
    }

    fn activate_bus(
        &mut self,
        persist: &mut Persist,
        identity: &BusIdentity,
    ) -> Result<(), String> {
        if self.active_bus.as_ref() == Some(identity) {
            return Ok(());
        }
        // Capture the destructive-lane boundary before flushing the durable
        // terminal outbox. Activation is committed only if every result lane
        // can be read and every pending result can be made durable on this Bus.
        persist.reopen_if_index_changed();
        let tail = persist
            .latest_ulid(ACTION_TOPIC)
            .map_err(|error| format!("health action activation tail failed: {error}"))?;
        let pending = self.stage_action_results(persist)?;
        self.flush_staged_action_results(persist, &pending)?;
        self.action_cursor = tail;
        // A replacement Bus has no retained health row. Force the next
        // projection to republish even when the semantic health state is
        // unchanged from the retired Bus.
        self.invalidate_publication_acknowledgements();
        self.active_bus = Some(identity.clone());
        Ok(())
    }

    fn invalidate_publication_acknowledgements(&mut self) {
        self.last_health_publication = None;
        self.last_health_file_publication = None;
        self.last_snapshot_file_publication = None;
        self.last_snapshot_bus_publication = None;
    }

    fn action_transaction(&mut self) -> Result<(), String> {
        let (mut persist, identity) = self.open_bus()?;
        self.activate_bus(&mut persist, &identity)?;
        self.drain_actions(&mut persist)
    }

    fn projection_transaction(&mut self) -> Result<SystemMeshHealthSnapshot, String> {
        let (mut persist, identity) = self.open_bus()?;
        self.activate_bus(&mut persist, &identity)?;
        self.cycle(Some(&mut persist))
    }

    fn cycle(&mut self, persist: Option<&mut Persist>) -> Result<SystemMeshHealthSnapshot, String> {
        self.cycle_with_condition_update(persist, None)
    }

    fn cycle_with_condition_update(
        &mut self,
        persist: Option<&mut Persist>,
        condition_update: Option<(&HealthActionRequest, u64)>,
    ) -> Result<SystemMeshHealthSnapshot, String> {
        let now = now_ms();
        let path = node_dir(&self.workgroup_root).join(format!("{}.json", self.host));
        let previous = read_restart_state(&path, &self.host, now);
        // The in-memory counter used to restart at zero. Durable ingress then
        // rejected every post-restart publication until the counter caught up
        // with its retained high-water mark (hours on a long-running seat).
        // A canonical row carries both the prior counter and its Unix-ms
        // publication time; either is a safe monotonic floor when capped at the
        // current clock. This also repairs nodes whose first broken restart
        // already overwrote the row with a low counter but a fresh timestamp.
        let durable_restart_floor = previous.as_ref().map_or(0, |state| {
            state.generation.max(state.published_at_ms).min(now)
        });
        let generation_floor = if self.generation == 0 {
            durable_restart_floor
        } else {
            self.generation
                .max(previous.as_ref().map_or(0, |state| state.generation))
        };
        let generation = generation_floor.saturating_add(1);
        let observations = self.sampler.sample();
        let mut pressure = self.pressure.clone();
        pressure.observe(observations.resources);
        let mut active = evaluate_conditions(&self.host, &observations, &pressure, generation, now);
        let resolved = merge_lifecycle(previous.as_ref(), &mut active, now);
        if let Some((request, updated_at_ms)) = condition_update {
            if let Some(condition) = active.iter_mut().find(|condition| {
                condition.id == request.condition_id && condition.scope == request.target
            }) {
                match request.action {
                    HealthAction::Acknowledge => {
                        condition.acknowledged_at_ms = Some(updated_at_ms);
                    }
                    HealthAction::SnoozeOneHour => {
                        condition.snoozed_until_ms = Some(updated_at_ms.saturating_add(3_600_000));
                    }
                    _ => {}
                }
            }
        }
        let grade_factors = factors(&self.host, &observations, now);
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
            generation,
            published_at_ms: now,
            valid_until_ms: now.saturating_add(PUBLICATION_VALIDITY_MS),
            grade,
            active_conditions: active.clone(),
            resolved_conditions: resolved,
        };
        let health_key = health_publication_key(&state);
        let publish_health =
            self.last_health_publication
                .as_ref()
                .is_none_or(|(previous, published_at_ms)| {
                    previous != &health_key
                        || now.saturating_sub(*published_at_ms) >= HEALTH_PUBLICATION_HEARTBEAT_MS
                });
        #[cfg(test)]
        let inject_snapshot_file_failure = self.fail_snapshot_file_writes > 0;
        #[cfg(not(test))]
        let inject_snapshot_file_failure = false;
        let health_file_due =
            self.last_health_file_publication
                .as_ref()
                .is_none_or(|(previous, published_at_ms)| {
                    previous != &health_key
                        || now.saturating_sub(*published_at_ms) >= HEALTH_PUBLICATION_HEARTBEAT_MS
                })
                || inject_snapshot_file_failure;
        let previous_critical: BTreeSet<_> = previous
            .iter()
            .flat_map(|old| old.active_conditions.iter())
            .filter(|condition| {
                condition.requirement == RequirementClass::Required
                    && condition.severity == HealthSeverity::Critical
            })
            .map(|condition| condition.id.as_str())
            .collect();
        let mut publications = read_canonical_states(&self.workgroup_root);
        publications.retain(|published| published.publisher != self.host);
        publications.push(state.clone());
        let durable_snapshot = self.last_snapshot.clone().or_else(|| {
            read_snapshot(&snapshot_path(&self.workgroup_root, &self.host)).filter(|snapshot| {
                snapshot.observer == self.host
                    && snapshot.roster_revision == observations.roster_revision
                    && snapshot.generated_at_ms <= now
                    && snapshot.is_fresh(snapshot.generated_at_ms)
            })
        });
        let mut snapshot = fold_health_snapshot(
            &self.workgroup_root,
            &self.host,
            observations.roster_revision,
            &observations.canonical_nodes,
            publications,
            generation,
            now,
            observations.reachable_lighthouses,
        );
        merge_availability_lifecycle(&mut snapshot, durable_snapshot.as_ref(), now);
        let snapshot_key = snapshot_publication_key(&snapshot);
        // The injected failure exercises a real partial canonical transaction:
        // the node row is durable, the snapshot write fails, and no Bus
        // projection is acknowledged. The retry must therefore republish both
        // files and both Bus rows at a strictly newer generation.
        let snapshot_file_due = self.last_snapshot_file_publication.as_ref().is_none_or(
            |(previous, published_at_ms)| {
                previous != &snapshot_key
                    || now.saturating_sub(*published_at_ms) >= SNAPSHOT_PUBLICATION_HEARTBEAT_MS
            },
        ) || inject_snapshot_file_failure;
        let snapshot_bus_due = self.last_snapshot_bus_publication.as_ref().is_none_or(
            |(previous, published_at_ms)| {
                previous != &snapshot_key
                    || now.saturating_sub(*published_at_ms) >= SNAPSHOT_PUBLICATION_HEARTBEAT_MS
            },
        );
        if health_file_due {
            write_json_atomic(&path, &state)
                .map_err(|error| format!("node health state publication failed: {error}"))?;
            self.last_health_file_publication = Some((health_key.clone(), now));
        }
        if snapshot_file_due {
            #[cfg(test)]
            if self.fail_snapshot_file_writes > 0 {
                self.fail_snapshot_file_writes -= 1;
                self.invalidate_publication_acknowledgements();
                return Err("injected canonical snapshot publication failure".into());
            }
            if let Err(error) =
                write_json_atomic(&snapshot_path(&self.workgroup_root, &self.host), &snapshot)
            {
                self.invalidate_publication_acknowledgements();
                return Err(format!("health snapshot publication failed: {error}"));
            }
            self.last_snapshot_file_publication = Some((snapshot_key.clone(), now));
        }
        if let Some(bus) = persist {
            if publish_health {
                emit_json(bus, &node_health_topic(&self.host), &state)?;
                self.last_health_publication = Some((health_key, now));
            }
            if snapshot_bus_due {
                emit_json(bus, SNAPSHOT_TOPIC, &snapshot)?;
                self.last_snapshot_bus_publication = Some((snapshot_key, now));
            }
            for condition in &active {
                if condition.requirement == RequirementClass::Required
                    && condition.severity == HealthSeverity::Critical
                    && !previous_critical.contains(condition.id.as_str())
                {
                    if let Err(error) = emit_critical(bus, condition, &self.host) {
                        tracing::warn!(error, "node_grade: critical notification deferred");
                    }
                }
            }
        }
        self.generation = generation;
        self.pressure = pressure;
        self.last_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    fn stage_actions(&self, persist: &mut Persist) -> Result<Vec<StagedActionMessage>, String> {
        persist.reopen_if_index_changed();
        let messages = persist
            .list_since(ACTION_TOPIC, self.action_cursor.as_deref())
            .map_err(|error| format!("health action lane unreadable: {error}"))?;
        let mut staged = Vec::new();
        for message in messages {
            if !valid_action_source_ulid(&message.ulid) {
                staged.push(StagedActionMessage::Terminal(message.ulid));
                continue;
            }
            let Some(body) = message.body.as_deref() else {
                staged.push(StagedActionMessage::Terminal(message.ulid));
                continue;
            };
            let Ok(request) = serde_json::from_str::<HealthActionRequest>(body) else {
                staged.push(StagedActionMessage::Terminal(message.ulid));
                continue;
            };
            let audit_id = format!("health:{}:{}", self.host, message.ulid);
            let topic = action_result_topic(&request.request_id);
            persist.reopen_if_index_changed();
            let rows = persist
                .list_since(&topic, None)
                .map_err(|error| format!("action result lane {topic} unreadable: {error}"))?;
            let mut published_result = None;
            for row in rows {
                let body = row
                    .body
                    .as_deref()
                    .ok_or_else(|| format!("action result lane {topic} contains an empty row"))?;
                let result: HealthActionResult = serde_json::from_str(body)
                    .map_err(|error| format!("action result lane {topic} is invalid: {error}"))?;
                result
                    .validate_at(now_ms())
                    .map_err(|error| format!("action result lane {topic} is invalid: {error}"))?;
                if result.audit_id == audit_id {
                    if result.request_id != request.request_id
                        || result.condition_id != request.condition_id
                        || result.action != request.action
                    {
                        return Err(format!(
                            "action result lane {topic} contains a conflicting request body for audit id {audit_id}"
                        ));
                    }
                    if published_result
                        .as_ref()
                        .is_some_and(|published| published != &result)
                    {
                        return Err(format!(
                            "action result lane {topic} contains conflicting bodies for audit id {audit_id}"
                        ));
                    }
                    published_result = Some(result);
                }
            }
            staged.push(StagedActionMessage::Request {
                source_ulid: message.ulid,
                request,
                result_already_published: published_result.is_some(),
            });
        }
        Ok(staged)
    }

    fn drain_actions(&mut self, persist: &mut Persist) -> Result<(), String> {
        if self.last_snapshot.is_none() {
            return Ok(());
        }
        // Complete every required ingress read before claims, remediations, or
        // cursor changes. One unreadable action/result lane defers the sweep.
        let pending_results = self.stage_action_results(persist)?;
        let messages = self.stage_actions(persist)?;
        self.flush_staged_action_results(persist, &pending_results)?;
        for message in messages {
            let StagedActionMessage::Request {
                source_ulid,
                request,
                result_already_published,
            } = message
            else {
                let StagedActionMessage::Terminal(source_ulid) = message else {
                    unreachable!()
                };
                self.action_cursor = Some(source_ulid);
                continue;
            };
            if self
                .action_cursor
                .as_ref()
                .is_some_and(|cursor| cursor >= &source_ulid)
            {
                continue;
            }
            if result_already_published {
                self.action_cursor = Some(source_ulid);
                continue;
            }
            let journal_path = action_journal_path(&self.action_state_root, &source_ulid);
            match read_action_journal(
                &self.action_state_root,
                &journal_path,
                self.trusted_action_owner_uid(),
            ) {
                Ok(Some(DurableHealthAction::Complete {
                    source_ulid: record_source,
                    result,
                })) if record_source == source_ulid
                    && result.request_id == request.request_id
                    && result.condition_id == request.condition_id
                    && result.action == request.action =>
                {
                    let staged = StagedActionResult {
                        source_ulid: record_source,
                        result,
                        complete: None,
                        already_published: false,
                    };
                    self.publish_action_result(persist, &staged)?;
                    self.action_cursor = Some(source_ulid);
                    continue;
                }
                Ok(Some(DurableHealthAction::Claimed {
                    source_ulid: record_source,
                    request: claimed_request,
                    snapshot_generation,
                    ..
                })) if record_source == source_ulid && claimed_request == request => {
                    let result = self.interrupted_action_result(
                        &record_source,
                        claimed_request,
                        snapshot_generation,
                    );
                    let complete = DurableHealthAction::Complete {
                        source_ulid: record_source.clone(),
                        result: result.clone(),
                    };
                    if let Err(error) = self.store_action_journal(&record_source, &complete) {
                        tracing::warn!(
                            source_ulid = record_source,
                            error,
                            "node_grade: interrupted action remains recoverable"
                        );
                        break;
                    }
                    let staged = StagedActionResult {
                        source_ulid: record_source,
                        result,
                        complete: None,
                        already_published: false,
                    };
                    self.publish_action_result(persist, &staged)?;
                    self.action_cursor = Some(source_ulid);
                    continue;
                }
                Ok(Some(_)) => {
                    tracing::error!(
                        source_ulid,
                        "node_grade: action journal identity mismatch; refusing execution"
                    );
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::error!(
                        source_ulid,
                        error,
                        "node_grade: unreadable action journal; refusing execution"
                    );
                    break;
                }
            }
            let Some(snapshot) = self.last_snapshot.clone() else {
                break;
            };
            let decision = authorize_action(&request, &self.host, &snapshot);
            let condition_source = snapshot
                .active_conditions
                .iter()
                .find(|condition| condition.id == request.condition_id)
                .map_or("", |condition| condition.source.as_str());
            if decision == ActionDecision::Apply {
                let claim = DurableHealthAction::Claimed {
                    source_ulid: source_ulid.clone(),
                    request: request.clone(),
                    snapshot_generation: snapshot.generation,
                    claimed_at_ms: now_ms(),
                };
                if let Err(error) = self.store_action_journal(&source_ulid, &claim) {
                    tracing::warn!(
                        source_ulid,
                        error,
                        "node_grade: action execution deferred until its claim is durable"
                    );
                    break;
                }
            }
            let (outcome, detail) = match decision {
                ActionDecision::Refuse(reason) => (HealthActionOutcome::Refused, reason.into()),
                ActionDecision::Stale => (
                    HealthActionOutcome::StaleGeneration,
                    "snapshot generation changed; refresh before applying".into(),
                ),
                ActionDecision::Apply => {
                    #[cfg(test)]
                    {
                        self.action_execution_count += 1;
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
            let refreshed_snapshot = if outcome == HealthActionOutcome::Applied {
                let condition_update = matches!(
                    request.action,
                    HealthAction::Acknowledge | HealthAction::SnoozeOneHour
                )
                .then(|| (&request, now_ms()));
                Some(self.cycle_with_condition_update(Some(persist), condition_update)?)
            } else {
                None
            };
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
                audit_id: format!("health:{}:{}", self.host, source_ulid),
                completed_at_ms: now_ms(),
                snapshot_generation: result_generation,
                refreshed_evidence,
            };
            result
                .validate_at(now_ms())
                .map_err(|error| format!("action result generation refused: {error}"))?;
            let complete = DurableHealthAction::Complete {
                source_ulid: source_ulid.clone(),
                result: result.clone(),
            };
            if let Err(error) = self.store_action_journal(&source_ulid, &complete) {
                tracing::warn!(
                    source_ulid,
                    error,
                    "node_grade: terminal result storage failed; durable claim retained"
                );
                break;
            }
            let staged = StagedActionResult {
                source_ulid: source_ulid.clone(),
                result,
                complete: None,
                already_published: false,
            };
            self.publish_action_result(persist, &staged)?;
            self.action_cursor = Some(source_ulid);
        }
        Ok(())
    }
}

/// Remove publication clock fields from a health row before comparing it with
/// the last Bus publication. Generation and timestamps advance on every local
/// sample; condition/grade changes remain immediately publishable.
fn health_publication_key(state: &NodeHealthState) -> NodeHealthState {
    let mut key = state.clone();
    key.generation = 0;
    key.published_at_ms = 0;
    key.valid_until_ms = 0;
    key.grade.evaluated_at_ms = 0;
    normalize_condition_timestamps(&mut key.active_conditions);
    normalize_condition_timestamps(&mut key.resolved_conditions);
    key
}

/// Remove publication clock fields before comparing folded snapshots. The
/// health contents remain immediately publishable while steady snapshots use
/// the bounded validity-preserving heartbeat.
fn snapshot_publication_key(snapshot: &SystemMeshHealthSnapshot) -> SystemMeshHealthSnapshot {
    let mut key = snapshot.clone();
    key.generation = 0;
    key.generated_at_ms = 0;
    key.fresh_until_ms = 0;
    for grade in &mut key.current_node_grades {
        grade.evaluated_at_ms = 0;
    }
    normalize_condition_timestamps(&mut key.active_conditions);
    normalize_condition_timestamps(&mut key.resolved_conditions);
    key
}

fn normalize_condition_timestamps(conditions: &mut [HealthCondition]) {
    for condition in conditions {
        condition.last_observed_ms = 0;
        condition.evidence.observed_at_ms = 0;
        for remediation in &mut condition.remediation {
            remediation.expected_snapshot_generation = 0;
        }
    }
}

#[async_trait::async_trait]
impl Worker for NodeGradeWorker {
    fn name(&self) -> &'static str {
        "node_grade"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Anchor the existing 10-second interval at a deterministic per-host
        // phase. Shutdown remains prompt while the initial offset is pending;
        // every later cycle retains the original freshness and action cadence.
        tokio::select! {
            () = tokio::time::sleep(initial_phase_for(&self.host)) => {}
            () = shutdown.wait() => return Ok(()),
        }
        if let Err(error) = self.projection_transaction() {
            tracing::warn!(error, "node_grade: initial Bus projection deferred");
        }
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(error) = self.action_transaction() {
                        tracing::warn!(error, "node_grade: action transaction deferred");
                    }
                    if let Err(error) = self.projection_transaction() {
                        tracing::warn!(error, "node_grade: Bus projection deferred");
                    }
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
    use mackes_mesh_types::health::fold_snapshot;

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
    fn resolution_preserves_last_positive_observation_before_recovery() {
        let sample = observations("workstation");
        let mut active = condition(
            "node",
            "root-space",
            HealthComponent::Resources,
            "root-filesystem",
            HealthSeverity::Warning,
            "Root filesystem utilization breached policy.",
            100,
            Vec::new(),
        );
        active.last_observed_ms = 140;
        active.evidence.observed_at_ms = 140;
        let previous = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: "node".into(),
            roster_revision: "r1".into(),
            generation: 7,
            published_at_ms: 150,
            valid_until_ms: 270,
            grade: NodeGrade::evaluate(
                "node",
                100,
                factors("node", &sample, 150),
                std::slice::from_ref(&active),
                150,
            ),
            active_conditions: vec![active],
            resolved_conditions: Vec::new(),
        };

        let resolved = merge_lifecycle(Some(&previous), &mut [], 200);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].last_observed_ms, 140);
        assert_eq!(resolved[0].evidence.observed_at_ms, 140);
        assert_eq!(resolved[0].resolved_at_ms, Some(200));
    }

    #[test]
    fn restart_prunes_resolved_history_outside_the_six_hour_privacy_epoch() {
        let now = HEALTH_HISTORY_RETENTION_MS.saturating_add(10_000);
        let sample = observations("workstation");
        let resolved_condition = |id: &str, resolved_at_ms: u64| {
            let mut condition = condition(
                "node",
                id,
                HealthComponent::Resources,
                "history-fixture",
                HealthSeverity::Warning,
                "Resolved history fixture.",
                resolved_at_ms.saturating_sub(1),
                Vec::new(),
            );
            condition.last_observed_ms = resolved_at_ms.saturating_sub(1);
            condition.evidence.observed_at_ms = resolved_at_ms.saturating_sub(1);
            condition.resolved_at_ms = Some(resolved_at_ms);
            condition
        };
        let previous = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: "node".into(),
            roster_revision: "r1".into(),
            generation: 7,
            published_at_ms: now.saturating_sub(1),
            valid_until_ms: now.saturating_add(PUBLICATION_VALIDITY_MS),
            grade: NodeGrade::evaluate("node", 100, factors("node", &sample, now), &[], now),
            active_conditions: Vec::new(),
            resolved_conditions: vec![
                resolved_condition("recent", now.saturating_sub(1_000)),
                resolved_condition("inclusive-boundary", now - HEALTH_HISTORY_RETENTION_MS),
                resolved_condition(
                    "expired",
                    now.saturating_sub(HEALTH_HISTORY_RETENTION_MS.saturating_add(1)),
                ),
                resolved_condition("future", now.saturating_add(1)),
            ],
        };

        let retained = merge_lifecycle(Some(&previous), &mut [], now);
        let retained_ids: BTreeSet<_> = retained
            .iter()
            .map(|condition| condition.id.as_str())
            .collect();

        assert_eq!(
            retained_ids,
            BTreeSet::from(["node:inclusive-boundary", "node:recent"]),
            "restart must not carry expired, unresolved, or future-dated history into a fresh authority row"
        );
    }

    #[test]
    fn declared_seat_absence_escalates_only_after_its_return_deadline() {
        use super::super::node_availability::runtime_availability_path;
        use mackes_mesh_types::health::{
            ExpectedReturn, NodeAvailabilityIntent, NodeAvailabilityState, NodeConnectionType,
            NodeDeviceClass, NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
        };

        let workgroup = tempfile::tempdir().expect("workgroup");
        let node = "seat-remote";
        let intent = NodeAvailabilityIntent {
            schema_version: NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
            node_id: node.into(),
            device_id: "seat-remote-device".into(),
            device_class: NodeDeviceClass::Laptop,
            connection_type: NodeConnectionType::Wifi,
            state: NodeAvailabilityState::Sleeping,
            reason: "local seat suspend".into(),
            source: "host-state".into(),
            event_id: "sleep-1".into(),
            generation: 1,
            observed_at_ms: 1_000,
            expires_at_ms: 500_000,
            expected_return: Some(ExpectedReturn::new(31_000)),
            old_connectivity: None,
            new_connectivity: None,
        };
        write_json_atomic(&runtime_availability_path(workgroup.path(), node), &intent)
            .expect("write lifecycle intent");
        let canonical = BTreeSet::from([node.to_string()]);
        let fold_at = |now_ms| {
            fold_health_snapshot(
                workgroup.path(),
                "observer",
                "r1".into(),
                &canonical,
                Vec::new(),
                1,
                now_ms,
                1,
            )
        };

        let expected = fold_at(40_000);
        assert_eq!(expected.active_conditions.len(), 1);
        assert_eq!(
            expected.active_conditions[0].severity,
            HealthSeverity::Warning
        );
        assert_eq!(
            expected.active_conditions[0].requirement,
            RequirementClass::Informational
        );
        assert_ne!(expected.active_conditions[0].id, "mesh:publisher-freshness");

        let warning = fold_at(91_000);
        assert_eq!(warning.active_conditions.len(), 1);
        assert_eq!(
            warning.active_conditions[0].severity,
            HealthSeverity::Warning
        );

        let critical = fold_at(331_000);
        assert_eq!(critical.active_conditions.len(), 1);
        assert_eq!(
            critical.active_conditions[0].severity,
            HealthSeverity::Critical
        );
    }

    #[test]
    fn availability_lifecycle_survives_refresh_and_records_return_history() {
        use super::super::node_availability::runtime_availability_path;
        use mackes_mesh_types::health::{
            ExpectedReturn, NodeAvailabilityIntent, NodeAvailabilityState, NodeConnectionType,
            NodeDeviceClass, NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
        };

        let workgroup = tempfile::tempdir().expect("workgroup");
        let node = "seat-remote";
        let intent = NodeAvailabilityIntent {
            schema_version: NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
            node_id: node.into(),
            device_id: "seat-remote-device".into(),
            device_class: NodeDeviceClass::Laptop,
            connection_type: NodeConnectionType::Wifi,
            state: NodeAvailabilityState::Sleeping,
            reason: "local seat suspend".into(),
            source: "host-state".into(),
            event_id: "sleep-history-1".into(),
            generation: 1,
            observed_at_ms: 1_000,
            expires_at_ms: 500_000,
            expected_return: Some(ExpectedReturn::new(31_000)),
            old_connectivity: None,
            new_connectivity: None,
        };
        write_json_atomic(&runtime_availability_path(workgroup.path(), node), &intent)
            .expect("write lifecycle intent");
        let canonical = BTreeSet::from([node.to_string()]);
        let mut first = fold_health_snapshot(
            workgroup.path(),
            "observer",
            "r1".into(),
            &canonical,
            Vec::new(),
            1,
            40_000,
            1,
        );
        first.active_conditions[0].acknowledged_at_ms = Some(42_000);

        let mut escalated = fold_health_snapshot(
            workgroup.path(),
            "observer",
            "r1".into(),
            &canonical,
            Vec::new(),
            2,
            91_000,
            1,
        );
        merge_availability_lifecycle(&mut escalated, Some(&first), 91_000);
        assert_eq!(escalated.active_conditions[0].active_since_ms, 40_000);
        assert_eq!(
            escalated.active_conditions[0].acknowledged_at_ms,
            Some(42_000)
        );
        assert_eq!(
            escalated.active_conditions[0].severity,
            HealthSeverity::Warning
        );

        let mut returned = escalated.clone();
        returned.generation = 3;
        returned.generated_at_ms = 100_000;
        returned.fresh_until_ms = 130_000;
        returned.active_conditions.clear();
        returned.resolved_conditions.clear();
        merge_availability_lifecycle(&mut returned, Some(&escalated), 100_000);

        assert_eq!(returned.resolved_conditions.len(), 1);
        let history = &returned.resolved_conditions[0];
        assert_eq!(history.active_since_ms, 40_000);
        assert_eq!(history.last_observed_ms, 91_000);
        assert_eq!(history.resolved_at_ms, Some(100_000));
        assert_eq!(history.acknowledged_at_ms, Some(42_000));
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
        assert_eq!(factors("node", &sample, 100).system, Some(100));
    }

    #[test]
    fn future_device_inventory_cannot_publish_an_a_grade() {
        let now = 1_000;
        let mut sample = observations("workstation");
        sample.device_inventory.as_mut().unwrap().published_at_ms = now + 1;

        let conditions = evaluate_conditions("node", &sample, &PressureWindow::default(), 1, now);
        let grade_factors = factors("node", &sample, now);
        let grade = NodeGrade::evaluate(
            "node",
            capability_score(grade_factors),
            grade_factors,
            &conditions,
            now,
        );

        assert_eq!(grade.factors.devices, None);
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].id, "node:device-evidence-invalid");
        assert_eq!(grade.grade, mackes_mesh_types::health::GradeLetter::D);
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
            NodeGrade::evaluate("node", 70, factors("node", &sample, 100), &[], 100)
                .grade
                .as_str(),
            "C"
        );
    }

    #[test]
    fn worker_evidence_produces_e_for_compounded_distinct_warnings() {
        let mut sample = observations("workstation");
        sample.services.insert("workbench".into(), false);
        sample.audio = None;
        let conditions = evaluate_conditions("node", &sample, &PressureWindow::default(), 1, 100);
        assert!(conditions
            .iter()
            .any(|condition| condition.id == "node:required-service-workbench"));
        assert!(conditions
            .iter()
            .any(|condition| condition.id == "node:workstation-audio"));
        assert_eq!(
            NodeGrade::evaluate("node", 99, factors("node", &sample, 100), &conditions, 100).grade,
            mackes_mesh_types::health::GradeLetter::E
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
            grade: NodeGrade::evaluate("node", 100, factors("node", &sample, 100), &[], 100),
            active_conditions: Vec::new(),
            resolved_conditions: Vec::new(),
        };
        write_json_atomic(&nodes.join("node.sync-conflict-1.json"), &state).unwrap();
        assert!(read_canonical_states(dir.path()).is_empty());
    }

    #[test]
    fn restart_generation_uses_durable_publication_floor_after_counter_rollback() {
        let workgroup = tempfile::tempdir().unwrap();
        let nodes = node_dir(workgroup.path());
        std::fs::create_dir_all(&nodes).unwrap();
        let now = now_ms();
        let published_at_ms = now.saturating_sub(1);
        let sample = observations("workstation");
        let stale_counter = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: "node".into(),
            roster_revision: "r1".into(),
            generation: 136,
            published_at_ms,
            valid_until_ms: published_at_ms.saturating_add(PUBLICATION_VALIDITY_MS),
            grade: NodeGrade::evaluate(
                "node",
                100,
                factors("node", &sample, published_at_ms),
                &[],
                published_at_ms,
            ),
            active_conditions: Vec::new(),
            resolved_conditions: Vec::new(),
        };
        write_json_atomic(&nodes.join("node.json"), &stale_counter).unwrap();
        let mut worker = NodeGradeWorker {
            host: "node".into(),
            workgroup_root: workgroup.path().to_path_buf(),
            sampler: Box::new(FixtureSampler(sample)),
            bus_root_override: None,
            active_bus: None,
            action_state_root: workgroup.path().join("local-action-state"),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
            last_health_publication: None,
            last_health_file_publication: None,
            last_snapshot_file_publication: None,
            last_snapshot_bus_publication: None,
            fail_terminal_result_writes: 0,
            fail_snapshot_file_writes: 0,
            action_execution_count: 0,
            trusted_action_owner_uid: rustix::process::geteuid().as_raw(),
        };

        worker.cycle(None).unwrap();

        let recovered = read_state(&nodes.join("node.json")).unwrap();
        assert!(
            recovered.generation > 4_527,
            "a restarted producer must immediately clear the retained ingress high-water"
        );
        assert!(recovered.generation > stale_counter.generation);
    }

    #[test]
    fn foreign_restart_row_cannot_seed_local_health_generation_or_lifecycle() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let nodes = node_dir(workgroup.path());
        std::fs::create_dir_all(&nodes).unwrap();
        let published_at_ms = now_ms().saturating_sub(1);
        let mut foreign_sample = observations("workstation");
        foreign_sample.device_inventory.as_mut().unwrap().host = "foreign-node".into();
        foreign_sample
            .device_inventory
            .as_mut()
            .unwrap()
            .published_at_ms = published_at_ms;
        let foreign_condition = condition(
            "foreign-node",
            "fabricated-critical",
            HealthComponent::Evidence,
            "replicated-replacement",
            HealthSeverity::Critical,
            "A foreign producer replaced the local canonical filename.",
            published_at_ms,
            Vec::new(),
        );
        let foreign = NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: "foreign-node".into(),
            roster_revision: "r1".into(),
            generation: published_at_ms,
            published_at_ms,
            valid_until_ms: published_at_ms.saturating_add(PUBLICATION_VALIDITY_MS),
            grade: NodeGrade::evaluate(
                "foreign-node",
                100,
                factors("foreign-node", &foreign_sample, published_at_ms),
                std::slice::from_ref(&foreign_condition),
                published_at_ms,
            ),
            active_conditions: vec![foreign_condition],
            resolved_conditions: Vec::new(),
        };
        assert!(foreign.validate_at(published_at_ms).is_ok());
        write_json_atomic(&nodes.join("node.json"), &foreign).unwrap();

        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());
        let snapshot = worker.cycle(Some(&mut persist)).unwrap();
        let recovered = read_state(&nodes.join("node.json")).unwrap();

        assert_eq!(recovered.publisher, "node");
        assert_eq!(recovered.generation, 1);
        assert!(
            recovered.resolved_conditions.is_empty(),
            "foreign lifecycle state must not be rewritten as local resolved history"
        );
        assert!(recovered.validate_at(now_ms()).is_ok());
        assert!(snapshot
            .current_node_grades
            .iter()
            .any(|grade| grade.node == "node"));
    }

    #[test]
    fn steady_health_reuses_bus_row_until_bounded_heartbeat() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());

        worker.cycle(Some(&mut persist)).unwrap();
        worker.cycle(Some(&mut persist)).unwrap();

        assert_eq!(
            persist
                .list_since(&node_health_topic("node"), None)
                .unwrap()
                .len(),
            1,
            "unchanged condition state is not appended every 10-second sample"
        );
        assert_eq!(
            persist.list_since(SNAPSHOT_TOPIC, None).unwrap().len(),
            1,
            "unchanged folded snapshots are not appended every 10-second sample"
        );
    }

    #[test]
    fn partial_canonical_publication_retries_forward_before_bus_projection() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());

        let first = worker.cycle(Some(&mut persist)).unwrap();
        worker.fail_snapshot_file_writes = 1;
        assert!(worker.cycle(Some(&mut persist)).is_err());

        let partial = read_state(&node_dir(workgroup.path()).join("node.json")).unwrap();
        let retained_snapshot: SystemMeshHealthSnapshot = serde_json::from_slice(
            &std::fs::read(snapshot_path(workgroup.path(), "node")).unwrap(),
        )
        .unwrap();
        assert_eq!(partial.generation, first.generation.saturating_add(1));
        assert_eq!(retained_snapshot.generation, first.generation);
        assert_eq!(worker.generation, first.generation);

        let bus_node_before = persist
            .list_since(&node_health_topic("node"), None)
            .unwrap();
        let bus_snapshot_before = persist.list_since(SNAPSHOT_TOPIC, None).unwrap();
        assert_eq!(bus_node_before.len(), 1);
        assert_eq!(bus_snapshot_before.len(), 1);

        let repaired = worker.cycle(Some(&mut persist)).unwrap();
        assert_eq!(
            repaired.generation,
            partial.generation.saturating_add(1),
            "retry must advance beyond the partially durable generation"
        );
        let bus_node_generations: Vec<u64> = persist
            .list_since(&node_health_topic("node"), None)
            .unwrap()
            .into_iter()
            .map(|message| {
                serde_json::from_str::<NodeHealthState>(message.body.as_deref().unwrap())
                    .unwrap()
                    .generation
            })
            .collect();
        let bus_snapshot_generations: Vec<u64> = persist
            .list_since(SNAPSHOT_TOPIC, None)
            .unwrap()
            .into_iter()
            .map(|message| {
                serde_json::from_str::<SystemMeshHealthSnapshot>(message.body.as_deref().unwrap())
                    .unwrap()
                    .generation
            })
            .collect();
        assert_eq!(
            bus_node_generations,
            vec![first.generation, repaired.generation]
        );
        assert_eq!(bus_snapshot_generations, bus_node_generations);
        assert!(!bus_node_generations.contains(&partial.generation));
    }

    #[test]
    fn applied_actions_emit_audited_results_with_refreshed_evidence() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut sample = observations("workstation");
        sample.services.insert("workbench".into(), false);
        sample.device_inventory.as_mut().unwrap().published_at_ms = now_ms();
        let mut worker = NodeGradeWorker {
            host: "node".into(),
            workgroup_root: workgroup.path().to_path_buf(),
            sampler: Box::new(FixtureSampler(sample)),
            bus_root_override: Some(bus.path().to_path_buf()),
            active_bus: None,
            action_state_root: workgroup.path().join("local-action-state"),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
            last_health_publication: None,
            last_health_file_publication: None,
            last_snapshot_file_publication: None,
            last_snapshot_bus_publication: None,
            fail_terminal_result_writes: 0,
            fail_snapshot_file_writes: 0,
            action_execution_count: 0,
            trusted_action_owner_uid: rustix::process::geteuid().as_raw(),
        };
        let snapshot = worker.cycle(Some(&mut persist)).unwrap();
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

        worker.drain_actions(&mut persist).unwrap();

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

    fn action_test_worker(workgroup_root: &Path, bus_root: &Path) -> NodeGradeWorker {
        let mut sample = observations("workstation");
        sample.services.insert("workbench".into(), false);
        sample.device_inventory.as_mut().unwrap().published_at_ms = now_ms();
        NodeGradeWorker {
            host: "node".into(),
            workgroup_root: workgroup_root.to_path_buf(),
            sampler: Box::new(FixtureSampler(sample)),
            bus_root_override: Some(bus_root.to_path_buf()),
            active_bus: None,
            action_state_root: workgroup_root.join("local-action-state"),
            poll: DEFAULT_POLL_INTERVAL,
            generation: 0,
            pressure: PressureWindow::default(),
            action_cursor: None,
            last_snapshot: None,
            last_health_publication: None,
            last_health_file_publication: None,
            last_snapshot_file_publication: None,
            last_snapshot_bus_publication: None,
            fail_terminal_result_writes: 0,
            fail_snapshot_file_writes: 0,
            action_execution_count: 0,
            trusted_action_owner_uid: rustix::process::geteuid().as_raw(),
        }
    }

    #[test]
    fn bus_roots_use_current_then_canonical_system_fallback() {
        let current = PathBuf::from("/tmp/current-node-grade-bus");
        assert_eq!(
            node_grade_bus_roots(None, Some(current.clone())),
            vec![current, PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)]
        );
        assert_eq!(
            node_grade_bus_roots(Some(Path::new("/tmp/explicit-node-grade-bus")), None),
            vec![PathBuf::from("/tmp/explicit-node-grade-bus")]
        );
    }

    #[test]
    fn late_and_replaced_bus_activation_skips_retained_and_executes_forward_once() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus_parent = tempfile::tempdir().unwrap();
        let bus_root = bus_parent.path().join("bus");
        std::fs::write(&bus_root, b"temporarily unopenable").unwrap();
        let mut worker = action_test_worker(workgroup.path(), &bus_root);

        assert!(worker.action_transaction().is_err());
        assert!(worker.active_bus.is_none());

        std::fs::remove_file(&bus_root).unwrap();
        let retained_bus = Persist::open(bus_root.clone()).unwrap();
        let retained = request(HealthAction::Acknowledge);
        retained_bus
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&retained).unwrap()),
            )
            .unwrap();
        worker.action_transaction().unwrap();
        worker.projection_transaction().unwrap();
        assert!(retained_bus
            .list_since(&action_result_topic(&retained.request_id), None)
            .unwrap()
            .is_empty());

        let forward = acknowledge_request(worker.last_snapshot.as_ref().unwrap(), "forward-one");
        retained_bus
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward).unwrap()),
            )
            .unwrap();
        worker.action_transaction().unwrap();
        worker.action_transaction().unwrap();
        assert_eq!(worker.action_execution_count, 1);
        assert_eq!(
            retained_bus
                .list_since(&action_result_topic(&forward.request_id), None)
                .unwrap()
                .len(),
            1
        );
        drop(retained_bus);

        let replacement_root = bus_parent.path().join("replacement");
        let replacement = Persist::open(replacement_root.clone()).unwrap();
        let retained_replacement = acknowledge_request(
            worker.last_snapshot.as_ref().unwrap(),
            "retained-replacement",
        );
        replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&retained_replacement).unwrap()),
            )
            .unwrap();
        drop(replacement);
        std::fs::rename(&bus_root, bus_parent.path().join("old-bus")).unwrap();
        std::fs::rename(&replacement_root, &bus_root).unwrap();

        worker.action_transaction().unwrap();
        assert_eq!(worker.action_execution_count, 1);
        let replacement = Persist::open(bus_root.clone()).unwrap();
        assert!(replacement
            .list_since(&action_result_topic(&retained_replacement.request_id), None)
            .unwrap()
            .is_empty());

        worker.projection_transaction().unwrap();
        let forward_replacement = acknowledge_request(
            worker.last_snapshot.as_ref().unwrap(),
            "forward-replacement",
        );
        replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward_replacement).unwrap()),
            )
            .unwrap();
        worker.action_transaction().unwrap();
        worker.action_transaction().unwrap();
        assert_eq!(worker.action_execution_count, 2);
        assert_eq!(
            replacement
                .list_since(&action_result_topic(&forward_replacement.request_id), None)
                .unwrap()
                .len(),
            1
        );
    }

    fn acknowledge_request(
        snapshot: &SystemMeshHealthSnapshot,
        request_id: &str,
    ) -> HealthActionRequest {
        let condition = snapshot
            .active_conditions
            .iter()
            .find(|condition| condition.id.ends_with("required-service-workbench"))
            .unwrap();
        HealthActionRequest {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request_id.into(),
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
        }
    }

    #[test]
    fn terminal_result_storage_failure_recovers_without_repeating_mutation() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());
        let snapshot = worker.cycle(Some(&mut persist)).unwrap();
        let request = acknowledge_request(&snapshot, "storage-failure-request");
        let message = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).unwrap()),
            )
            .unwrap();
        worker.fail_terminal_result_writes = 1;
        let journal_path = action_journal_path(&worker.action_state_root, &message.ulid);
        let trusted_uid = worker.trusted_action_owner_uid();

        worker.drain_actions(&mut persist).unwrap();

        assert_eq!(worker.action_execution_count, 1);
        assert_eq!(worker.action_cursor, None);
        assert!(matches!(
            read_action_journal(&worker.action_state_root, &journal_path, trusted_uid),
            Ok(Some(DurableHealthAction::Claimed { .. }))
        ));
        assert!(persist
            .list_since(&action_result_topic(&request.request_id), None)
            .unwrap()
            .is_empty());

        worker.drain_actions(&mut persist).unwrap();

        assert_eq!(worker.action_execution_count, 1);
        assert_eq!(worker.action_cursor.as_deref(), Some(message.ulid.as_str()));
        let results = persist
            .list_since(&action_result_topic(&request.request_id), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        let result: HealthActionResult =
            serde_json::from_str(results[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(result.outcome, HealthActionOutcome::Failed);
        assert!(result.detail.contains("was not repeated"));
        assert!(!journal_path.exists());
    }

    #[test]
    fn result_publication_failure_replays_durable_result_without_repeating_mutation() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());
        let snapshot = worker.cycle(Some(&mut persist)).unwrap();
        let request = acknowledge_request(&snapshot, "publication-failure-request");
        let message = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).unwrap()),
            )
            .unwrap();
        let blocked_topic = bus.path().join(action_result_topic(&request.request_id));
        std::fs::create_dir_all(blocked_topic.parent().unwrap()).unwrap();
        std::fs::write(&blocked_topic, b"hostile regular file").unwrap();
        let journal_path = action_journal_path(&worker.action_state_root, &message.ulid);
        let trusted_uid = worker.trusted_action_owner_uid();

        assert!(worker.drain_actions(&mut persist).is_err());

        assert_eq!(worker.action_execution_count, 1);
        assert_eq!(worker.action_cursor, None);
        assert!(matches!(
            read_action_journal(&worker.action_state_root, &journal_path, trusted_uid),
            Ok(Some(DurableHealthAction::Complete { .. }))
        ));
        assert!(persist
            .list_since(&action_result_topic(&request.request_id), None)
            .unwrap()
            .is_empty());

        std::fs::remove_file(&blocked_topic).unwrap();
        worker.drain_actions(&mut persist).unwrap();

        assert_eq!(worker.action_execution_count, 1);
        let results = persist
            .list_since(&action_result_topic(&request.request_id), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        let result: HealthActionResult =
            serde_json::from_str(results[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(result.outcome, HealthActionOutcome::Applied);
        assert!(!journal_path.exists());
    }

    #[test]
    fn conflicting_same_audit_result_cannot_acknowledge_restart_replay() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut worker = action_test_worker(workgroup.path(), bus.path());
        let snapshot = worker.cycle(Some(&mut persist)).unwrap();
        let request = acknowledge_request(&snapshot, "conflicting-replay-result");
        let message = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).unwrap()),
            )
            .unwrap();
        let result = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            condition_id: request.condition_id.clone(),
            action: request.action,
            outcome: HealthActionOutcome::Applied,
            detail: "genuine durable action result".into(),
            audit_id: format!("health:node:{}", message.ulid),
            completed_at_ms: now_ms(),
            snapshot_generation: snapshot.generation,
            refreshed_evidence: None,
        };
        let record = DurableHealthAction::Complete {
            source_ulid: message.ulid.clone(),
            result: result.clone(),
        };
        worker.store_action_journal(&message.ulid, &record).unwrap();
        let conflicting = HealthActionResult {
            detail: "hostile conflicting result".into(),
            ..result.clone()
        };
        persist
            .write(
                &action_result_topic(&request.request_id),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&conflicting).unwrap()),
            )
            .unwrap();
        let journal_path = action_journal_path(&worker.action_state_root, &message.ulid);
        let trusted_uid = worker.trusted_action_owner_uid();
        drop(worker);

        let mut restarted = action_test_worker(workgroup.path(), bus.path());
        restarted.cycle(Some(&mut persist)).unwrap();
        let error = restarted.drain_actions(&mut persist).unwrap_err();

        assert!(error.contains("conflicting body for audit id"), "{error}");
        assert_eq!(restarted.action_execution_count, 0);
        assert_eq!(restarted.action_cursor, None);
        assert_eq!(
            read_action_journal(&restarted.action_state_root, &journal_path, trusted_uid).unwrap(),
            Some(record),
            "a conflicting Bus row must not discard the genuine durable result"
        );
        let rows = persist
            .list_since(&action_result_topic(&request.request_id), None)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            serde_json::from_str::<HealthActionResult>(rows[0].body.as_deref().unwrap()).unwrap(),
            conflicting
        );
    }

    #[test]
    fn invalid_action_result_cannot_publish_or_satisfy_replay() {
        let workgroup = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let worker = action_test_worker(workgroup.path(), bus.path());
        let request_id = "invalid-result";
        let valid = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request_id.into(),
            condition_id: "node:required-service-workbench".into(),
            action: HealthAction::Acknowledge,
            outcome: HealthActionOutcome::Applied,
            detail: "acknowledgement completed".into(),
            audit_id: "health:node:01J00000000000000000000001".into(),
            completed_at_ms: now_ms(),
            snapshot_generation: 1,
            refreshed_evidence: None,
        };
        let staged = StagedActionResult {
            source_ulid: "01J00000000000000000000001".into(),
            result: HealthActionResult {
                detail: "x".repeat(mackes_mesh_types::health::MAX_HEALTH_TEXT_BYTES + 1),
                ..valid.clone()
            },
            complete: None,
            already_published: false,
        };

        assert!(worker.publish_action_result(&mut persist, &staged).is_err());
        let topic = action_result_topic(request_id);
        assert!(persist.list_since(&topic, None).unwrap().is_empty());

        let mut unknown = serde_json::to_value(&valid).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("untrusted_extension".into(), serde_json::json!(true));
        persist
            .write(
                &topic,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&unknown).unwrap()),
            )
            .unwrap();
        assert!(NodeGradeWorker::action_result_published(&mut persist, &valid).is_err());
    }

    #[test]
    fn local_action_journal_rejects_symlink_and_unsafe_owner_or_mode() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let parent = tempfile::tempdir().unwrap();
        let state_root = parent.path().join("local-action-state");
        let trusted_uid = rustix::process::geteuid().as_raw();
        let source_ulid = "01K23F4Q6X8A0B2C4D6E8F0G2H";
        let record = DurableHealthAction::Claimed {
            source_ulid: source_ulid.into(),
            request: request(HealthAction::Acknowledge),
            snapshot_generation: 7,
            claimed_at_ms: 100,
        };
        write_action_journal(&state_root, source_ulid, &record, trusted_uid).unwrap();
        let path = action_journal_path(&state_root, source_ulid);

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o660);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(read_action_journal(&state_root, &path, trusted_uid).is_err());

        std::fs::remove_file(&path).unwrap();
        let victim = parent.path().join("victim.json");
        std::fs::write(&victim, serde_json::to_vec(&record).unwrap()).unwrap();
        symlink(&victim, &path).unwrap();
        assert!(read_action_journal(&state_root, &path, trusted_uid).is_err());

        assert!(pending_action_journals(&state_root, trusted_uid.saturating_add(1)).is_err());
        let linked_root = parent.path().join("linked-action-state");
        symlink(&state_root, &linked_root).unwrap();
        assert!(pending_action_journals(&linked_root, trusted_uid).is_err());
    }

    #[test]
    fn local_action_journal_enforces_cap_and_cleans_bounded_safe_temp() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let state_root = parent.path().join("capped-action-state");
        let trusted_uid = rustix::process::geteuid().as_raw();
        ensure_action_state_root(&state_root, trusted_uid).unwrap();
        let record = DurableHealthAction::Claimed {
            source_ulid: format!("{:026}", 0),
            request: request(HealthAction::Acknowledge),
            snapshot_generation: 7,
            claimed_at_ms: 100,
        };
        let body = serde_json::to_vec(&record).unwrap();
        for index in 0..MAX_PENDING_ACTION_RESULTS {
            let source_ulid = format!("{index:026}");
            let path = action_journal_path(&state_root, &source_ulid);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(&body).unwrap();
        }
        assert_eq!(
            pending_action_journals(&state_root, trusted_uid)
                .unwrap()
                .len(),
            MAX_PENDING_ACTION_RESULTS
        );
        let overflow_ulid = format!("{:026}", MAX_PENDING_ACTION_RESULTS);
        let overflow = DurableHealthAction::Claimed {
            source_ulid: overflow_ulid.clone(),
            request: request(HealthAction::Acknowledge),
            snapshot_generation: 7,
            claimed_at_ms: 100,
        };
        assert!(write_action_journal(&state_root, &overflow_ulid, &overflow, trusted_uid).is_err());
        assert!(!action_journal_path(&state_root, &overflow_ulid).exists());

        let temp_root = parent.path().join("temp-action-state");
        ensure_action_state_root(&temp_root, trusted_uid).unwrap();
        let temp_path = temp_root.join(format!(".{:026}.tmp", 1));
        let mut temp = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .unwrap();
        temp.write_all(&body).unwrap();
        drop(temp);
        assert!(pending_action_journals(&temp_root, trusted_uid)
            .unwrap()
            .is_empty());
        assert!(!temp_path.exists());
    }
}
