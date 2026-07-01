//! MV-4 — `container`: the Podman container-lifecycle worker.
//!
//! The container half of the mesh management layer — the exact
//! [`super::vm_lifecycle`] (MV-3) shape, but for **Podman containers** instead of
//! libvirt/KVM VMs. Where MV-3 turns an operator's `action/vm/lifecycle` request
//! into `virsh`/`qemu-img` calls, MV-4 turns an `action/container/lifecycle`
//! request into `podman` calls and publishes the resulting container roster to
//! `event/podman/containers`. Together they cover VMs **and** containers so the
//! Datacenter UI drives both (`docs/design/mesh-virt-management.md`: manage "KVM
//! VMs + Podman containers across the mesh, no single center").
//!
//! It runs on **every** mesh node (like `kvm_health`/`vm_lifecycle`): the KVM +
//! Podman stack is universal, so any node can host datacenter containers. Because
//! the action topic is flat (shared by the whole mesh), each [`ContainerAction`]
//! carries a `host` — the worker acts only on requests addressed to *its* node id
//! ([`ContainerAction::targets`]) and advances past the rest, so one `run` request
//! doesn't fan out to every node.
//!
//! ## Shape (mirrors `vm_lifecycle` + `kvm_health`)
//!
//! - An **injectable [`PodmanBackend`] trait** (`run`/`stop`/`remove`/`list`/
//!   `info`) is the sole seam to the outside. Production wires [`PodmanCli`]
//!   (shells `podman` through the bounded-proc path, [`crate::workers::proc`]);
//!   tests wire a `FakePodman`.
//! - The **pure core** is fully unit-tested with no live podman: the
//!   [`ContainerSpec`] → `podman run -d` argv ([`build_run_argv`] + the argv
//!   builders), the [`parse_podman_ps`] parser, and the [`plan_transition`]
//!   lifecycle state machine. [`apply_action`] composes them over an injected
//!   backend, so the request→action wiring is testable against `FakePodman`
//!   without Podman.
//! - Publishing mirrors `vm_lifecycle` exactly: the `event/podman/containers` body
//!   is fired through the `mde-bus` CLI + [`crate::proc_reap::fire_and_reap`], and
//!   every podman shell-out is bounded by the EFF-20 timeout so a wedged podman
//!   can't pin a runtime thread.
//!
//! ## Why leaner than the libvirt backend
//!
//! Unlike [`super::vm_lifecycle`], there is **no `ensure_default_pool` /
//! `ensure_default_network`** dance and **no `qemu-img`/XML disk-prep step**:
//! `podman run` provisions its own storage graph and attaches the default podman
//! network out of the box, and `run` is a single create-and-start (containers have
//! no separate `define`/`start` split like a VM). The state read also collapses:
//! `podman ps --format json` already carries the container state, so one
//! [`Container`] row plays both the roster-row and the state-detail role (where a
//! VM needs a terse `virsh list` row **and** a richer `virsh dominfo`).
//!
//! ## First slice vs deferred
//!
//! Implemented (this slice): **run** (image + name + optional ports/env/volumes →
//! `podman run -d`), **stop** (graceful `podman stop` or `force` `podman kill`),
//! **remove** (`podman rm`, or `podman rm -f` to evict a running one), **list**
//! (`podman ps --all --format json`), and **info** (a name-filtered `podman ps`).
//!
//! Deferred (intentionally NOT stubbed with `todo!()` — each rides an existing or
//! future worker): pods / `podman-compose` orchestration, image pull-policy + auth,
//! healthchecks + restart policies, resource limits (`--memory`/`--cpus`), extra
//! networks + secrets, logs/exec/attach streaming (E12 egui VDI console), and
//! richer per-container telemetry (VIRT-1 `compute_registry` already publishes
//! cpu/ram to `compute/inventory`).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use thiserror::Error;

use crate::workers::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};

use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for lifecycle requests (flat; per-node targeting
/// is via the request's `host` field, not the topic).
pub const ACTION_TOPIC: &str = "action/container/lifecycle";

/// Bus topic the worker publishes this node's container roster to.
pub const CONTAINERS_TOPIC: &str = "event/podman/containers";

/// Action-drain cadence. The bus read is a cheap local log scan; container
/// lifecycle is a slow, operator-visible event, so a 2 s poll is plenty responsive
/// without spinning podman.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Slow heartbeat for the `event/podman/containers` publish. Between heartbeats the
/// roster is published only right after a handled action (state changed); once this
/// elapses it republishes unconditionally so a freshly-pruned topic / late
/// subscriber still finds a recent roster. Keeps the `podman ps` snapshot off the
/// hot path (queried on-action or every 30 s, never every 2 s tick).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

// ───────────────────────────── data model ─────────────────────────────

/// A published host port → container port mapping (`podman run -p`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortMapping {
    /// Host-side port.
    pub host: u16,
    /// Container-side port.
    pub container: u16,
    /// Optional protocol (`tcp` default when absent; e.g. `udp`).
    #[serde(default)]
    pub protocol: Option<String>,
}

impl PortMapping {
    /// The `-p` value: `host:container` (or `host:container/proto` when a non-empty
    /// protocol is set).
    #[must_use]
    pub fn as_arg(&self) -> String {
        match self.protocol.as_deref() {
            Some(proto) if !proto.is_empty() => {
                format!("{}:{}/{proto}", self.host, self.container)
            }
            _ => format!("{}:{}", self.host, self.container),
        }
    }
}

