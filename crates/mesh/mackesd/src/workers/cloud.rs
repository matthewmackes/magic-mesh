//! WL-ARCH-001 Phase B — the mackesd `cloud` worker: the **OpenTofu + Ansible
//! cloud backend** that succeeds the deleted OpenStack worker tree.
//!
//! The operator directive (2026-07-19) removed ALL OpenStack and rebuilt cloud
//! operations on **OpenTofu (provision) + Ansible (configure)** against local
//! libvirt/KVM. This worker is the mesh-side runner + status publisher for that
//! stack — the successor to `workers/openstack/`. It:
//!
//! 1. **Drains `action/cloud/*` verbs off the Bus** ([`CLOUD_ACTION_PREFIX`]) and
//!    answers each with a neutral [`CloudReply`] on `reply/<ulid>` — the same §6
//!    contract the KDC-MESH-8 phone command surface (`kdc_host::cloud`) already
//!    speaks. Verbs: `list`/`list-instances` + `status` (READS), `provision` /
//!    `configure` / `destroy` and `instance-{start,stop,reboot,delete}`
//!    (MUTATIONS).
//! 2. **Shells OpenTofu + Ansible** through the injectable [`CloudRunner`] seam
//!    (production: [`ShellCloudRunner`] — `tofu -chdir=infra/tofu/cloud plan/apply`,
//!    `ansible-playbook`, `virsh`; tests inject a fake so CI never touches a live
//!    hypervisor).
//! 3. **Publishes `state/cloud/<node>`** ([`cloud_state_topic`]) — a
//!    [`CloudState`] carrying per-tool backend health ([`ServiceHealth`]) + the
//!    resource roster ([`ResourceTable`]), built ENTIRELY from the
//!    `mackes_mesh_types::cloud` neutral types so the recreated IaC surface
//!    (Phase C) renders it without depending on `mackesd`.
//!
//! **Live mutation is operator-gated** (RUN-006's staged idiom): the worker only
//! performs a real `apply` / `ansible-playbook` / `virsh` op when
//! `MDE_CLOUD_APPLY=1` ([`APPLY_ENV`]) is set — otherwise every mutation is
//! *staged* (a `tofu plan` / `--check` dry-run: honestly not applied, §7). The
//! action drain is **leader-gated** (the shared `.mackesd-leader.lock` every
//! leader-gated worker contends on) so a flat `action/cloud/*` topic on an N-node
//! mesh executes + replies exactly once; the `state/cloud/<node>` mirror is
//! per-node universal (every node publishes its OWN local roster, no center).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::cloud::{
    cloud_state_topic, CloudInstance, CloudProviderAdapter, CloudReply, CloudState,
    EndpointInterface, HealthState, LifecycleAction, ResourceRow, ResourceTable, ServiceHealth,
    CLOUD_ACTION_PREFIX,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

/// Action-drain cadence — a verb lands within ~3 s (as `router_action` / `container`).
pub const POLL: Duration = Duration::from_secs(3);

/// Unconditional `state/cloud/<node>` republish cadence (between change publishes).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// The env flag that arms LIVE cloud mutation (`tofu apply` / `ansible-playbook` /
/// `virsh` ops). Absent / `!= "1"` ⇒ every mutation is *staged* (plan / `--check`
/// dry-run, honestly not applied — the parked, operator-gated live seam, like
/// `MDE_ROUTER_ACTION_LIVE`).
pub const APPLY_ENV: &str = "MDE_CLOUD_APPLY";

/// The env override for the IaC tree root (holds `infra/tofu/cloud` + the Ansible
/// tree). Defaults to [`DEFAULT_IAC_ROOT`] when unset.
pub const IAC_ROOT_ENV: &str = "MDE_IAC_ROOT";

/// The env override for the libvirt connection URI the runner drives.
pub const LIBVIRT_URI_ENV: &str = "MDE_LIBVIRT_URI";

/// Default IaC tree root when [`IAC_ROOT_ENV`] is unset (a deployed node ships the
/// tree here; a dev checkout sets `MDE_IAC_ROOT` to the repo root).
pub const DEFAULT_IAC_ROOT: &str = "/usr/share/mde/iac";

/// Default libvirt connection URI (local system KVM — E12 local-first).
pub const DEFAULT_LIBVIRT_URI: &str = "qemu:///system";

/// The OpenTofu root (provision), relative to the IaC root.
pub const TOFU_SUBDIR: &str = "infra/tofu/cloud";

/// The Ansible tree (configure), relative to the IaC root.
pub const ANSIBLE_SUBDIR: &str = "automation/ansible";

// ── backend tool identifiers (the `service_type` on each health row) ──
/// OpenTofu (provision leg).
pub const TOOL_TOFU: &str = "opentofu";
/// Ansible (configure leg).
pub const TOOL_ANSIBLE: &str = "ansible";
/// libvirt/KVM (the local VM backend).
pub const TOOL_LIBVIRT: &str = "libvirt";

/// Every backend tool the state mirror reports health for, in render order.
pub const BACKEND_TOOLS: [&str; 3] = [TOOL_TOFU, TOOL_ANSIBLE, TOOL_LIBVIRT];

// ─────────────────────────── verbs + the apply gate ───────────────────────────