/// A host-path → container-path bind mount (`podman run -v`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VolumeMount {
    /// Host filesystem path.
    pub host_path: String,
    /// Mount point inside the container.
    pub container_path: String,
    /// Mount read-only (`:ro`).
    #[serde(default)]
    pub read_only: bool,
}

impl VolumeMount {
    /// The `-v` value: `host:container` (or `host:container:ro` when read-only).
    #[must_use]
    pub fn as_arg(&self) -> String {
        if self.read_only {
            format!("{}:{}:ro", self.host_path, self.container_path)
        } else {
            format!("{}:{}", self.host_path, self.container_path)
        }
    }
}

/// A Podman container spec — the operator-facing description [`PodmanBackend::run`]
/// turns into a `podman run -d` (via [`build_run_argv`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContainerSpec {
    /// Container name (also the `--name` + the lifecycle key).
    pub name: String,
    /// Image reference to run (`docker.io/library/nginx:latest`, `postgres:16`, …).
    pub image: String,
    /// Published port mappings (`-p`). Empty ⇒ no published ports.
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    /// Environment variables (`-e KEY=VALUE`). A `BTreeMap` so the argv is
    /// deterministic (sorted by key) and a key can't be double-specified.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Bind mounts (`-v`). Empty ⇒ no host mounts.
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
}

/// One lifecycle command drained off [`ACTION_TOPIC`]. Internally tagged by `op`
/// so the JSON a Datacenter UI publishes is self-describing, e.g.
/// `{"op":"run","host":"node-a","spec":{…}}` /
/// `{"op":"stop","host":"node-a","name":"web1","force":true}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContainerAction {
    /// Create + start a container from `spec` (`podman run -d`). Containers have no
    /// separate define/start split — `run` is the whole birth.
    Run {
        /// Target node id (must match this node to act).
        host: String,
        /// The container to run.
        spec: ContainerSpec,
    },
    /// Stop a running container — graceful `podman stop` (SIGTERM→SIGKILL), or a
    /// hard `podman kill` (immediate SIGKILL) when `force`.
    Stop {
        /// Target node id.
        host: String,
        /// Container name.
        name: String,
        /// Send SIGKILL now instead of a graceful SIGTERM.
        #[serde(default)]
        force: bool,
    },
    /// Remove a container: `podman rm`, or `podman rm -f` (`force`) to evict one
    /// that is still running.
    #[serde(rename = "rm")]
    Remove {
        /// Target node id.
        host: String,
        /// Container name.
        name: String,
        /// Force-remove a running container (`-f`).
        #[serde(default)]
        force: bool,
    },
    /// No-op lifecycle change — just asks the target to re-publish its roster (the
    /// operator's "refresh" button). The `list` verb of the first slice.
    Refresh {
        /// Target node id.
        host: String,
    },
}

impl ContainerAction {
    /// The node id this action is addressed to.
    #[must_use]
    pub fn host(&self) -> &str {
        match self {
            Self::Run { host, .. }
            | Self::Stop { host, .. }
            | Self::Remove { host, .. }
            | Self::Refresh { host } => host,
        }
    }

    /// Whether this action targets `node_id`. An empty target never matches
    /// (fail-safe: an unaddressed run must not fan out to every node).
    #[must_use]
    pub fn targets(&self, node_id: &str) -> bool {
        !self.host().is_empty() && self.host() == node_id
    }
}

/// Parse a [`ContainerAction`] request body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `op`.
pub fn parse_action(body: &str) -> Result<ContainerAction, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed container action: {e}"))
}

/// One row of `podman ps --all --format json` — the lean container record. Because
/// podman's `ps` JSON already carries the state, this single row serves both the
/// published roster **and** the state read (unlike a VM's terse `virsh list` row +
/// separate `virsh dominfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Container {
    /// Container id.
    pub id: String,
    /// Container name (first entry of podman's `Names`).
    pub name: String,
    /// Image reference.
    pub image: String,
    /// Raw podman state string (`running`, `exited`, `created`, `paused`, …).
    pub state: String,
}

impl Container {
    /// This container's state as a [`ContainerState`] the state machine reasons
    /// over.
    #[must_use]
    pub fn state_kind(&self) -> ContainerState {
        container_state_from_str(&self.state)
    }
}

/// The whole-node container roster — the body published to [`CONTAINERS_TOPIC`].
/// `host`-stamped like `vm_lifecycle`'s [`super::vm_lifecycle::InstanceReport`] so a
/// consumer reads one node's row off the flat topic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContainerReport {
    /// Publishing node id.
    pub host: String,
    /// The node's containers in `podman ps --all` order.
    pub containers: Vec<Container>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ─────────────────────────── state machine ───────────────────────────

/// A container's coarse state (the transitions the machine cares about; `Other`
/// folds the transient `stopping`/`removing`/`dead`/`unknown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    /// `running`.
    Running,
    /// `paused`.
    Paused,
    /// `exited` (a stopped container that still exists).
    Exited,
    /// `created` (defined but never started).
    Created,
    /// Any other/transient podman state.
    Other,
}

impl ContainerState {
    /// The canonical `podman` state string for this state (round-trips
    /// [`container_state_from_str`] for the well-known states).
    #[must_use]
    pub fn as_podman_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Exited => "exited",
            Self::Created => "created",
            Self::Other => "unknown",
        }
    }
}

/// Map a raw `podman` state string to a [`ContainerState`]. Case-insensitive
/// (podman emits lowercase, but be tolerant); `stopped` folds to `Exited`.
#[must_use]
pub fn container_state_from_str(s: &str) -> ContainerState {
    match s.trim().to_ascii_lowercase().as_str() {
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "exited" | "stopped" => ContainerState::Exited,
        "created" | "configured" => ContainerState::Created,
        _ => ContainerState::Other,
    }
}

/// The lifecycle operation a request maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerOp {
    /// Create + start a container.
    Run,
    /// Stop a running container.
    Stop,
    /// Remove a container.
    Remove,
}

/// The intended outcome of a valid transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Now running (created + started).
    Started,
    /// Now stopped (exited).
    Stopped,
    /// Removed.
    Removed,
}

/// A rejected transition — the precondition the current state failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransitionError {
    /// Run on a name that already exists (podman won't reuse a name).
    #[error("already exists")]
    AlreadyExists,
    /// An op on a name that doesn't exist.
    #[error("not found")]
    NotFound,
    /// Stop on a container that isn't running.
    #[error("not running")]
    NotRunning,
    /// The op isn't valid from the current state.
    #[error("cannot {op:?} a container in state {state:?}")]
    Invalid {
        /// The rejected op.
        op: ContainerOp,
        /// The state it was rejected from.
        state: ContainerState,
    },
}

/// The pure lifecycle state machine: given the container's current state (`None`
/// = doesn't exist) and an op, either the intended [`Transition`] or the
/// precondition [`TransitionError`]. No I/O — fully unit-testable.
///
/// `Remove` is allowed from **any** existing state (the runtime `-f` flag, not the
/// state, decides whether a running container can go); a plain `rm` of a running
/// container is left for podman itself to reject at the shell, surfaced on the
/// alert lane — mirroring how `vm_lifecycle` lets libvirt reject the edge cases.
///
/// # Errors
/// A [`TransitionError`] when the op is invalid from `current`.
pub fn plan_transition(
    current: Option<ContainerState>,
    op: ContainerOp,
) -> Result<Transition, TransitionError> {
    match (op, current) {
        (ContainerOp::Run, None) => Ok(Transition::Started),
        (ContainerOp::Run, Some(_)) => Err(TransitionError::AlreadyExists),

        (ContainerOp::Stop, None) => Err(TransitionError::NotFound),
        (ContainerOp::Stop, Some(ContainerState::Running | ContainerState::Paused)) => {
            Ok(Transition::Stopped)
        }
        (ContainerOp::Stop, Some(ContainerState::Exited | ContainerState::Created)) => {
            Err(TransitionError::NotRunning)
        }
        (ContainerOp::Stop, Some(state)) => Err(TransitionError::Invalid {
            op: ContainerOp::Stop,
            state,
        }),

        (ContainerOp::Remove, None) => Err(TransitionError::NotFound),
        (ContainerOp::Remove, Some(_)) => Ok(Transition::Removed),
    }
}

// ─────────────────────── pure: podman argv builders ───────────────────────
// Each returns the argv WITHOUT the leading `podman`. Kept pure + tested so the
// command surface can't silently drift (the `mackes-xcp` doctrine).

/// `podman run -d --name <name> [-p …] [-e …] [-v …] <image>` — a detached
/// create-and-start. The image is last (podman: `run [options] IMAGE`); env vars
/// are emitted in sorted key order (the `BTreeMap`), so the argv is deterministic.
#[must_use]
pub fn build_run_argv(spec: &ContainerSpec) -> Vec<String> {
    let mut a = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        spec.name.clone(),
    ];
    for p in &spec.ports {
        a.push("-p".into());
        a.push(p.as_arg());
    }
    for (k, v) in &spec.env {
        a.push("-e".into());
        a.push(format!("{k}={v}"));
    }
    for vol in &spec.volumes {
        a.push("-v".into());
        a.push(vol.as_arg());
    }
    a.push(spec.image.clone());
    a
}

/// `podman stop <name>` (graceful SIGTERM) or `podman kill <name>` (`force`,
/// immediate SIGKILL).
#[must_use]
pub fn build_stop_argv(name: &str, force: bool) -> Vec<String> {
    if force {
        vec!["kill".into(), name.into()]
    } else {
        vec!["stop".into(), name.into()]
    }
}

/// `podman rm <name> [-f]` — remove a container (`-f` force-removes a running one).
#[must_use]
pub fn build_rm_argv(name: &str, force: bool) -> Vec<String> {
    let mut a = vec!["rm".into()];
    if force {
        a.push("-f".into());
    }
    a.push(name.into());
    a
}

/// `podman ps --all --format json` — the whole roster (incl. stopped, like
/// `virsh list --all`, so the panel shows exited containers too).
#[must_use]
pub fn build_ps_argv() -> Vec<String> {
    vec![
        "ps".into(),
        "--all".into(),
        "--format".into(),
        "json".into(),
    ]
}

/// `podman ps --all --filter name=<name> --format json` — the single-container
/// state read. podman's `name=` filter is a substring match, so the caller
/// exact-matches the parsed rows on `name`.
#[must_use]
pub fn build_ps_filter_argv(name: &str) -> Vec<String> {
    vec![
        "ps".into(),
        "--all".into(),
        "--filter".into(),
        format!("name={name}"),
        "--format".into(),
        "json".into(),
    ]
}

// ─────────────────────────── pure: parser ───────────────────────────