/// A drained `action/cloud/<verb>` classified for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudVerb {
    /// `list` / `list-instances` — the instance roster (READ).
    List,
    /// `status` — the roster + health summary (READ).
    Status,
    /// `provision` — `tofu plan/apply` in `infra/tofu/cloud` (MUTATION).
    Provision,
    /// `configure` — `ansible-playbook` over the mesh inventory (MUTATION).
    Configure,
    /// `destroy` — `tofu plan -destroy` / `destroy` (MUTATION).
    Destroy,
    /// `instance-{start,stop,reboot,delete}` — a `virsh` domain op (MUTATION).
    Lifecycle(LifecycleAction),
}

impl CloudVerb {
    /// Classify a verb token, or `None` for an unrecognized verb (never guessed).
    #[must_use]
    pub fn from_verb(verb: &str) -> Option<Self> {
        if let Some(action) = LifecycleAction::from_verb(verb) {
            return Some(Self::Lifecycle(action));
        }
        match verb {
            "list" | "list-instances" => Some(Self::List),
            "status" => Some(Self::Status),
            "provision" => Some(Self::Provision),
            "configure" => Some(Self::Configure),
            "destroy" => Some(Self::Destroy),
            _ => None,
        }
    }

    /// Whether this verb mutates the backend (so it rides the [`APPLY_ENV`] gate).
    /// Reads (`list`/`status`) are always served.
    #[must_use]
    pub const fn is_mutation(self) -> bool {
        matches!(
            self,
            Self::Provision | Self::Configure | Self::Destroy | Self::Lifecycle(_)
        )
    }

    /// Whether performing this verb is destructive (`destroy` / a destructive
    /// lifecycle op) — the ops audited on the events plane when performed (§7).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        match self {
            Self::Destroy => true,
            Self::Lifecycle(a) => a.is_destructive(),
            _ => false,
        }
    }
}

/// The pre-run decision for a verb given the operator gate — the pure gate tested
/// without a runner (mirrors `router_action::pre_apply_decision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDecision {
    /// A read verb — always served.
    Read,
    /// A mutation and the live gate is armed — perform the real op.
    Apply,
    /// A mutation and the live gate is disarmed — stage it (plan / `--check`).
    Staged,
}

/// The pure apply-gate decision: reads serve; mutations apply iff `apply_armed`,
/// else stage. No I/O — the gate is tested without a hypervisor.
#[must_use]
pub const fn decide(verb: CloudVerb, apply_armed: bool) -> CloudDecision {
    if !verb.is_mutation() {
        CloudDecision::Read
    } else if apply_armed {
        CloudDecision::Apply
    } else {
        CloudDecision::Staged
    }
}

/// The honest outcome of one runner op — its success, a short human summary, and
/// whether a live mutation was actually attempted (drives the reply's `audited`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRunOutcome {
    /// Whether the op succeeded.
    pub ok: bool,
    /// A short human summary (a `tofu`/`ansible` result line, or the failure).
    pub summary: String,
    /// Whether a live mutation was attempted (`apply == true`). A staged dry-run is
    /// `false` (nothing was applied).
    pub applied: bool,
}

impl CloudRunOutcome {
    /// A successful outcome.
    #[must_use]
    pub fn ok(summary: impl Into<String>, applied: bool) -> Self {
        Self {
            ok: true,
            summary: summary.into(),
            applied,
        }
    }

    /// A failed outcome (never marked applied — a failure durably changed nothing
    /// we can claim).
    #[must_use]
    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: summary.into(),
            applied: false,
        }
    }
}

// ─────────────────────────── the runner seam ───────────────────────────

/// The backend-execution seam the worker drives: shell OpenTofu / Ansible / virsh.
///
/// Production is [`ShellCloudRunner`]; tests inject a fake so the drain, gate,
/// reply, audit, and state-publish paths are exercised WITHOUT a live hypervisor
/// (the `router_action` applier-injection idiom).
pub trait CloudRunner: Send + Sync {
    /// Honestly probe whether backend tool `tool` ([`BACKEND_TOOLS`]) is present +
    /// reachable, as a [`ServiceHealth`] row (`Up` / `Down` / `Absent`, never a
    /// fabricated up).
    fn probe_tool(&self, tool: &str) -> ServiceHealth;

    /// List the local cloud instances (a READ — `virsh list`). `Err` when the
    /// backend can't be reached (an honest gate, never a fabricated empty roster).
    fn list_instances(&self) -> Result<Vec<CloudInstance>, String>;

    /// Provision via OpenTofu. `apply` ⇒ `tofu apply`; else `tofu plan` (staged).
    fn provision(&self, apply: bool) -> CloudRunOutcome;

    /// Configure via Ansible. `apply` ⇒ `ansible-playbook`; else `--check` (staged).
    fn configure(&self, apply: bool) -> CloudRunOutcome;

    /// Destroy via OpenTofu. `apply` ⇒ `tofu destroy`; else `tofu plan -destroy`.
    fn destroy(&self, apply: bool) -> CloudRunOutcome;

    /// A per-instance lifecycle op via `virsh`. `apply` ⇒ the real op; else staged.
    fn lifecycle(&self, action: LifecycleAction, instance: &str, apply: bool) -> CloudRunOutcome;
}

/// Normalize a libvirt domain state to the OpenStack-style status token the
/// neutral [`CloudInstance`] carries (the roster the KDC client filters on
/// `ACTIVE`/`SHUTOFF`). Unknown states pass through upper-cased (honest).
#[must_use]
pub fn normalize_domain_state(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "running" => "ACTIVE".to_string(),
        "shut off" | "shutoff" => "SHUTOFF".to_string(),
        "paused" => "PAUSED".to_string(),
        "" => "UNKNOWN".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Build the `compute`/`instances` [`ResourceTable`] for a roster (name · status).
#[must_use]
pub fn instances_table(instances: &[CloudInstance]) -> ResourceTable {
    ResourceTable {
        service_type: "compute".to_string(),
        collection: "instances".to_string(),
        columns: vec!["name".to_string(), "status".to_string()],
        rows: instances
            .iter()
            .map(|i| ResourceRow {
                id: i.id.clone(),
                cells: vec![i.name.clone(), i.status.clone()],
            })
            .collect(),
    }
}

/// The production runner: shells `tofu` / `ansible-playbook` / `virsh` rooted at
/// the deployed IaC tree + the local libvirt URI.
pub struct ShellCloudRunner {
    /// The OpenTofu root (`<iac_root>/infra/tofu/cloud`).
    tofu_dir: PathBuf,
    /// The Ansible tree (`<iac_root>/automation/ansible`).
    ansible_dir: PathBuf,
    /// The libvirt connection URI (`qemu:///system` by default).
    libvirt_uri: String,
}

impl ShellCloudRunner {
    /// Construct from an IaC tree root + a libvirt URI.
    #[must_use]
    pub fn new(iac_root: &Path, libvirt_uri: String) -> Self {
        Self {
            tofu_dir: iac_root.join(TOFU_SUBDIR),
            ansible_dir: iac_root.join(ANSIBLE_SUBDIR),
            libvirt_uri,
        }
    }

    /// Run `bin args…`, returning `(success, stdout, stderr)`. `Err` only on a
    /// spawn failure (binary absent) so the caller can report an honest "tool
    /// unavailable" rather than a fake failure.
    fn run(bin: &str, args: &[&str]) -> Result<(bool, String, String), String> {
        let out = Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| format!("{bin}: {e}"))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// Map a shell result into a [`CloudRunOutcome`] for `action` (`applied` is the
    /// requested apply flag; a failed apply is reported but not marked applied).
    fn outcome(
        res: Result<(bool, String, String), String>,
        applied: bool,
        action: &str,
    ) -> CloudRunOutcome {
        match res {
            Ok((true, out, _)) => {
                CloudRunOutcome::ok(format!("{action}: {}", summary_line(&out)), applied)
            }
            Ok((false, _, err)) => {
                CloudRunOutcome::failed(format!("{action} failed: {}", summary_line(&err)))
            }
            Err(e) => CloudRunOutcome::failed(format!("{action} unavailable: {e}")),
        }
    }

    fn tofu_chdir(&self) -> String {
        format!("-chdir={}", self.tofu_dir.display())
    }
}

impl CloudRunner for ShellCloudRunner {
    fn probe_tool(&self, tool: &str) -> ServiceHealth {
        let start = Instant::now();
        let uri = self.libvirt_uri.clone();
        let (bin, args, url): (&str, Vec<&str>, &str) = match tool {
            TOOL_TOFU => ("tofu", vec!["version"], "(local)"),
            TOOL_ANSIBLE => ("ansible-playbook", vec!["--version"], "(local)"),
            TOOL_LIBVIRT => ("virsh", vec!["-c", uri.as_str(), "version"], uri.as_str()),
            _ => {
                return ServiceHealth {
                    service_type: tool.to_string(),
                    interface: EndpointInterface::Internal,
                    url: String::new(),
                    state: HealthState::Absent,
                    latency_ms: None,
                    microversion: None,
                    version_id: None,
                    detail: Some("unknown backend tool".to_string()),
                }
            }
        };
        match Self::run(bin, &args) {
            Ok((true, out, _)) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: url.to_string(),
                state: HealthState::Up,
                latency_ms: Some(elapsed_ms(start)),
                microversion: None,
                version_id: None,
                detail: Some(summary_line(&out)),
            },
            // Present but errored (e.g. libvirtd unreachable) ⇒ Down, not faked up.
            Ok((false, _, err)) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: url.to_string(),
                state: HealthState::Down,
                latency_ms: Some(elapsed_ms(start)),
                microversion: None,
                version_id: None,
                detail: Some(summary_line(&err)),
            },
            // Binary absent ⇒ honestly Absent (nothing to reach).
            Err(e) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: String::new(),
                state: HealthState::Absent,
                latency_ms: None,
                microversion: None,
                version_id: None,
                detail: Some(e),
            },
        }
    }

    fn list_instances(&self) -> Result<Vec<CloudInstance>, String> {
        let (ok, out, err) = Self::run(
            "virsh",
            &["-c", &self.libvirt_uri, "list", "--all", "--name"],
        )
        .map_err(|e| format!("libvirt unavailable: {e}"))?;
        if !ok {
            return Err(format!("virsh list failed: {}", summary_line(&err)));
        }
        let mut instances = Vec::new();
        for name in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let status = match Self::run("virsh", &["-c", &self.libvirt_uri, "domstate", name]) {
                Ok((true, sout, _)) => normalize_domain_state(&sout),
                _ => "UNKNOWN".to_string(),
            };
            instances.push(CloudInstance {
                id: name.to_string(),
                name: name.to_string(),
                status,
                flavor: None,
                image: None,
                networks: None,
            });
        }
        Ok(instances)
    }

    fn provision(&self, apply: bool) -> CloudRunOutcome {
        let chdir = self.tofu_chdir();
        let args: Vec<&str> = if apply {
            vec![
                &chdir,
                "apply",
                "-auto-approve",
                "-input=false",
                "-no-color",
            ]
        } else {
            vec![&chdir, "plan", "-input=false", "-no-color"]
        };
        Self::outcome(
            Self::run("tofu", &args),
            apply,
            if apply {
                "tofu apply"
            } else {
                "tofu plan (staged)"
            },
        )
    }

    fn destroy(&self, apply: bool) -> CloudRunOutcome {
        let chdir = self.tofu_chdir();
        let args: Vec<&str> = if apply {
            vec![
                &chdir,
                "destroy",
                "-auto-approve",
                "-input=false",
                "-no-color",
            ]
        } else {
            vec![&chdir, "plan", "-destroy", "-input=false", "-no-color"]
        };
        Self::outcome(
            Self::run("tofu", &args),
            apply,
            if apply {
                "tofu destroy"
            } else {
                "tofu plan -destroy (staged)"
            },
        )
    }

    fn configure(&self, apply: bool) -> CloudRunOutcome {
        let playbook = self.ansible_dir.join("playbooks").join("site.yml");
        let playbook_str = playbook.display().to_string();
        let inventory = self.ansible_dir.join("inventory").join("mesh.py");
        let inventory_str = inventory.display().to_string();
        let mut args: Vec<&str> = vec!["-i", &inventory_str, &playbook_str];
        if !apply {
            args.push("--check");
        }
        Self::outcome(
            Self::run("ansible-playbook", &args),
            apply,
            if apply {
                "ansible-playbook"
            } else {
                "ansible-playbook --check (staged)"
            },
        )
    }

    fn lifecycle(&self, action: LifecycleAction, instance: &str, apply: bool) -> CloudRunOutcome {
        if !apply {
            return CloudRunOutcome::ok(
                format!("virsh {} {instance} (staged)", action.cli_verb()),
                false,
            );
        }
        // Map the neutral lifecycle action onto the virsh subcommand.
        let subcmd = match action {
            LifecycleAction::Start => "start",
            LifecycleAction::Stop => "shutdown",
            LifecycleAction::Reboot => "reboot",
            LifecycleAction::Delete => "undefine",
        };
        // A Delete first force-stops the domain (best-effort) so the undefine
        // succeeds on a running persistent domain.
        if matches!(action, LifecycleAction::Delete) {
            let _ = Self::run("virsh", &["-c", &self.libvirt_uri, "destroy", instance]);
        }
        Self::outcome(
            Self::run("virsh", &["-c", &self.libvirt_uri, subcmd, instance]),
            true,
            &format!("virsh {subcmd} {instance}"),
        )
    }
}