/// Extract the primary name from a podman `Names` value — an array (modern
/// podman, first element) or a bare string (older). Empty when absent.
fn extract_name(names: Option<&serde_json::Value>) -> String {
    match names {
        Some(v) if v.is_array() => v
            .as_array()
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some(v) => v.as_str().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// Parse the JSON payload of `podman ps --all --format json` into [`Container`]
/// rows. Malformed JSON (or a missing podman's empty output) yields an empty Vec;
/// a nameless row (junk) is skipped.
#[must_use]
pub fn parse_podman_ps(stdout: &str) -> Vec<Container> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|row| {
            let name = extract_name(row.get("Names"));
            if name.is_empty() {
                return None;
            }
            Some(Container {
                id: row
                    .get("Id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                name,
                image: row
                    .get("Image")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                state: row
                    .get("State")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

// ─────────────────────── backend trait + errors ───────────────────────

/// A podman-access failure.
#[derive(Debug, Error)]
pub enum PodmanError {
    /// The `podman` process couldn't be spawned or timed out.
    #[error("spawn {0}: {1}")]
    Spawn(String, #[source] std::io::Error),
    /// A command exited non-zero — carries the sub-command, exit code, and any
    /// captured stderr.
    #[error("{cmd} failed (exit {code}): {stderr}")]
    Command {
        /// The failing sub-command (first argv element).
        cmd: String,
        /// Process exit code (or -1 if killed by signal).
        code: i32,
        /// Captured stderr (empty for status-only calls whose stderr is nulled).
        stderr: String,
    },
}

/// The injectable podman-access seam (MV-4). [`PodmanCli`] is the production
/// `podman` impl; a `FakePodman` drives the unit tests.
pub trait PodmanBackend {
    /// Create + start a container from `spec` (`podman run -d`).
    ///
    /// # Errors
    /// Spawn / non-zero podman failures.
    fn run(&self, spec: &ContainerSpec) -> Result<(), PodmanError>;

    /// Stop a container — graceful, or a hard `podman kill` when `force`.
    ///
    /// # Errors
    /// Spawn / non-zero podman failures.
    fn stop(&self, name: &str, force: bool) -> Result<(), PodmanError>;

    /// Remove a container, optionally force-removing a running one.
    ///
    /// # Errors
    /// Spawn / non-zero podman failures.
    fn remove(&self, name: &str, force: bool) -> Result<(), PodmanError>;

    /// The node's container roster (`podman ps --all`).
    ///
    /// # Errors
    /// Spawn / non-zero podman failures.
    fn list(&self) -> Result<Vec<Container>, PodmanError>;

    /// Detail for one container, or `None` when it doesn't exist.
    ///
    /// # Errors
    /// Spawn / non-zero podman failures (a not-found is `Ok(None)`, not an error).
    fn info(&self, name: &str) -> Result<Option<Container>, PodmanError>;
}

/// Production [`PodmanBackend`]: shells `podman` through the bounded
/// [`crate::workers::proc`] path so a wedged podman can't pin a runtime thread.
/// Stateless — every call is a fresh bounded process.
#[derive(Debug, Clone, Default)]
pub struct PodmanCli;

impl PodmanCli {
    /// Construct the production podman CLI backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run `podman <args>` to completion (status only; stdout/stderr nulled),
    /// bounded.
    fn podman_status(&self, args: &[String]) -> Result<(), PodmanError> {
        let _ = self;
        let mut cmd = Command::new("podman");
        cmd.args(args);
        let st = status_with_timeout(cmd, DEFAULT_CMD_TIMEOUT)
            .map_err(|e| PodmanError::Spawn("podman".to_string(), e))?;
        if st.success() {
            Ok(())
        } else {
            Err(PodmanError::Command {
                cmd: args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "podman".to_string()),
                code: st.code().unwrap_or(-1),
                stderr: String::new(),
            })
        }
    }

    /// Run `podman <args>` capturing stdout+stderr, bounded.
    fn podman_output(&self, args: &[String]) -> Result<(bool, String, String), PodmanError> {
        let _ = self;
        let mut cmd = Command::new("podman");
        cmd.args(args);
        let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT)
            .map_err(|e| PodmanError::Spawn("podman".to_string(), e))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

impl PodmanBackend for PodmanCli {
    fn run(&self, spec: &ContainerSpec) -> Result<(), PodmanError> {
        self.podman_status(&build_run_argv(spec))
    }

    fn stop(&self, name: &str, force: bool) -> Result<(), PodmanError> {
        self.podman_status(&build_stop_argv(name, force))
    }

    fn remove(&self, name: &str, force: bool) -> Result<(), PodmanError> {
        self.podman_status(&build_rm_argv(name, force))
    }

    fn list(&self) -> Result<Vec<Container>, PodmanError> {
        let (ok, stdout, stderr) = self.podman_output(&build_ps_argv())?;
        if !ok {
            return Err(PodmanError::Command {
                cmd: "ps".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        Ok(parse_podman_ps(&stdout))
    }

    fn info(&self, name: &str) -> Result<Option<Container>, PodmanError> {
        let (ok, stdout, stderr) = self.podman_output(&build_ps_filter_argv(name))?;
        if !ok {
            // A not-found container is `Ok(None)`, not an error — that's how the
            // run precondition (name must not exist) reads a fresh name.
            let s = stderr.to_ascii_lowercase();
            if s.contains("no such container") || s.contains("not found") {
                return Ok(None);
            }
            return Err(PodmanError::Command {
                cmd: "ps".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        // podman's `name=` filter is a substring match — exact-match the rows.
        Ok(parse_podman_ps(&stdout)
            .into_iter()
            .find(|c| c.name == name))
    }
}

/// Apply one lifecycle action over an injected [`PodmanBackend`]: read the
/// container's current state, validate the transition ([`plan_transition`]), then
/// call the backend. `Refresh` is a no-op (it only nudges a roster publish). Pure
/// over the backend seam — driven by `FakePodman` in tests.
///
/// # Errors
/// A human-readable message when the transition is invalid or a backend call
/// fails (the worker logs it on the alert lane and moves on).
pub fn apply_action(backend: &dyn PodmanBackend, action: &ContainerAction) -> Result<(), String> {
    let (name, op) = match action {
        ContainerAction::Refresh { .. } => return Ok(()),
        ContainerAction::Run { spec, .. } => (spec.name.as_str(), ContainerOp::Run),
        ContainerAction::Stop { name, .. } => (name.as_str(), ContainerOp::Stop),
        ContainerAction::Remove { name, .. } => (name.as_str(), ContainerOp::Remove),
    };
    let current = backend
        .info(name)
        .map_err(|e| e.to_string())?
        .map(|c| c.state_kind());
    plan_transition(current, op).map_err(|e| format!("container '{name}': {e}"))?;
    match action {
        ContainerAction::Run { spec, .. } => backend.run(spec).map_err(|e| e.to_string()),
        ContainerAction::Stop { name, force, .. } => {
            backend.stop(name, *force).map_err(|e| e.to_string())
        }
        ContainerAction::Remove { name, force, .. } => {
            backend.remove(name, *force).map_err(|e| e.to_string())
        }
        ContainerAction::Refresh { .. } => Ok(()),
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Publish a [`ContainerReport`] to [`CONTAINERS_TOPIC`] via the `mde-bus` CLI —
/// the same fire-and-reap path `vm_lifecycle` uses. Best-effort: a missing
/// `mde-bus` binary (pre-RPM dev box) is swallowed, and the detached reaper
/// prevents a zombie pile.
fn publish_containers(report: &ContainerReport) {
    let Ok(body) = serde_json::to_string(report) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", CONTAINERS_TOPIC, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `vm_lifecycle`.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<ContainerAction> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_action(body) {
            Ok(a) => out.push(a),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "container: bad container action")
            }
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-execute the whole backlog of lifecycle commands — a queued `rm` shouldn't
/// fire on the next daemon restart. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The MV-4 container worker.
pub struct ContainerWorker {
    /// This node's id — the event `host` stamp AND the action target this worker
    /// matches ([`ContainerAction::targets`]).
    host: String,
    /// The injectable podman seam (production: [`PodmanCli`]). `Arc` so each action
    /// runs on a `spawn_blocking` thread without borrowing `self`.
    backend: Arc<dyn PodmanBackend + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Roster-publish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl ContainerWorker {
    /// Construct with production defaults: the live [`PodmanCli`] backend, the
    /// default cadences, and the auto-resolved bus root. `host` is this node's id
    /// (the action target + event stamp).
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            backend: Arc::new(PodmanCli::new()),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
        }
    }

    /// Inject a backend (tests). Production uses the [`PodmanCli`] default.
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn PodmanBackend + Send + Sync>) -> Self {
        self.backend = backend;
        self
    }

    /// Override the action-drain cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Drain + apply new actions addressed to this node. Returns `true` when any
    /// action ran (so the caller force-publishes the fresh roster).
    async fn drain_and_apply(&self, bus_root: &Path, cursor: &mut Option<String>) -> bool {
        let actions = read_new_actions(bus_root, cursor);
        let mut acted = false;
        for action in actions {
            if !action.targets(&self.host) {
                continue;
            }
            // Refresh is a pure publish nudge — no backend work.
            if matches!(action, ContainerAction::Refresh { .. }) {
                acted = true;
                continue;
            }
            let backend = Arc::clone(&self.backend);
            match tokio::task::spawn_blocking(move || apply_action(&*backend, &action)).await {
                Ok(Ok(())) => acted = true,
                Ok(Err(e)) => {
                    // Alert lane (mirrors kvm_health) — a failed operator action is
                    // operator-visible.
                    tracing::warn!(
                        target: "mackesd::alert",
                        "ALERT (warn): container action failed — {e}",
                    );
                }
                Err(e) => tracing::warn!(error = %e, "container: action task join failed"),
            }
        }
        acted
    }

    /// Snapshot the roster (`podman ps`) + publish it, gated so the podman call
    /// stays off the hot path: query only when `force` (an action just changed
    /// state) or the heartbeat has elapsed since `last_at`.
    async fn publish_snapshot(&self, last_at: &mut Option<Instant>, force: bool) {
        let now = Instant::now();
        let due = force
            || match last_at {
                None => true,
                Some(at) => now.duration_since(*at) >= self.heartbeat,
            };
        if !due {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let containers = match tokio::task::spawn_blocking(move || backend.list()).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "container: list failed; skipping publish");
                return;
            }
            Err(_) => return,
        };
        let report = ContainerReport {
            host: self.host.clone(),
            containers,
            published_at_ms: now_ms(),
        };
        publish_containers(&report);
        *last_at = Some(now);
    }
}

#[async_trait::async_trait]
impl Worker for ContainerWorker {
    fn name(&self) -> &'static str {
        "container"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        // Publish an immediate roster on start so a panel doesn't wait a full
        // heartbeat for the first row.
        let mut last_pub: Option<Instant> = None;
        self.publish_snapshot(&mut last_pub, true).await;
        // Skip any backlog so a restart doesn't re-run stale lifecycle commands.
        let mut cursor = bus_root.as_deref().and_then(prime_cursor);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let acted = if let Some(root) = &bus_root {
                        self.drain_and_apply(root, &mut cursor).await
                    } else {
                        false
                    };
                    self.publish_snapshot(&mut last_pub, acted).await;
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── request parsing ──

    #[test]
    fn parse_run_action_round_trips() {
        let body = r#"{"op":"run","host":"node-a","spec":{"name":"web1","image":"nginx:latest","ports":[{"host":8080,"container":80}],"env":{"TZ":"UTC"},"volumes":[{"host_path":"/data","container_path":"/var/data"}]}}"#;
        let a = parse_action(body).expect("parse");
        match &a {
            ContainerAction::Run { host, spec } => {
                assert_eq!(host, "node-a");
                assert_eq!(spec.name, "web1");
                assert_eq!(spec.image, "nginx:latest");
                assert_eq!(spec.ports.len(), 1);
                assert_eq!(spec.ports[0].host, 8080);
                assert_eq!(spec.ports[0].container, 80);
                assert_eq!(spec.env.get("TZ").map(String::as_str), Some("UTC"));
                assert_eq!(spec.volumes[0].host_path, "/data");
                assert_eq!(spec.volumes[0].container_path, "/var/data");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(a.targets("node-a"));
        assert!(!a.targets("node-b"));
    }

    #[test]
    fn parse_optional_fields_default() {
        let body = r#"{"op":"run","host":"n","spec":{"name":"d","image":"busybox"}}"#;
        let a = parse_action(body).expect("parse");
        let ContainerAction::Run { spec, .. } = a else {
            panic!("expected run");
        };
        assert!(spec.ports.is_empty());
        assert!(spec.env.is_empty());
        assert!(spec.volumes.is_empty());
    }

    #[test]
    fn parse_stop_and_rm_flags() {
        let stop = parse_action(r#"{"op":"stop","host":"n","name":"web1","force":true}"#).unwrap();
        assert_eq!(
            stop,
            ContainerAction::Stop {
                host: "n".into(),
                name: "web1".into(),
                force: true
            }
        );
        // force defaults to false.
        let graceful = parse_action(r#"{"op":"stop","host":"n","name":"web1"}"#).unwrap();
        assert_eq!(
            graceful,
            ContainerAction::Stop {
                host: "n".into(),
                name: "web1".into(),
                force: false
            }
        );
        // The remove verb is wire-named `rm`.
        let rm = parse_action(r#"{"op":"rm","host":"n","name":"web1","force":true}"#).unwrap();
        assert_eq!(
            rm,
            ContainerAction::Remove {
                host: "n".into(),
                name: "web1".into(),
                force: true
            }
        );
    }

    #[test]
    fn parse_refresh_and_reject_malformed() {
        assert_eq!(
            parse_action(r#"{"op":"refresh","host":"n"}"#).unwrap(),
            ContainerAction::Refresh { host: "n".into() }
        );
        assert!(parse_action("nope").is_err());
        assert!(parse_action(r#"{"op":"teleport","host":"n"}"#).is_err());
        // `remove` is NOT the wire op — only `rm` is.
        assert!(parse_action(r#"{"op":"remove","host":"n","name":"x"}"#).is_err());
    }

    #[test]
    fn empty_host_never_targets() {
        // Fail-safe: an unaddressed run must not fan out to every node.
        let a = ContainerAction::Refresh {
            host: String::new(),
        };
        assert!(!a.targets("node-a"));
        assert!(!a.targets(""));
    }

    // ── argv builders ──

    #[test]
    fn run_argv_with_ports_env_volumes() {
        let mut env = BTreeMap::new();
        // Insert out of order — the BTreeMap sorts, so the argv is deterministic.
        env.insert("TZ".to_string(), "UTC".to_string());
        env.insert("LANG".to_string(), "C".to_string());
        let spec = ContainerSpec {
            name: "web1".into(),
            image: "docker.io/library/nginx:latest".into(),
            ports: vec![
                PortMapping {
                    host: 8080,
                    container: 80,
                    protocol: None,
                },
                PortMapping {
                    host: 53,
                    container: 53,
                    protocol: Some("udp".into()),
                },
            ],
            env,
            volumes: vec![VolumeMount {
                host_path: "/data".into(),
                container_path: "/var/data".into(),
                read_only: true,
            }],
        };
        assert_eq!(
            build_run_argv(&spec),
            vec![
                "run",
                "-d",
                "--name",
                "web1",
                "-p",
                "8080:80",
                "-p",
                "53:53/udp",
                "-e",
                "LANG=C", // sorted before TZ
                "-e",
                "TZ=UTC",
                "-v",
                "/data:/var/data:ro",
                "docker.io/library/nginx:latest", // image is LAST
            ]
        );
    }

    #[test]
    fn run_argv_minimal_is_just_name_and_image() {
        let spec = ContainerSpec {
            name: "bare".into(),
            image: "busybox".into(),
            ..ContainerSpec::default()
        };
        assert_eq!(
            build_run_argv(&spec),
            vec!["run", "-d", "--name", "bare", "busybox"]
        );
    }

    #[test]
    fn stop_rm_ps_argv_shapes() {
        // Graceful stop vs. force kill.
        assert_eq!(build_stop_argv("web1", false), vec!["stop", "web1"]);
        assert_eq!(build_stop_argv("web1", true), vec!["kill", "web1"]);
        // rm vs. rm -f.
        assert_eq!(build_rm_argv("web1", false), vec!["rm", "web1"]);
        assert_eq!(build_rm_argv("web1", true), vec!["rm", "-f", "web1"]);
        assert_eq!(build_ps_argv(), vec!["ps", "--all", "--format", "json"]);
        assert_eq!(
            build_ps_filter_argv("web1"),
            vec!["ps", "--all", "--filter", "name=web1", "--format", "json"]
        );
    }

    // ── parser ──

    #[test]
    fn parse_ps_multi_and_fields() {
        let raw = r#"[
            {"Id":"abc123","Names":["web1"],"Image":"docker.io/library/nginx:latest","State":"running"},
            {"Id":"def456","Names":["db"],"Image":"postgres:16","State":"exited"}
        ]"#;
        let v = parse_podman_ps(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            Container {
                id: "abc123".into(),
                name: "web1".into(),
                image: "docker.io/library/nginx:latest".into(),
                state: "running".into(),
            }
        );
        assert_eq!(v[1].name, "db");
        assert_eq!(v[1].state, "exited");
        assert_eq!(v[1].state_kind(), ContainerState::Exited);
    }

    #[test]
    fn parse_ps_empty_and_malformed() {
        // podman prints `[]` for an empty host.
        assert!(parse_podman_ps("[]").is_empty());
        assert!(parse_podman_ps("").is_empty());
        assert!(parse_podman_ps("not json").is_empty());
        // A nameless junk row is skipped.
        assert!(parse_podman_ps(r#"[{"Id":"x","State":"running"}]"#).is_empty());
    }

    #[test]
    fn parse_ps_tolerates_string_names() {
        // Older podman emitted Names as a bare string.
        let v = parse_podman_ps(r#"[{"Id":"z","Names":"solo","Image":"i","State":"created"}]"#);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "solo");
        assert_eq!(v[0].state_kind(), ContainerState::Created);
    }

    #[test]
    fn container_state_mapping_and_roundtrip() {
        assert_eq!(container_state_from_str("running"), ContainerState::Running);
        assert_eq!(container_state_from_str("exited"), ContainerState::Exited);
        assert_eq!(container_state_from_str("stopped"), ContainerState::Exited);
        assert_eq!(container_state_from_str("paused"), ContainerState::Paused);
        assert_eq!(container_state_from_str("created"), ContainerState::Created);
        assert_eq!(container_state_from_str("RUNNING"), ContainerState::Running); // case-insensitive
        assert_eq!(container_state_from_str("removing"), ContainerState::Other);
        // The well-known states round-trip through the canonical string.
        for s in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Exited,
            ContainerState::Created,
        ] {
            assert_eq!(container_state_from_str(s.as_podman_str()), s);
        }
    }

    // ── state machine ──

    #[test]
    fn plan_transition_run() {
        assert_eq!(
            plan_transition(None, ContainerOp::Run),
            Ok(Transition::Started)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Running), ContainerOp::Run),
            Err(TransitionError::AlreadyExists)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Exited), ContainerOp::Run),
            Err(TransitionError::AlreadyExists)
        );
    }

    #[test]
    fn plan_transition_stop() {
        assert_eq!(
            plan_transition(Some(ContainerState::Running), ContainerOp::Stop),
            Ok(Transition::Stopped)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Paused), ContainerOp::Stop),
            Ok(Transition::Stopped)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Exited), ContainerOp::Stop),
            Err(TransitionError::NotRunning)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Created), ContainerOp::Stop),
            Err(TransitionError::NotRunning)
        );
        assert_eq!(
            plan_transition(None, ContainerOp::Stop),
            Err(TransitionError::NotFound)
        );
        // A transient state is an Invalid stop.
        assert_eq!(
            plan_transition(Some(ContainerState::Other), ContainerOp::Stop),
            Err(TransitionError::Invalid {
                op: ContainerOp::Stop,
                state: ContainerState::Other
            })
        );
    }

    #[test]
    fn plan_transition_remove() {
        // Remove is valid from any existing state (the -f flag decides at runtime).
        assert_eq!(
            plan_transition(Some(ContainerState::Running), ContainerOp::Remove),
            Ok(Transition::Removed)
        );
        assert_eq!(
            plan_transition(Some(ContainerState::Exited), ContainerOp::Remove),
            Ok(Transition::Removed)
        );
        assert_eq!(
            plan_transition(None, ContainerOp::Remove),
            Err(TransitionError::NotFound)
        );
    }

    // ── FakePodman-driven apply_action (no live podman) ──

    /// An in-memory [`PodmanBackend`] recording calls + container states, so
    /// [`apply_action`] is exercised end-to-end without Podman.
    struct FakePodman {
        containers: Mutex<BTreeMap<String, ContainerState>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakePodman {
        fn new() -> Self {
            Self {
                containers: Mutex::new(BTreeMap::new()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn with_container(name: &str, state: ContainerState) -> Self {
            let f = Self::new();
            f.containers.lock().unwrap().insert(name.to_string(), state);
            f
        }
        fn record(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_string());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn state_of(&self, name: &str) -> Option<ContainerState> {
            self.containers.lock().unwrap().get(name).copied()
        }
    }

    impl PodmanBackend for FakePodman {
        fn run(&self, spec: &ContainerSpec) -> Result<(), PodmanError> {
            self.record(&format!("run:{}", spec.name));
            self.containers
                .lock()
                .unwrap()
                .insert(spec.name.clone(), ContainerState::Running);
            Ok(())
        }
        fn stop(&self, name: &str, force: bool) -> Result<(), PodmanError> {
            self.record(&format!("stop:{name}:force={force}"));
            self.containers
                .lock()
                .unwrap()
                .insert(name.to_string(), ContainerState::Exited);
            Ok(())
        }
        fn remove(&self, name: &str, force: bool) -> Result<(), PodmanError> {
            self.record(&format!("remove:{name}:force={force}"));
            self.containers.lock().unwrap().remove(name);
            Ok(())
        }
        fn list(&self) -> Result<Vec<Container>, PodmanError> {
            Ok(self
                .containers
                .lock()
                .unwrap()
                .iter()
                .map(|(n, s)| Container {
                    id: "-".into(),
                    name: n.clone(),
                    image: "img".into(),
                    state: s.as_podman_str().into(),
                })
                .collect())
        }
        fn info(&self, name: &str) -> Result<Option<Container>, PodmanError> {
            Ok(self.state_of(name).map(|s| Container {
                name: name.to_string(),
                state: s.as_podman_str().into(),
                ..Container::default()
            }))
        }
    }

    fn run_action(name: &str) -> ContainerAction {
        ContainerAction::Run {
            host: "node-a".into(),
            spec: ContainerSpec {
                name: name.into(),
                image: "nginx".into(),
                ..ContainerSpec::default()
            },
        }
    }

    #[test]
    fn apply_run_starts_a_fresh_container() {
        let fake = FakePodman::new();
        apply_action(&fake, &run_action("web1")).expect("run ok");
        // No pool/network ensure dance — run is the sole call.
        assert_eq!(fake.calls(), vec!["run:web1".to_string()]);
        assert_eq!(fake.state_of("web1"), Some(ContainerState::Running));
    }

    #[test]
    fn apply_double_run_is_rejected_by_the_state_machine() {
        let fake = FakePodman::with_container("web1", ContainerState::Running);
        let err = apply_action(&fake, &run_action("web1")).expect_err("already exists");
        assert!(err.contains("already exists"), "{err}");
        // The backend run was NOT called (guarded before the seam).
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn apply_run_stop_remove_lifecycle() {
        let fake = FakePodman::new();
        apply_action(&fake, &run_action("web1")).expect("run ok");
        assert_eq!(fake.state_of("web1"), Some(ContainerState::Running));

        // Force stop passes the flag through (→ podman kill).
        let stop = ContainerAction::Stop {
            host: "node-a".into(),
            name: "web1".into(),
            force: true,
        };
        apply_action(&fake, &stop).expect("stop ok");
        assert_eq!(fake.state_of("web1"), Some(ContainerState::Exited));
        assert!(fake.calls().iter().any(|c| c == "stop:web1:force=true"));

        let remove = ContainerAction::Remove {
            host: "node-a".into(),
            name: "web1".into(),
            force: true,
        };
        apply_action(&fake, &remove).expect("remove ok");
        assert_eq!(fake.state_of("web1"), None); // removed
        assert!(fake.calls().iter().any(|c| c == "remove:web1:force=true"));
    }

    #[test]
    fn apply_stop_on_exited_is_not_running() {
        let fake = FakePodman::with_container("web1", ContainerState::Exited);
        let stop = ContainerAction::Stop {
            host: "node-a".into(),
            name: "web1".into(),
            force: false,
        };
        let err = apply_action(&fake, &stop).expect_err("not running");
        assert!(err.contains("not running"), "{err}");
    }

    #[test]
    fn apply_stop_missing_is_not_found() {
        let fake = FakePodman::new();
        let stop = ContainerAction::Stop {
            host: "node-a".into(),
            name: "ghost".into(),
            force: false,
        };
        let err = apply_action(&fake, &stop).expect_err("not found");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn apply_refresh_is_a_noop() {
        let fake = FakePodman::new();
        apply_action(
            &fake,
            &ContainerAction::Refresh {
                host: "node-a".into(),
            },
        )
        .expect("noop");
        assert!(fake.calls().is_empty());
    }

    // ── report + topics ──

    #[test]
    fn container_report_round_trips_json() {
        let report = ContainerReport {
            host: "node-a".into(),
            containers: vec![Container {
                id: "abc".into(),
                name: "web1".into(),
                image: "nginx".into(),
                state: "running".into(),
            }],
            published_at_ms: 42,
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: ContainerReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, report);
    }

    #[test]
    fn topics_are_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/container/lifecycle");
        assert_eq!(CONTAINERS_TOPIC, "event/podman/containers");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert!(CONTAINERS_TOPIC.starts_with("event/"));
    }

    #[test]
    fn worker_name_matches_module() {
        let w = ContainerWorker::new("node".to_string());
        assert_eq!(w.name(), "container");
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        // Drives run() with a FakePodman + a temp bus root (no live podman, no real
        // bus actions) and exits promptly on shutdown.
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = ContainerWorker::new("node".to_string())
            .with_backend(Arc::new(FakePodman::new()))
            .with_bus_root(dir.path().to_path_buf())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