// ─────────────────────────── small helpers ───────────────────────────

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The first non-empty line of a command's output, trimmed + length-capped — the
/// human "why" carried in a health/reply detail (never a multi-KB dump).
fn summary_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    if line.len() > 200 {
        format!("{}…", &line[..200])
    } else {
        line
    }
}

/// The default IaC tree root: [`IAC_ROOT_ENV`] or [`DEFAULT_IAC_ROOT`].
fn default_iac_root() -> PathBuf {
    std::env::var_os(IAC_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_IAC_ROOT))
}

/// The default libvirt URI: [`LIBVIRT_URI_ENV`] or [`DEFAULT_LIBVIRT_URI`].
fn default_libvirt_uri() -> String {
    std::env::var(LIBVIRT_URI_ENV).unwrap_or_else(|_| DEFAULT_LIBVIRT_URI.to_string())
}

/// Whether the live-apply gate is armed (`MDE_CLOUD_APPLY=1`).
fn apply_armed_from_env() -> bool {
    std::env::var(APPLY_ENV).ok().as_deref() == Some("1")
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Parse the `instance` name from a lifecycle request body (`{"instance":"…"}`).
fn parse_instance(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|v| {
            v.get("instance")
                .and_then(|i| i.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.trim().is_empty())
}

// ─────────────────────────── the worker ───────────────────────────

/// The WL-ARCH-001 Phase B cloud worker (per-node, rank-0 universal; the action
/// drain is leader-gated).
pub struct CloudWorker {
    /// This node's id — the `state/cloud/<host>` namespace + the audit actor.
    host: String,
    /// The mesh node id (`peer:<host>`) — the leader-lease holder identity.
    node_id: String,
    /// The injectable backend seam (production: [`ShellCloudRunner`]).
    runner: Arc<dyn CloudRunner>,
    /// SAFETY — the live-mutation gate (`MDE_CLOUD_APPLY=1`); default staged.
    apply_armed: bool,
    /// The shared leader lock (`<workgroup_root>/.mackesd-leader.lock`) — only the
    /// elected node executes + replies (exactly-once on a flat topic).
    leader_lock: PathBuf,
    /// The hash-chain audit DB (destructive performed ops audit here).
    db_path: PathBuf,
    /// The Bus root the mirror publish targets + the action drain reads (`None` ⇒
    /// publish/drain is a no-op — a pre-RPM dev box with no bus).
    bus_root: Option<PathBuf>,
    /// Fold/publish cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// Test-only leadership override (bypasses the [`crate::leader_gate::LeaderGate`]
    /// so the drain path is exercised deterministically). `None` ⇒ the real gate.
    leader_override: Option<bool>,
}

impl CloudWorker {
    /// Construct with production defaults: the [`ShellCloudRunner`] over the
    /// deployed IaC tree ([`default_iac_root`]) + local libvirt, the operator gate
    /// read from [`APPLY_ENV`], the shared leader lock under `workgroup_root`, the
    /// canonical audit DB, and the persisted Bus tree. `host` is this node's id.
    #[must_use]
    pub fn new(host: String, node_id: String, workgroup_root: PathBuf) -> Self {
        let runner = Arc::new(ShellCloudRunner::new(
            &default_iac_root(),
            default_libvirt_uri(),
        ));
        Self {
            host,
            node_id,
            runner,
            apply_armed: apply_armed_from_env(),
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            db_path: crate::default_db_path(),
            bus_root: default_bus_root(),
            poll: POLL,
            heartbeat: PUBLISH_HEARTBEAT,
            leader_override: None,
        }
    }

    /// Inject a backend runner (tests supply a fake).
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn CloudRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Arm/disarm the live-apply gate (tests + the operator gate).
    #[must_use]
    pub const fn with_apply(mut self, armed: bool) -> Self {
        self.apply_armed = armed;
        self
    }

    /// Override the audit DB path (tests point it at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the Bus root (tests point it at a tempdir; `None` disables it).
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override the fold cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Force leadership on/off (tests) — bypasses the shared election so the drain
    /// path is deterministic without etcd / an fs lock race.
    #[must_use]
    pub const fn with_leader_override(mut self, leader: bool) -> Self {
        self.leader_override = Some(leader);
        self
    }

    /// Whether this node currently holds the shared leadership (only the leader
    /// executes + replies to a flat `action/cloud/*` verb). A test override wins.
    fn is_leader(&self) -> bool {
        if let Some(forced) = self.leader_override {
            return forced;
        }
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    /// Write one hash-chain audit row for a performed destructive cloud op through
    /// the EXISTING events plane (best-effort — a store fault is logged, never
    /// fatal). Makes the reply's `audited: true` truthful.
    fn audit(&self, verb: &str, instance: Option<&str>, outcome: &CloudRunOutcome) {
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            serde_json::json!({
                "action": "cloud",
                "verb": verb,
                "instance": instance,
                "ok": outcome.ok,
                "applied": outcome.applied,
                "summary": outcome.summary,
            }),
        );
    }

    /// Handle one `action/cloud/<verb>` request end to end → a typed [`CloudReply`].
    /// Reads serve the roster; mutations gate on [`APPLY_ENV`] (staged unless armed)
    /// and audit when destructive + performed. Never panics.
    pub fn handle(&self, verb_name: &str, body: &str) -> CloudReply {
        let base = |ok: bool| CloudReply {
            ok,
            verb: verb_name.to_string(),
            ..Default::default()
        };
        let Some(verb) = CloudVerb::from_verb(verb_name) else {
            return CloudReply {
                error: Some(format!("unknown cloud verb `{verb_name}`")),
                ..base(false)
            };
        };

        // ── READ verbs (list / status) — always served ──
        if matches!(verb, CloudVerb::List | CloudVerb::Status) {
            return match self.runner.list_instances() {
                Ok(instances) => CloudReply {
                    instances: Some(instances),
                    ..base(true)
                },
                Err(e) => CloudReply {
                    gated: Some(format!("cloud backend not ready: {e}")),
                    ..base(false)
                },
            };
        }

        // ── MUTATION verbs — resolve a target (lifecycle) + the apply gate ──
        let instance = if matches!(verb, CloudVerb::Lifecycle(_)) {
            match parse_instance(body) {
                Some(i) => Some(i),
                None => {
                    return CloudReply {
                        error: Some(format!(
                            "`{verb_name}` requires an `instance` field in the request body"
                        )),
                        ..base(false)
                    }
                }
            }
        } else {
            None
        };

        let decision = decide(verb, self.apply_armed);
        let apply = matches!(decision, CloudDecision::Apply);
        let outcome = match verb {
            CloudVerb::Provision => self.runner.provision(apply),
            CloudVerb::Configure => self.runner.configure(apply),
            CloudVerb::Destroy => self.runner.destroy(apply),
            CloudVerb::Lifecycle(action) => {
                self.runner
                    .lifecycle(action, instance.as_deref().unwrap_or_default(), apply)
            }
            CloudVerb::List | CloudVerb::Status => unreachable!("reads handled above"),
        };

        match decision {
            CloudDecision::Staged => CloudReply {
                gated: Some(format!(
                    "live apply is operator-gated (set {APPLY_ENV}=1) — {} — nothing applied",
                    outcome.summary
                )),
                ..base(false)
            },
            CloudDecision::Apply => {
                // A destructive op performed live is audited (durable row) so the
                // reply's `audited: true` is truthful.
                let audited = verb.is_destructive();
                if audited {
                    self.audit(verb_name, instance.as_deref(), &outcome);
                }
                if outcome.ok {
                    CloudReply {
                        audited,
                        ..base(true)
                    }
                } else {
                    CloudReply {
                        error: Some(outcome.summary),
                        audited,
                        ..base(false)
                    }
                }
            }
            CloudDecision::Read => unreachable!("reads returned above"),
        }
    }

    /// Drain every new `action/cloud/*` request, advance the per-topic cursors, and
    /// — only on the elected leader — handle + reply to each. A non-leader advances
    /// cursors and replies to nothing (the elected node acts), so a flat topic on an
    /// N-node mesh executes exactly once. Returns `true` when any request was handled
    /// (so the caller force-republishes the fresh roster).
    fn drain_actions(&self, cursors: &mut HashMap<String, String>) -> bool {
        let Some(root) = self.bus_root.clone() else {
            return false;
        };
        let Ok(persist) = Persist::open(root) else {
            return false;
        };
        let Ok(topics) = persist.list_topics() else {
            return false;
        };
        let leader = self.is_leader();
        let mut acted = false;
        for topic in topics {
            let Some(verb) = topic.strip_prefix(CLOUD_ACTION_PREFIX) else {
                continue;
            };
            let verb = verb.to_string();
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                if !leader {
                    continue;
                }
                let body = msg.body.as_deref().unwrap_or("{}");
                let reply = self.handle(&verb, body);
                tracing::info!(
                    target: "mackesd::cloud",
                    ulid = %msg.ulid, verb = %verb, ok = reply.ok,
                    audited = reply.audited, "cloud action handled (WL-ARCH-001 Phase B)"
                );
                let body = serde_json::to_string(&reply).unwrap_or_default();
                if let Err(e) = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&body),
                ) {
                    tracing::warn!(target: "mackesd::cloud", ulid = %msg.ulid, error = %e, "cloud reply write failed");
                }
                acted = true;
            }
        }
        acted
    }

    /// Seed each existing `action/cloud/*` topic's cursor to its newest message so
    /// a (re)start doesn't replay a backlog of verbs (a queued `provision` must not
    /// re-fire on the next daemon restart — the `container::prime_cursor` idiom).
    fn prime_cursors(&self, cursors: &mut HashMap<String, String>) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(topics) = persist.list_topics() else {
            return;
        };
        for topic in topics {
            if !topic.starts_with(CLOUD_ACTION_PREFIX) {
                continue;
            }
            if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                cursors.insert(topic, ulid);
            }
        }
    }

    /// Build the current `state/cloud/<node>` mirror: probe each backend tool's
    /// health + fold the live roster into a resource table (all neutral types).
    pub fn build_state(&self) -> CloudState {
        let health: Vec<ServiceHealth> = BACKEND_TOOLS
            .iter()
            .map(|t| self.runner.probe_tool(t))
            .collect();
        let resources = match self.runner.list_instances() {
            Ok(instances) => vec![instances_table(&instances)],
            // An unreachable backend ⇒ no resource table (the Down/Absent health
            // rows already carry the honest reason); never a fabricated roster.
            Err(_) => Vec::new(),
        };
        CloudState {
            host: self.host.clone(),
            adapter: CloudProviderAdapter::ConstructCloud,
            health,
            resources,
            apply_armed: self.apply_armed,
            published_at_ms: now_ms(),
        }
    }

    /// Publish the current mirror to `state/cloud/<host>` (best-effort, in-process
    /// through the shared bus root every other mackesd mirror uses).
    fn publish_state(&self) {
        let state = self.build_state();
        if let Some(mut persist) =
            crate::bus_publish::open_bus(self.bus_root.as_ref().map(PathBuf::clone))
        {
            crate::bus_publish::publish_json(&mut persist, &cloud_state_topic(&self.host), &state);
        }
    }
}

#[async_trait::async_trait]
impl Worker for CloudWorker {
    fn name(&self) -> &'static str {
        "cloud"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut cursors: HashMap<String, String> = HashMap::new();
        // Don't replay a backlog of verbs across a restart.
        self.prime_cursors(&mut cursors);
        // Publish an initial mirror so a surface doesn't wait a full tick.
        self.publish_state();
        let mut last_pub = Instant::now();
        loop {
            let acted = self.drain_actions(&mut cursors);
            // Republish on a handled action (fresh roster) or the heartbeat.
            if acted || last_pub.elapsed() >= self.heartbeat {
                self.publish_state();
                last_pub = Instant::now();
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted fake runner: records the `apply` flag each mutation was called
    /// with, returns canned outcomes, and serves a fixed roster.
    #[derive(Default)]
    struct FakeRunner {
        roster: Vec<CloudInstance>,
        roster_err: Option<String>,
        tofu_up: bool,
        /// (verb, apply) calls the worker made — proves the gate.
        calls: Mutex<Vec<(String, bool)>>,
    }

    impl FakeRunner {
        fn record(&self, verb: &str, apply: bool) {
            self.calls.lock().unwrap().push((verb.to_string(), apply));
        }
    }

    impl CloudRunner for FakeRunner {
        fn probe_tool(&self, tool: &str) -> ServiceHealth {
            let state = if tool == TOOL_TOFU && self.tofu_up {
                HealthState::Up
            } else {
                HealthState::Absent
            };
            ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: "(local)".to_string(),
                state,
                latency_ms: Some(1),
                microversion: None,
                version_id: None,
                detail: Some("fake".to_string()),
            }
        }
        fn list_instances(&self) -> Result<Vec<CloudInstance>, String> {
            match &self.roster_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.roster.clone()),
            }
        }
        fn provision(&self, apply: bool) -> CloudRunOutcome {
            self.record("provision", apply);
            CloudRunOutcome::ok("2 to add, 0 to change", apply)
        }
        fn configure(&self, apply: bool) -> CloudRunOutcome {
            self.record("configure", apply);
            CloudRunOutcome::ok("ok=3 changed=1", apply)
        }
        fn destroy(&self, apply: bool) -> CloudRunOutcome {
            self.record("destroy", apply);
            CloudRunOutcome::ok("1 to destroy", apply)
        }
        fn lifecycle(
            &self,
            action: LifecycleAction,
            instance: &str,
            apply: bool,
        ) -> CloudRunOutcome {
            self.record(&format!("lifecycle-{}", action.cli_verb()), apply);
            CloudRunOutcome::ok(format!("virsh {} {instance}", action.cli_verb()), apply)
        }
    }

    fn instance(name: &str, status: &str) -> CloudInstance {
        CloudInstance {
            id: name.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            flavor: None,
            image: None,
            networks: None,
        }
    }

    fn worker_with(runner: Arc<dyn CloudRunner>, apply_armed: bool) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), PathBuf::from("/tmp"))
            .with_runner(runner)
            .with_apply(apply_armed)
            .with_bus_root(None)
            .with_leader_override(true)
    }

    // ── the pure verb classifier + apply gate ──
    #[test]
    fn verbs_classify_reads_mutations_and_lifecycle() {
        assert_eq!(CloudVerb::from_verb("list"), Some(CloudVerb::List));
        assert_eq!(
            CloudVerb::from_verb("list-instances"),
            Some(CloudVerb::List)
        );
        assert_eq!(CloudVerb::from_verb("status"), Some(CloudVerb::Status));
        assert_eq!(
            CloudVerb::from_verb("provision"),
            Some(CloudVerb::Provision)
        );
        assert_eq!(
            CloudVerb::from_verb("instance-reboot"),
            Some(CloudVerb::Lifecycle(LifecycleAction::Reboot))
        );
        assert_eq!(CloudVerb::from_verb("frobnicate"), None);
        assert!(!CloudVerb::List.is_mutation());
        assert!(CloudVerb::Provision.is_mutation());
        assert!(CloudVerb::Destroy.is_destructive());
        assert!(!CloudVerb::Provision.is_destructive());
        assert!(CloudVerb::Lifecycle(LifecycleAction::Delete).is_destructive());
        assert!(!CloudVerb::Lifecycle(LifecycleAction::Start).is_destructive());
    }

    #[test]
    fn decide_serves_reads_and_gates_mutations() {
        assert_eq!(decide(CloudVerb::List, false), CloudDecision::Read);
        assert_eq!(decide(CloudVerb::Status, true), CloudDecision::Read);
        assert_eq!(decide(CloudVerb::Provision, true), CloudDecision::Apply);
        assert_eq!(decide(CloudVerb::Provision, false), CloudDecision::Staged);
        assert_eq!(
            decide(CloudVerb::Lifecycle(LifecycleAction::Stop), false),
            CloudDecision::Staged
        );
    }

    // ── list / status reads ──
    #[test]
    fn list_returns_the_roster_and_matches_the_kdc_contract() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE"), instance("db", "SHUTOFF")],
            ..Default::default()
        });
        let w = worker_with(runner, false);
        // Both the task `list` verb and the KDC `list-instances` verb answer.
        for verb in ["list", "list-instances", "status"] {
            let reply = w.handle(verb, "{}");
            assert!(reply.ok, "{verb} ok");
            let instances = reply.instances.expect("roster");
            assert_eq!(instances.len(), 2);
            assert_eq!(instances[0].name, "web");
        }
    }

    #[test]
    fn a_read_against_an_unreachable_backend_is_gated_not_faked() {
        let runner = Arc::new(FakeRunner {
            roster_err: Some("libvirt unavailable".into()),
            ..Default::default()
        });
        let w = worker_with(runner, false);
        let reply = w.handle("list", "{}");
        assert!(!reply.ok);
        assert!(reply.instances.is_none(), "no fabricated empty roster");
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    // ── the apply gate: staged vs armed ──
    #[test]
    fn a_staged_mutation_runs_a_dry_run_and_applies_nothing() {
        let runner = Arc::new(FakeRunner::default());
        let w = worker_with(runner.clone(), false); // apply gate OFF
        let reply = w.handle("provision", "{}");
        assert!(!reply.ok, "a staged mutation is not a fabricated success");
        assert!(reply
            .gated
            .as_deref()
            .unwrap()
            .contains("MDE_CLOUD_APPLY=1"));
        // The runner was called with apply=false (a plan, not an apply).
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), false)]
        );
    }

    #[test]
    fn an_armed_mutation_applies_and_is_not_audited_when_non_destructive() {
        let runner = Arc::new(FakeRunner::default());
        let w = worker_with(runner.clone(), true); // apply gate ON
        let reply = w.handle("provision", "{}");
        assert!(reply.ok);
        assert!(!reply.audited, "provision is not destructive");
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), true)]
        );
    }

    #[test]
    fn an_armed_destructive_op_applies_audits_and_reports_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = worker_with(runner.clone(), true).with_db_path(tmp.path().join("events.sqlite"));
        let reply = w.handle("destroy", "{}");
        assert!(reply.ok);
        assert!(reply.audited, "a performed destroy is audited");
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("destroy".into(), true)]
        );
    }

    #[test]
    fn a_lifecycle_verb_requires_an_instance_and_routes_the_action() {
        let runner = Arc::new(FakeRunner::default());
        let w = worker_with(runner.clone(), true);
        // Missing instance → honest rejection, no runner call.
        let bad = w.handle("instance-start", "{}");
        assert!(!bad.ok && bad.error.unwrap().contains("instance"));
        assert!(runner.calls.lock().unwrap().is_empty());
        // With an instance → the start action runs (apply armed).
        let good = w.handle("instance-start", r#"{"instance":"web"}"#);
        assert!(good.ok);
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("lifecycle-start".into(), true)]
        );
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = worker_with(Arc::new(FakeRunner::default()), true);
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown cloud verb"));
    }

    // ── the state mirror ──
    #[test]
    fn build_state_reports_honest_tool_health_and_the_roster_table() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            tofu_up: true,
            ..Default::default()
        });
        let w = worker_with(runner, false);
        let state = w.build_state();
        assert_eq!(state.host, "me");
        assert_eq!(state.adapter, CloudProviderAdapter::ConstructCloud);
        assert!(!state.apply_armed);
        // Health: tofu Up, ansible + libvirt Absent (honest, not faked).
        assert_eq!(
            state.tool_health(TOOL_TOFU).map(|h| h.state),
            Some(HealthState::Up)
        );
        assert_eq!(
            state.tool_health(TOOL_LIBVIRT).map(|h| h.state),
            Some(HealthState::Absent)
        );
        assert!(!state.backend_ready(), "one Up tool is not full readiness");
        // The roster folds into a compute/instances table.
        assert_eq!(state.resources.len(), 1);
        let table = &state.resources[0];
        assert_eq!(table.service_type, "compute");
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.row_label(&table.rows[0]), "web");
    }

    #[test]
    fn normalize_domain_state_maps_libvirt_states_to_neutral_status() {
        assert_eq!(normalize_domain_state("running"), "ACTIVE");
        assert_eq!(normalize_domain_state("shut off"), "SHUTOFF");
        assert_eq!(normalize_domain_state("paused"), "PAUSED");
        assert_eq!(normalize_domain_state("pmsuspended"), "PMSUSPENDED");
    }

    // ── the leader-gated bus drain + reply round-trip ──
    #[tokio::test]
    async fn drain_replies_only_on_the_leader_and_round_trips_a_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        // A KDC-style client publishes a list-instances request.
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .unwrap();

        // A NON-leader drains: it advances its cursor but writes NO reply.
        let follower = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("web", "ACTIVE")],
                ..Default::default()
            }))
            .with_bus_root(Some(bus.clone()))
            .with_leader_override(false);
        let mut cursors = HashMap::new();
        follower.drain_actions(&mut cursors);
        assert!(
            persist
                .list_since(&reply_topic(&req.ulid), None)
                .unwrap()
                .is_empty(),
            "a non-leader must not reply"
        );

        // The leader drains: it handles + writes the reply.
        let leader = CloudWorker::new("l".into(), "peer:l".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("web", "ACTIVE")],
                ..Default::default()
            }))
            .with_bus_root(Some(bus.clone()))
            .with_leader_override(true);
        let mut cursors = HashMap::new();
        assert!(
            leader.drain_actions(&mut cursors),
            "the leader handled a request"
        );
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1, "exactly one reply");
        let reply: CloudReply = serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.instances.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prime_cursors_skips_the_backlog_so_a_restart_does_not_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        // A queued provision predates the worker start.
        persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some("{}"),
            )
            .unwrap();
        let runner = Arc::new(FakeRunner::default());
        let leader = CloudWorker::new("l".into(), "peer:l".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_apply(true)
            .with_bus_root(Some(bus.clone()))
            .with_leader_override(true);
        let mut cursors = HashMap::new();
        leader.prime_cursors(&mut cursors); // seeds past the backlog
        assert!(
            !leader.drain_actions(&mut cursors),
            "the backlog is not replayed"
        );
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "no stale provision fired"
        );
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        let mut w = worker_with(Arc::new(FakeRunner::default()), false)
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
