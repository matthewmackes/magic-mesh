//! The **Workloads** app (WL-ARCH-006) — the single workspace for every
//! delivery-type workload on the local-first **OpenTofu + Ansible + libvirt +
//! Podman** backend (WL-ARCH-001).
//!
//! The surface is organized as a lifecycle-first operations app: a persistent
//! sidebar opens **Provision**, **Plan**, **Run**, **Drift**, **Audit**,
//! **Images**, and **Containers** routes; delivery types are filters inside the
//! resource table and provision form rather than the top-level navigation.
//!
//! ## Layout (the U3 seam)
//!
//! This module owns the durable seam the six panel workers (U14–U19) plug into:
//! the nav, the folded `state/cloud` mirror, the review-sheet arming + audit
//! backend wiring, and the dispatch to each route's own render fn. Each panel
//! lives in its own file and owns its own `State` sub-struct, so a downstream
//! worker adds panel-specific state + rendering in THEIR file and never edits
//! this one.
//!
//! ## How the cloud is consumed (§6)
//!
//! The shell never depends on `mackesd`. It **reads** the per-node status mirror
//! `state/cloud/<node>` ([`CloudState`], folded across every node — now carrying
//! per-workload rows / drift / capacity) off the Bus, and **emits**
//! `action/cloud/*` verbs as typed request/reply (the reply lands on
//! `reply/<request-ulid>`). Only the mesh-neutral [`mackes_mesh_types::cloud`]
//! shapes are shared; the worker owns the actual `tofu` / `ansible-playbook` /
//! `virsh` / `podman` execution. The surface never shells a tool itself.
//!
//! Every state is honest (§7): an off-mesh Bus is a silent degrade, an empty
//! roster is a real "no workloads", and a panel with no landed backend render
//! draws an honest **not yet built** stub rather than fake data. Every
//! destructive intent passes a typed-confirm echo first (RUN-006), while
//! prepared service/image changes use a simple Yes/No review. Every performed
//! op lands in the session audit trail.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mde_egui::egui::{self, Color32, RichText, Sense};
use mde_egui::{carbon_icon, card, field, muted_note, Style};

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::android_provider::AndroidVdiSource;
use mackes_mesh_types::app_catalog::is_valid_flatpak_app_id;
use mackes_mesh_types::cloud::{
    cloud_request_digest, decode_cloud_arm_credential, CloudArmSigner, CloudArmedToken,
    CloudReply as WireCloudReply, CloudState, DeliveryType, DeploymentRole, DriftFlag, WorkloadRow,
    WorkloadSpec, APP_VM_ALLOWED_CAPABILITIES, CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_CREDENTIAL,
    CLOUD_ARM_NODE_SCOPE, CLOUD_STATE_PREFIX, VERB_ANDROID_LIFECYCLE, VERB_ANDROID_PROVISION,
    VERB_PLAN, VERB_SET_DESIRED,
};
use mackes_mesh_types::music_auth::{self, MusicAuthContext};
use mackes_mesh_types::workloads::{
    WorkloadAttachmentProtocol, WorkloadBackend, WorkloadOperationAction, WorkloadOperationRequest,
    WorkloadProfile,
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

use crate::bus_reader::BusReader;

/// User-facing cloud product name. The backend toolchain (OpenTofu / Ansible /
/// libvirt / Podman) stays behind the Bus + shared payload contracts, so a later
/// backend can satisfy the same UI seam.
pub(super) const CLOUD_PRODUCT_LABEL: &str = "Construct Cloud";

/// The workspace title the MENUBAR-ALL bar wears.
pub(super) const WORKSPACE_TITLE: &str = "Workloads";

/// The typed-confirm echo an apply intent must match before the verb publishes
/// (RUN-006's typed-arming idiom — the destructive-op hard wall).
const APPLY_ECHO: &str = "apply";

/// Typed Android launcher actions use the audited action worker rather than the
/// cloud desired-state verbs. The body still carries only the closed app enum;
/// these constants bind the capability to the worker's fixed execution lane.
const EXEC_ACTION_TOPIC: &str = "action/exec/request";
const EXEC_AUTH_VERB: &str = "exec-request";
const EXEC_AUTH_NODE: &str = "fleet-control";

const ANDROID_VDI_HANDOFF_ID: &str = "iac-android-vdi-handoff";
const ANDROID_CATALOG_CACHE_ROOT: &str = "/var/lib/mackesd/android-catalog";

/// Typed handoff from governed Android Workloads into the shell's existing
/// remote-session attachment authority. It is discovery identity only; the VDI
/// route must still acquire its own attachment authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AndroidVdiHandoff {
    pub(crate) placement_node: String,
    pub(crate) source: AndroidVdiSource,
}

/// Consume one IAC-owned Android VDI handoff. The shell integration lane can
/// call this without parsing an endpoint or accepting a raw connection string.
pub(crate) fn take_android_vdi_handoff(ctx: &egui::Context) -> Option<AndroidVdiHandoff> {
    ctx.data_mut(|data| {
        data.remove_temp::<Option<AndroidVdiHandoff>>(egui::Id::new(ANDROID_VDI_HANDOFF_ID))
            .flatten()
    })
}

fn queue_android_vdi_handoff(ctx: &egui::Context, handoff: AndroidVdiHandoff) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(ANDROID_VDI_HANDOFF_ID), Some(handoff)));
}

/// The systemd credential is a 32-byte HMAC key encoded as 64 hex characters.
/// Keep a small amount of whitespace headroom while refusing an unexpected file
/// from becoming an unbounded allocation in the root shell.
const MAX_CLOUD_ARM_CREDENTIAL_BYTES: usize = 4 * 1024;

/// Capability lifetime: enough for one local publish and mesh drain, while
/// limiting interception value on the deliberately public Bus.
const ARM_TOKEN_TTL_MS: i64 = 30_000;
const MUSIC_AUTH_TTL_MS: i64 = music_auth::MUSIC_AUTH_MAX_TTL_MS;
const MAX_MUSIC_AUTH_SEED_BYTES: usize = 4 * 1024;

/// Load the production mint authority. Only the root DRM-shell service with its
/// private systemd credential can mint; ad-hoc user sessions fail closed.
fn production_cloud_arm_signer() -> Result<CloudArmSigner, String> {
    if !rustix::process::geteuid().is_root() {
        return Err(
            "Live mutation authorization is available only in the root DRM shell.".to_string(),
        );
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            "The root shell has no systemd cloud-arming credential; live mutation is disabled."
                .to_string()
        })?;
    let path = directory.join(CLOUD_ARM_CREDENTIAL);
    let raw = read_cloud_arm_credential(&path)?;
    let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
    CloudArmSigner::new(key).map_err(str::to_string)
}

/// Read the root shell's systemd credential through a bounded, regular-file
/// boundary. The credential directory is privileged input, but the final leaf
/// is still opened without following a planted Linux symlink.
fn read_cloud_arm_credential(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400000); // O_NOFOLLOW
    }
    #[cfg(all(
        unix,
        not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        ))
    ))]
    if !std::fs::symlink_metadata(path)
        .map_err(|e| format!("Could not inspect systemd cloud-arming credential: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("systemd cloud-arming credential is not a regular file".to_string());
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .map_err(|e| format!("Could not inspect systemd cloud-arming credential: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("systemd cloud-arming credential is not a regular file".to_string());
    }

    let file = options
        .open(path)
        .map_err(|e| format!("Could not read systemd cloud-arming credential: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Could not inspect systemd cloud-arming credential: {e}"))?;
    if !metadata.file_type().is_file() {
        return Err("systemd cloud-arming credential is not a regular file".to_string());
    }
    if metadata.len() > MAX_CLOUD_ARM_CREDENTIAL_BYTES as u64 {
        return Err(format!(
            "systemd cloud-arming credential exceeds {MAX_CLOUD_ARM_CREDENTIAL_BYTES} bytes"
        ));
    }

    let mut raw = Vec::with_capacity(
        MAX_CLOUD_ARM_CREDENTIAL_BYTES
            .min(usize::try_from(metadata.len()).unwrap_or(MAX_CLOUD_ARM_CREDENTIAL_BYTES)),
    );
    file.take((MAX_CLOUD_ARM_CREDENTIAL_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|e| format!("Could not read systemd cloud-arming credential: {e}"))?;
    if raw.len() > MAX_CLOUD_ARM_CREDENTIAL_BYTES {
        return Err(format!(
            "systemd cloud-arming credential exceeds {MAX_CLOUD_ARM_CREDENTIAL_BYTES} bytes"
        ));
    }
    Ok(raw)
}

/// Insert a short-lived, request-body-bound capability into a frozen JSON body.
fn authorize_body_with_signer(
    signer: &CloudArmSigner,
    body: &str,
    verb: &str,
    node: &str,
    target: &str,
) -> Result<String, String> {
    for (label, value) in [("verb", verb), ("node", node), ("target", target)] {
        if value.contains('|') || value.len() > 255 || value.trim().is_empty() {
            return Err(format!(
                "Mutation authorization {label} is not capability-safe."
            ));
        }
    }
    use rand::RngCore as _;
    let mut nonce_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = nonce_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .map_err(|_| "The system clock is before the Unix epoch.".to_string())?;
    let token = CloudArmedToken::mint(
        signer,
        &nonce,
        now.saturating_add(ARM_TOKEN_TTL_MS),
        verb,
        node,
        target,
        &cloud_request_digest(body).map_err(str::to_string)?,
    )
    .encode();
    let mut document: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid mutation request body: {e}"))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Mutation request body is not a JSON object.".to_string())?;
    object.insert("armed_token".to_string(), serde_json::Value::String(token));
    Ok(document.to_string())
}

/// Mint authority for the shell's older direct-libvirt publisher surfaces.
/// Tests use a deterministic key so their isolated Bus contracts remain
/// executable; production always goes through the root/systemd loader above.
pub(crate) fn authorize_root_mutation_body(
    body: &str,
    verb: &str,
    node: &str,
    target: &str,
) -> Result<String, String> {
    #[cfg(test)]
    let signer = CloudArmSigner::new(b"0123456789abcdef0123456789abcdef".to_vec())
        .expect("test arming key is non-empty");
    #[cfg(not(test))]
    let signer = production_cloud_arm_signer()?;
    authorize_body_with_signer(&signer, body, verb, node, target)
}

/// Mint the Music-only asymmetric capability used by the user `mde-musicd`.
/// The root DRM shell keeps the private seed in a dedicated systemd
/// credential; only the resulting signature crosses the shared Bus.
pub(crate) fn authorize_music_mutation_body(body: &str) -> Result<String, String> {
    #[cfg(test)]
    let seed = [7_u8; 32];
    #[cfg(not(test))]
    let seed = production_music_auth_seed()?;
    let mut nonce_bytes = [0_u8; 32];
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = nonce_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .map_err(|_| "The system clock is before the Unix epoch.".to_string())?;
    let node = music_local_hostname();
    music_auth::sign_request(
        body,
        MusicAuthContext {
            verb: "music-workspace",
            node: &node,
            target: "workspace",
        },
        &seed,
        &nonce,
        now.saturating_add(MUSIC_AUTH_TTL_MS),
    )
}

fn music_local_hostname() -> String {
    // Keep the signer byte-compatible with mde-musicd::state::local_host.
    // `$HOSTNAME` can be stale or differently normalized inside a user
    // manager, which would make an otherwise valid Music capability fail
    // closed as `unauthorized`.
    let command = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let file = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_string());
    let environment = std::env::var("HOSTNAME").ok();
    select_music_hostname(command, file, environment)
}

fn select_music_hostname(
    command: Option<String>,
    file: Option<String>,
    environment: Option<String>,
) -> String {
    fn clean(value: String) -> Option<String> {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    }

    command
        .and_then(clean)
        .or_else(|| file.and_then(clean))
        .or_else(|| environment.and_then(clean))
        .unwrap_or_else(|| "unknown-node".to_string())
}

#[cfg(not(test))]
fn production_music_auth_seed() -> Result<[u8; 32], String> {
    if !rustix::process::geteuid().is_root() {
        return Err("Music mutation authorization requires the root DRM shell.".to_string());
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "Music action credential is unavailable.".to_string())?;
    let path = directory.join(music_auth::MUSIC_AUTH_CREDENTIAL_NAME);
    let raw = read_cloud_arm_credential(&path)?;
    let text = std::str::from_utf8(&raw)
        .map_err(|_| "Music action credential is not UTF-8.".to_string())?
        .trim();
    if text.len() > MAX_MUSIC_AUTH_SEED_BYTES || text.len() != 64 {
        return Err("Music action credential must be exactly 64 hex characters.".to_string());
    }
    let mut seed = [0_u8; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        seed[index] = (music_hex(pair[0])? << 4) | music_hex(pair[1])?;
    }
    Ok(seed)
}

/// Return the already-provisioned root-shell Ed25519 seed for another
/// domain-separated shell capability. Clock signs the closed
/// `mcnf-clock-command-v1` transcript, so sharing the host-bound shell seed does
/// not share either Music's wire format or its authorization domain. A missing
/// credential remains a hard, visible refusal at the Clock surface.
pub(crate) fn provisioned_shell_signing_seed() -> Result<[u8; 32], String> {
    #[cfg(test)]
    {
        Ok([7_u8; 32])
    }
    #[cfg(not(test))]
    {
        production_music_auth_seed()
    }
}

#[cfg(not(test))]
fn music_hex(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("Music action credential contains non-hex data.".to_string()),
    }
}

/// How often the folded `state/cloud` mirror is re-read while the surface is in
/// view (a cheap bounded per-topic index probe).
const REFRESH: Duration = Duration::from_secs(15);

/// A cloud mirror is publish-heartbeat based, so three missed heartbeats make
/// its capability and live rows unsafe to present as current. This matches the
/// cloud worker's placement gate without importing the daemon crate into the
/// shell.
pub(super) const CLOUD_MIRROR_STALE_AFTER_MS: i64 = 3 * 60 * 1000;
/// Small forward-clock tolerance for a peer whose wall clock is just ahead of
/// the reader. A far-future stamp is not accepted as fresh.
const CLOUD_MIRROR_FUTURE_SKEW_MS: i64 = 30 * 1000;

/// How long an emitted `action/cloud/*` request waits for its reply before it
/// reads as unanswered. Real OpenTofu plans routinely take tens of seconds, and
/// image/service preparation may take minutes; the former four-second wall
/// discarded valid late replies and replaced their actionable error with a
/// misleading generic timeout. Keep the request correlated long enough for the
/// backend to return its real typed result.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The in-view repaint heartbeat that keeps the poll cadence alive.
const POLL_REPAINT: Duration = Duration::from_secs(1);

/// The most session-audit rows retained (the workspace's own record of the ops
/// it requested this session — the newest are kept).
const MAX_AUDIT: usize = 24;

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn workload_api_now_ms() -> u64 {
    unix_now_ms().try_into().unwrap_or(0)
}

/// Classify a mirror at a supplied clock for deterministic tests and for every
/// UI gate that must agree on freshness. Missing, zero, stale, or implausibly
/// future publication stamps fail closed.
pub(super) fn cloud_state_is_fresh_at(state: &CloudState, now_ms: i64) -> bool {
    let published = state.published_at_ms;
    published > 0
        && published <= now_ms.saturating_add(CLOUD_MIRROR_FUTURE_SKEW_MS)
        && now_ms.saturating_sub(published) <= CLOUD_MIRROR_STALE_AFTER_MS
}

/// Whether a cloud mirror is safe to treat as current at the live wall clock.
pub(super) fn cloud_state_is_fresh(state: &CloudState) -> bool {
    cloud_state_is_fresh_at(state, unix_now_ms())
}

fn browser_vm_running_status(status: &str) -> bool {
    matches!(status.trim(), "active" | "running")
}

fn compare_browser_vm_candidates(
    a_state: &CloudState,
    a: &WorkloadRow,
    b_state: &CloudState,
    b: &WorkloadRow,
    local_peer: &str,
    now_ms: i64,
) -> Ordering {
    let a_fresh = cloud_state_is_fresh_at(a_state, now_ms);
    let b_fresh = cloud_state_is_fresh_at(b_state, now_ms);
    let a_running = browser_vm_running_status(&a.status);
    let b_running = browser_vm_running_status(&b.status);
    let a_ready = a_fresh && a.reachable && a_running;
    let b_ready = b_fresh && b.reachable && b_running;
    let local_peer = local_peer.trim();
    let a_local = a.node.trim().eq_ignore_ascii_case(local_peer);
    let b_local = b.node.trim().eq_ignore_ascii_case(local_peer);

    // `max_by` consumes this ordering. Service readiness is authoritative;
    // locality breaks ties among equally usable (or equally unavailable)
    // duplicates. The remaining health ranks and reversed lexical comparisons
    // make the diagnostic fallback independent of mirror/row iteration order.
    a_ready
        .cmp(&b_ready)
        .then_with(|| a_local.cmp(&b_local))
        .then_with(|| a_fresh.cmp(&b_fresh))
        .then_with(|| a.reachable.cmp(&b.reachable))
        .then_with(|| a_running.cmp(&b_running))
        .then_with(|| b.node.cmp(&a.node))
        .then_with(|| b_state.host.cmp(&a_state.host))
        .then_with(|| b.status.cmp(&a.status))
}

/// Serialize the worker's `set-desired` envelope. The worker accepts one `spec`
/// only under this wrapper; publishing the [`WorkloadSpec`] at the JSON root is
/// malformed even though the nested spec itself has a placement node.
fn set_desired_request_body(spec: &WorkloadSpec) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": spec.node,
        "spec": spec,
    })
    .to_string()
}

/// Serialize a node-placed cloud request with no verb-specific fields.
fn node_request_body(node: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
    })
    .to_string()
}

/// Serialize the dedicated Cuttlefish desired-state request. The Android worker
/// supplies the minimum nested-KVM sizing and persists the resulting
/// `DeliveryType::AndroidVm` spec; a blank name intentionally lets it derive the
/// stable `android-<node>` default.
fn android_provision_request_body(node: &str, name: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node.trim(),
        "name": name.trim(),
    })
    .to_string()
}

/// Serialize an Ansible mutation for one explicitly selected placement node.
fn configure_request_body(node: &str, playbook: &str, group: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
        "playbook": playbook.trim(),
        "group": group.trim(),
    })
    .to_string()
}

/// Serialize a workload lifecycle request for the node that reported its row.
/// Destructive calls carry the operator's already-validated typed echo so the
/// daemon independently enforces the same target confirmation.
fn lifecycle_request_body(node: &str, instance: &str, typed_name: Option<&str>) -> String {
    let mut body = serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
        "instance": instance,
    });
    if let Some(typed_name) = typed_name {
        body["typed_name"] = serde_json::Value::String(typed_name.to_string());
    }
    body.to_string()
}

/// Match the action worker's bounded Android identity vocabulary before the
/// review sheet opens. Workload identities may include `:` because that is part
/// of the shared Android guest contract; placement hosts are path-safe segments
/// with no colon.
fn is_safe_android_action_identity(value: &str, allow_colon: bool) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_')
                || (allow_colon && character == ':')
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AndroidLifecycleRequest {
    schema_version: u16,
    node: String,
    workload_id: String,
    request_id: String,
    expected_generation: u64,
    operation: android_governed::LifecycleOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    app: Option<AospStarterApp>,
}

impl AndroidLifecycleRequest {
    fn body(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("could not encode Android lifecycle request: {error}"))
    }
}

// ───────────────────────────── the delivery-type axis ───────────────────────

/// Which delivery-type view is showing — the cockpit's primary organizing axis
/// (delivery type × placement). Mirrors [`DeliveryType`] on the UI side, adding
/// the nav label + Mackes-Carbon glyph each view tab wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DeliveryView {
    /// A full VM desktop delivered as a native VDI seat.
    #[default]
    DesktopVm,
    /// A headless VM running a service exposed on the mesh.
    ServiceVm,
    /// A VM whose individual apps are forwarded into the MDE desktop.
    AppVm,
    /// A VM providing Android via the two-layer Cuttlefish backend.
    AndroidVm,
    /// A Podman / Quadlet service container.
    ServiceContainer,
}

impl DeliveryView {
    /// Every delivery view, in tab order.
    pub(super) const ALL: [Self; 5] = [
        Self::DesktopVm,
        Self::ServiceVm,
        Self::AppVm,
        Self::AndroidVm,
        Self::ServiceContainer,
    ];

    /// The wire delivery type this view renders — the key the roster filters the
    /// mirror's `workloads` on.
    pub(super) const fn delivery_type(self) -> DeliveryType {
        match self {
            Self::DesktopVm => DeliveryType::DesktopVm,
            Self::ServiceVm => DeliveryType::ServiceVm,
            Self::AppVm => DeliveryType::AppVm,
            Self::AndroidVm => DeliveryType::AndroidVm,
            Self::ServiceContainer => DeliveryType::ServiceContainer,
        }
    }

    /// UI view for a wire delivery type.
    pub(super) const fn from_delivery_type(delivery_type: DeliveryType) -> Self {
        match delivery_type {
            DeliveryType::DesktopVm => Self::DesktopVm,
            DeliveryType::ServiceVm => Self::ServiceVm,
            DeliveryType::AppVm => Self::AppVm,
            DeliveryType::AndroidVm => Self::AndroidVm,
            DeliveryType::ServiceContainer => Self::ServiceContainer,
        }
    }

    /// The delivery-view tab label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::DesktopVm => "Desktop VM",
            Self::ServiceVm => "Service VM",
            Self::AppVm => "App VM",
            Self::AndroidVm => "Android VM",
            Self::ServiceContainer => "Container",
        }
    }

    /// The Mackes-Carbon glyph this view's tab wears (§4 — a registered symbolic
    /// icon, never a text glyph).
    pub(super) const fn icon(self) -> &'static str {
        match self {
            Self::DesktopVm => "view-grid",
            Self::ServiceVm => "globe",
            Self::AppVm => "overlay",
            Self::AndroidVm => "system-lock-screen",
            Self::ServiceContainer => "text-x-generic",
        }
    }
}

// ───────────────────────────── the route axis ───────────────────────────────

/// Which lifecycle route the main detail pane shows. Delivery type is no longer
/// a route axis; [`DeliveryView`] now behaves as a filter / draft type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum WorkloadsRoute {
    /// Author + place a new workload (U14 placement · U15 form).
    #[default]
    Provision,
    /// Dry-run plans and resource review.
    Plan,
    /// Live runs and day-2 workload actions.
    Run,
    /// Desired-vs-actual drift.
    Drift,
    /// Local session audit.
    Audit,
    /// Golden per-type image roster (U19).
    Images,
    /// Podman / Quadlet containers (U19).
    Containers,
}

impl WorkloadsRoute {
    /// Every lifecycle route, in sidebar order.
    pub(super) const ALL: [Self; 7] = [
        Self::Provision,
        Self::Plan,
        Self::Run,
        Self::Drift,
        Self::Audit,
        Self::Images,
        Self::Containers,
    ];

    /// The route label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Provision => "Provision",
            Self::Plan => "Plan results",
            Self::Run => "Run",
            Self::Drift => "Drift",
            Self::Audit => "Audit",
            Self::Images => "Images",
            Self::Containers => "Containers",
        }
    }

    /// The Mackes-Carbon glyph this route's sidebar row wears.
    pub(super) const fn icon(self) -> &'static str {
        match self {
            Self::Provision => "list-add",
            Self::Plan => "document-edit",
            Self::Run => "go-next",
            Self::Drift => "view-refresh",
            Self::Audit => "emblem-ok",
            Self::Images => "camera-photo",
            Self::Containers => "overlay",
        }
    }

    /// Short route context for the detail header.
    const fn blurb(self) -> &'static str {
        match self {
            Self::Provision => "Author desired workloads with explicit placement and review.",
            Self::Plan => "Inspect resources and run dry-run planning before mutation.",
            Self::Run => "Execute live workload and configuration actions through review.",
            Self::Drift => "Compare desired state against reported runtime reality.",
            Self::Audit => "Review this session's requested operations and outcomes.",
            Self::Images => "Build, list, and promote golden bootc images.",
            Self::Containers => "Render and deploy Podman Quadlet service containers.",
        }
    }
}

/// Density mode for the resource tables and side rails. Construct Ops defaults
/// to compact, while comfortable keeps a little more room for tablet use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DensityMode {
    #[default]
    Compact,
    Comfortable,
}

impl DensityMode {
    pub(super) const ALL: [Self; 2] = [Self::Compact, Self::Comfortable];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
        }
    }

    pub(super) const fn row_height(self) -> f32 {
        match self {
            Self::Compact => 30.0,
            Self::Comfortable => 42.0,
        }
    }
}

/// Sortable columns in the lifecycle routes' dense resource tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum WorkloadSortColumn {
    /// Workload name.
    #[default]
    Name,
    /// Placement node.
    Node,
    /// Live state word.
    Status,
    /// CPU utilization.
    Cpu,
    /// Memory usage.
    Memory,
    /// Disk allocation.
    Disk,
    /// Drift state.
    Drift,
}

impl WorkloadSortColumn {
    /// Human-facing header label.
    const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Node => "Node",
            Self::Status => "Status",
            Self::Cpu => "CPU",
            Self::Memory => "Mem",
            Self::Disk => "Disk",
            Self::Drift => "Drift",
        }
    }

    /// Every sortable column, in table order.
    const ALL: [Self; 7] = [
        Self::Name,
        Self::Node,
        Self::Status,
        Self::Cpu,
        Self::Memory,
        Self::Disk,
        Self::Drift,
    ];
}

/// Current Plan resource-table sort state. `descending == false` is ascending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct WorkloadSort {
    /// Active column.
    column: WorkloadSortColumn,
    /// Direction flag.
    descending: bool,
}

impl WorkloadSort {
    /// Toggle a column: same column flips direction; a new column starts ascending.
    fn toggled(self, column: WorkloadSortColumn) -> Self {
        if self.column == column {
            Self {
                column,
                descending: !self.descending,
            }
        } else {
            Self {
                column,
                descending: false,
            }
        }
    }

    /// Header adornment for the active sort column.
    fn marker(self, column: WorkloadSortColumn) -> &'static str {
        if self.column != column {
            ""
        } else if self.descending {
            " ↓"
        } else {
            " ↑"
        }
    }
}

/// Which lifecycle route owns a resource table render. Keeping this separate
/// from [`WorkloadsRoute`] lets the table expose route-specific action policy
/// while sharing the same dense, sortable rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceTableMode {
    /// Plan review: inspect sorted resources before a dry-run/apply.
    Plan,
    /// Run review: live day-2 controls for the filtered resources.
    Run,
    /// Drift review: desired-vs-actual signals and reconciliation dry-runs.
    Drift,
    /// Container review: existing Quadlet service containers plus day-2 actions.
    Containers,
}

impl ResourceTableMode {
    const fn heading(self) -> &'static str {
        match self {
            Self::Plan => "Plan resource table",
            Self::Run => "Run resource table",
            Self::Drift => "Drift resource table",
            Self::Containers => "Container resource table",
        }
    }

    const fn summary(self) -> &'static str {
        match self {
            Self::Plan => {
                "Inspect filtered resources before publishing a dry-run plan or opening a review sheet."
            }
            Self::Run => {
                "Operate filtered resources directly from dense rows; every live action opens review before publish."
            }
            Self::Drift => {
                "Scan desired-vs-actual state first; row actions request node-scoped plans, never a blind apply."
            }
            Self::Containers => {
                "Review existing service containers before deploying a new Quadlet unit."
            }
        }
    }

    const fn action_header(self) -> &'static str {
        match self {
            Self::Plan => "Plan Actions",
            Self::Run => "Run Actions",
            Self::Drift => "Drift Actions",
            Self::Containers => "Container Actions",
        }
    }

    const fn empty_title(self) -> &'static str {
        match self {
            Self::Plan => "No plan rows for this filter",
            Self::Run => "No run targets for this filter",
            Self::Drift => "No drift rows for this filter",
            Self::Containers => "No service containers in the mirror",
        }
    }

    const fn preview_word(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Run => "Run",
            Self::Drift => "Drift",
            Self::Containers => "Containers",
        }
    }
}

// ─────────────────────────────── the Bus reply ──────────────────────────────

/// The shell-side mirror of the worker's `CloudReply` for an `action/cloud/*`
/// mutation (§6 — the shell reads the JSON boundary without depending on the
/// daemon crate). Only the fields this workspace folds are named; the honest
/// tri-state is `ok` (applied) / `gated` (staged, nothing applied) / `error`
/// (failed).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CloudReply {
    /// `true` when a live mutation was performed; `false` on stage/failure.
    ok: bool,
    /// The verb this reply answers (echoed for the client's dispatch).
    verb: String,
    /// An honest gate reason — for a mutation this carries the staged
    /// `tofu plan` / `--check` summary (nothing was applied).
    gated: Option<String>,
    /// A rejection or a backend seam failure.
    error: Option<String>,
    /// Whether a destructive op (destroy / delete / reboot) was performed +
    /// audited on the events plane.
    audited: bool,
    /// Optional typed action-worker detail. Cloud replies may omit this; it is
    /// used to keep non-cloud execution replies visible without inventing a
    /// cloud verb name.
    detail: Option<String>,
    /// Optional bounded typed lifecycle receipt. Android lifecycle replies use
    /// this to project exact generation/phase without reading daemon files.
    raw_log: Option<String>,
}

/// One in-flight `action/cloud/*` request awaiting its `reply/<ulid>`.
#[derive(Debug, Clone)]
struct Pending {
    /// The request ULID — the correlation key its reply rides.
    ulid: String,
    /// The verb being awaited, used by the guided flow and visible status.
    verb: String,
    /// When the request was published (drives [`REQUEST_TIMEOUT`]).
    sent: Instant,
}

/// Progress through the shared guided provisioning flow. This is deliberately
/// delivery-neutral: every workload follows desired state → plan → typed row operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(super) enum ProvisionProgress {
    /// The current draft has not been saved as desired state.
    #[default]
    Draft,
    /// Desired state was accepted by the backend.
    DesiredSaved,
    /// The backend returned a successful dry-run plan.
    Planned,
}

// ───────────────────────────── mutation review ──────────────────────────────

/// What a confirmed review-sheet echo releases onto the Bus (RUN-006 idiom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArmAction {
    /// A live `configure` (Ansible apply) — echo [`APPLY_ECHO`].
    Configure,
    /// A destructive per-workload lifecycle op (`instance-reboot` /
    /// `instance-delete`) — echo the workload name.
    Lifecycle {
        /// The lifecycle verb.
        verb: &'static str,
        /// The placement node that reported the workload row.
        node: String,
        /// The target workload/instance id.
        instance_id: String,
        /// The workload's display name — the required echo.
        name: String,
    },
    /// A route-specific live mutation whose complete body is frozen before the
    /// simple Yes/No review opens (for example, image build or promote).
    Prepared {
        verb: &'static str,
        node: String,
        target: String,
        body: String,
        label: String,
        echo: String,
        word: &'static str,
        subject: String,
    },
    /// A typed VM/container operation sent only to `action/workload/operation`.
    Workload {
        /// The already authorized, immutable operation request.
        request: WorkloadOperationRequest,
        /// Human-readable pending/audit label.
        label: String,
        /// Confirm button echo (kept short; the review is Yes/No).
        echo: String,
        /// Confirm button word.
        word: &'static str,
        /// Review subject.
        subject: String,
    },
    /// A typed Android starter-app launch through the audited action worker.
    /// The body is frozen before the Yes/No review and receives the same
    /// short-lived, request-bound capability as every other privileged action.
    ExecPrepared {
        /// The placement node owning the Android guest.
        node: String,
        /// The capability target, including node/workload/package identity.
        target: String,
        /// Frozen typed action-worker JSON body.
        body: String,
        /// Human-readable pending/audit label.
        label: String,
        /// Confirm button echo (used by the generic review sheet).
        echo: String,
        /// Confirm button word.
        word: &'static str,
        /// Review subject.
        subject: String,
    },
    /// Exact generation-checked Android lifecycle request. The request carries
    /// only closed operation/app enums and bounded identities.
    AndroidLifecycle {
        request: AndroidLifecycleRequest,
        target: String,
        label: String,
        echo: String,
        word: &'static str,
        subject: String,
        approval: String,
    },
}

impl ArmAction {
    /// The `action/cloud/*` verb this action publishes (test seam — the perform
    /// path matches the variant directly).
    const fn verb(&self) -> &'static str {
        match self {
            Self::Configure => "configure",
            Self::Lifecycle { verb, .. } => verb,
            Self::Prepared { verb, .. } => verb,
            Self::Workload { .. } => "workload-operation",
            Self::ExecPrepared { .. } => EXEC_AUTH_VERB,
            Self::AndroidLifecycle { .. } => VERB_ANDROID_LIFECYCLE,
        }
    }

    /// The exact echo the operator must type before this action publishes.
    fn echo(&self) -> String {
        match self {
            Self::Configure => APPLY_ECHO.to_string(),
            Self::Lifecycle { name, .. } => name.clone(),
            Self::Prepared { echo, .. } => echo.clone(),
            Self::Workload { echo, .. } => echo.clone(),
            Self::ExecPrepared { echo, .. } => echo.clone(),
            Self::AndroidLifecycle { echo, .. } => echo.clone(),
        }
    }

    /// The confirm button's verb word.
    fn confirm_word(&self) -> &'static str {
        match self {
            Self::Configure => "Apply",
            Self::Lifecycle { verb, .. } => verb_label(verb),
            Self::Prepared { word, .. } => word,
            Self::Workload { word, .. } => word,
            Self::ExecPrepared { word, .. } => word,
            Self::AndroidLifecycle { word, .. } => word,
        }
    }

    /// What the confirm acts on — the review copy's subject.
    fn subject(&self) -> String {
        match self {
            Self::Configure => "the Ansible convergence (live apply)".to_string(),
            Self::Lifecycle { name, .. } => format!("workload {name}"),
            Self::Prepared { subject, .. } => subject.clone(),
            Self::Workload { subject, .. } => subject.clone(),
            Self::ExecPrepared { subject, .. } => subject.clone(),
            Self::AndroidLifecycle { subject, .. } => subject.clone(),
        }
    }

    /// Prepared service/image actions use the low-friction explicit-choice
    /// review. Live infrastructure applies and lifecycle mutations retain the
    /// stronger typed-echo interlock.
    const fn uses_yes_no_review(&self) -> bool {
        matches!(
            self,
            Self::Prepared { .. }
                | Self::Workload { .. }
                | Self::ExecPrepared { .. }
                | Self::AndroidLifecycle { .. }
        )
    }
}

/// A pending mutation review sheet — the action it releases plus any typed echo
/// entered for a strong-interlock action. Nothing reaches the Bus until the
/// operator confirms through the review's action-appropriate gate.
#[derive(Debug, Clone)]
pub(super) struct ReviewSheetState {
    /// What confirming publishes.
    pub(super) action: ArmAction,
    /// The operator's typed echo.
    pub(super) typed: String,
}

/// The review gate (RUN-006): the operator's echo must equal the required echo
/// byte-for-byte before the mutation may publish. The one decision the confirm
/// button + the tests share, so "unconfirmed ⇒ blocked" is proven without a
/// render.
fn armed(typed: &str, echo: &str) -> bool {
    typed == echo
}

/// The immutable facts the review sheet renders before the exact echo can
/// release an action. This is deliberately derived from the pending
/// [`ArmAction`], not live form controls, so prepared/lifecycle mutations show
/// the same command, target, placement, and request digest that will be bound to
/// the capability token.
#[derive(Debug, Clone)]
struct ReviewSheetFacts {
    command: String,
    subject: String,
    target: String,
    node: String,
    body_digest: String,
    body_summary: String,
    body_preview: String,
    impact: String,
}

fn review_sheet_facts(action: &ArmAction, state: &WorkloadsState) -> ReviewSheetFacts {
    let verb = action.verb();
    let command = if matches!(action, ArmAction::ExecPrepared { .. }) {
        EXEC_ACTION_TOPIC.to_string()
    } else if matches!(action, ArmAction::Workload { .. }) {
        mackes_mesh_types::workloads::WORKLOAD_OPERATION_TOPIC.to_string()
    } else {
        format!("action/cloud/{verb}")
    };
    let subject = action.subject();
    match action {
        ArmAction::Configure => {
            let node = state
                .selected_node()
                .map(str::trim)
                .filter(|node| !node.is_empty())
                .unwrap_or("no placement selected");
            let body =
                configure_request_body(node, &state.configure.playbook, &state.configure.group);
            request_review_facts(
                command,
                subject,
                CLOUD_ARM_NODE_SCOPE.to_string(),
                node.to_string(),
                &body,
                format!(
                    "Live Ansible convergence can change workloads on placement node {node}; \
                     authorization is scoped to {CLOUD_ARM_NODE_SCOPE}."
                ),
            )
        }
        ArmAction::Lifecycle {
            verb,
            node,
            instance_id,
            name,
        } => {
            let body = lifecycle_request_body(node, instance_id, Some(name));
            request_review_facts(
                command,
                subject,
                instance_id.clone(),
                node.clone(),
                &body,
                format!(
                    "{} affects one workload: {name} ({instance_id}) on placement node {node}. \
                     No other node or workload is authorized by this review.",
                    verb_label(verb)
                ),
            )
        }
        ArmAction::Prepared {
            node,
            target,
            body,
            label,
            ..
        } => request_review_facts(
            command,
            subject,
            target.clone(),
            node.clone(),
            body,
            format!(
                "{label} affects target {target} on placement node {node}; authorization is \
                 bound to this frozen request body digest."
            ),
        ),
        ArmAction::Workload { request, .. } => {
            let body = serde_json::to_string(request).unwrap_or_else(|_| "{}".to_string());
            request_review_facts(
                command,
                subject,
                format!("workload:{}", request.workload_id.as_str()),
                request.target_node.clone(),
                &body,
                "The typed Workload reconciler owns admission, lifecycle, and readiness; no legacy VM/container topic is used.".to_string(),
            )
        }
        ArmAction::ExecPrepared {
            node,
            target,
            body,
            label,
            ..
        } => request_review_facts(
            command,
            subject,
            target.clone(),
            node.clone(),
            body,
            format!(
                "{label} targets only the admitted Android workload on {node}; the action \
                 worker reconstructs the canonical MAIN + LAUNCHER intent and records the \
                 outcome in its audit lane."
            ),
        ),
        ArmAction::AndroidLifecycle {
            request,
            target,
            approval,
            ..
        } => {
            let body = request.body().unwrap_or_else(|_| "{}".to_owned());
            request_review_facts(
                command,
                subject,
                target.clone(),
                request.node.clone(),
                &body,
                format!(
                    "{approval}. The capability is bound to workload {} generation {} on {}.",
                    request.workload_id, request.expected_generation, request.node
                ),
            )
        }
    }
}

fn request_review_facts(
    command: String,
    subject: String,
    target: String,
    node: String,
    body: &str,
    impact: String,
) -> ReviewSheetFacts {
    ReviewSheetFacts {
        command,
        subject,
        target,
        node,
        body_digest: cloud_request_digest(body)
            .map(|digest| format!("sha256:{digest}"))
            .unwrap_or_else(|error| format!("unavailable: {error}")),
        body_summary: request_body_summary(body),
        body_preview: request_body_preview(body),
        impact,
    }
}

fn request_body_summary(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => {
            let mut keys = map.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            let shown = keys.iter().take(8).copied().collect::<Vec<_>>().join(", ");
            let suffix = if keys.len() > 8 { ", …" } else { "" };
            format!(
                "{} bytes · {} JSON fields: {shown}{suffix}",
                body.len(),
                map.len()
            )
        }
        Ok(serde_json::Value::Array(items)) => {
            format!(
                "{} bytes · JSON array with {} items",
                body.len(),
                items.len()
            )
        }
        Ok(_) => format!("{} bytes · JSON scalar", body.len()),
        Err(_) => format!("{} bytes · invalid JSON body", body.len()),
    }
}

fn request_body_preview(body: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 220;
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_PREVIEW_CHARS {
        compact
    } else {
        let preview = compact.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
        format!("{preview}…")
    }
}

// ─────────────────────────────── the audit trail ────────────────────────────

/// The honest outcome class of a performed op — the session audit row's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditOutcome {
    /// The op was applied live (and, if destructive, audited to the events plane).
    Applied,
    /// The backend persisted desired state, but a separate typed row operation remains.
    Desired,
    /// The op was staged (a `tofu plan` / `--check` dry-run — nothing applied).
    Staged,
    /// The op failed.
    Failed,
}

impl AuditOutcome {
    /// The Style token this verdict paints in (§4).
    const fn color(self) -> Color32 {
        match self {
            Self::Applied => Style::OK,
            Self::Desired => Style::ACCENT_WORKLOADS,
            Self::Staged => Style::WARN,
            Self::Failed => Style::DANGER,
        }
    }

    /// The verdict word.
    const fn word(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Desired => "desired saved",
            Self::Staged => "staged",
            Self::Failed => "failed",
        }
    }
}

/// One row of the session audit trail — the workspace's own honest record of an
/// op it requested (verb · verdict · detail). Distinct from the daemon's durable
/// hash-chained events log; this is the local "what did I do here" list.
#[derive(Debug, Clone)]
pub(super) struct AuditEntry {
    /// The verb performed (`provision` / `instance-delete` / …).
    verb: String,
    /// The honest verdict.
    outcome: AuditOutcome,
    /// A short detail (the staged plan summary / the failure / "audited").
    detail: String,
}

// ─────────────────────────────── the surface state ──────────────────────────

/// The **Workloads** lifecycle state — the folded `state/cloud` mirror, the active
/// delivery filter + route, the selected placement node, each route's own sub-state,
/// the review-sheet confirm, the one in-flight mutation, and the session audit
/// trail. A plain field on the shell's struct, borrowed `&mut` while the surface
/// is in view. Every panel owns its `State` in its own file, so a downstream
/// worker adds route-specific state without touching this struct.
///
/// `#[derive(Debug)]` deliberately: it keeps every sub-panel `State` field a live
/// read (the panel workers fill them incrementally), so the seam compiles clean
/// as the panels land one by one.
#[derive(Debug)]
pub struct WorkloadsState {
    // ── preserved backend wiring (do not repurpose) ──
    /// The Bus persist root (the client data dir). `None` when the Bus is
    /// unavailable — an honest off-mesh degrade (§7), never a crash.
    bus_root: Option<PathBuf>,
    /// The per-node status mirrors, folded across every `state/cloud/<node>`
    /// topic (host-sorted). Empty when nothing has published yet — an honest
    /// pre-mirror state, never fabricated.
    states: Vec<CloudState>,
    /// Admitted signed Android catalogs read during [`Self::poll`], never from
    /// the render path.
    android_catalogs: Vec<android_governed::CatalogSnapshot>,
    /// Root-owned daemon cache digests keyed by placement node, captured only
    /// during polling and matched against the writable Bus projection.
    android_admitted_catalog_digests: BTreeMap<String, String>,
    /// Cache directory seam; production uses the mackesd default while tests
    /// can exercise admission against isolated files.
    android_catalog_cache_root: PathBuf,
    /// Last typed lifecycle receipt per workload, learned from bounded replies.
    android_lifecycle_receipts: BTreeMap<String, android_governed::LifecycleReceipt>,
    /// Exact Android lifecycle operation currently awaiting a cloud reply.
    android_lifecycle_pending: Option<android_governed::PendingLifecycle>,
    /// Wall-clock snapshot captured by [`Self::poll`] for pure render models.
    android_projection_now_ms: u64,
    /// Monotonic in-process request suffix; avoids clock/RNG access in render.
    android_request_sequence: u64,
    /// When the mirror was last folded (the refresh cadence anchor).
    loaded_at: Option<Instant>,
    /// A manual refresh is queued — re-reads the mirror on the next poll.
    forced: bool,
    /// A pending review-sheet confirm for a destructive intent, if any.
    arming: Option<ReviewSheetState>,
    /// The one in-flight mutation — its reply resolves into the note + audit.
    mutation_pending: Option<Pending>,
    /// A transient one-line action note — honest feedback, never a silent op.
    note: Option<String>,
    /// A typed Workload operation awaiting its node-local projection. Workload
    /// actions do not produce cloud `reply/<ulid>` messages.
    workload_pending: Option<(String, String, Instant)>,
    /// The session audit trail (newest last), capped at [`MAX_AUDIT`].
    audit: Vec<AuditEntry>,
    /// Test-only signer injection. Production has no programmatic key seam and
    /// must obtain the credential from systemd in a root process.
    #[cfg(test)]
    arm_key_override: Option<Vec<u8>>,

    // ── the lifecycle nav ──
    /// The active delivery-type filter for resource/provision views.
    view: DeliveryView,
    /// The active lifecycle route.
    route: WorkloadsRoute,
    /// Table/sidebar density.
    density: DensityMode,
    /// Plan route resource-table sort state.
    resource_sort: WorkloadSort,
    /// The expanded Plan route resource row, keyed by delivery type + node + name.
    expanded_resource: Option<String>,
    /// The placement node the provision panel targets (from the placement
    /// picker); `None` until one is chosen.
    selected_node: Option<String>,
    /// Current guided-flow milestone for the exact draft identified below.
    provision_progress: ProvisionProgress,
    /// Stable key for delivery + placement + form values. Any edit resets the
    /// guided flow so a stale plan can never authorize a changed draft.
    provision_draft_key: Option<String>,

    // ── the per-panel sub-state (each panel worker owns its own file) ──
    //
    // `allow(dead_code)`: these are the fan-out seam — each panel worker (U14–U19)
    // reads + fills its own `State` in its own file. They are honestly unread
    // until then; the allow drops off the moment a worker consumes the field.
    // (`configure` is already consumed by `configure_body` + the Run route.)
    /// U14 — placement picker state.
    #[allow(dead_code)]
    placement: placement::State,
    /// U15 — provision form state.
    #[allow(dead_code)]
    form: provision_form::State,
    /// U17 — configure + inventory state (holds the Ansible playbook/group).
    configure: configure::State,
    /// U18 — status + metrics state.
    #[allow(dead_code)]
    status: status::State,
    /// U19 — images panel state.
    #[allow(dead_code)]
    images: images::State,
    /// U19 — containers panel state.
    #[allow(dead_code)]
    containers: containers::State,
}

impl Default for WorkloadsState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            states: Vec::new(),
            android_catalogs: Vec::new(),
            android_admitted_catalog_digests: BTreeMap::new(),
            android_catalog_cache_root: PathBuf::from(ANDROID_CATALOG_CACHE_ROOT),
            android_lifecycle_receipts: BTreeMap::new(),
            android_lifecycle_pending: None,
            android_projection_now_ms: 0,
            android_request_sequence: 0,
            loaded_at: None,
            forced: false,
            arming: None,
            mutation_pending: None,
            note: None,
            workload_pending: None,
            audit: Vec::new(),
            #[cfg(test)]
            arm_key_override: None,
            view: DeliveryView::default(),
            route: WorkloadsRoute::default(),
            density: DensityMode::default(),
            resource_sort: WorkloadSort::default(),
            expanded_resource: None,
            selected_node: None,
            provision_progress: ProvisionProgress::Draft,
            provision_draft_key: None,
            placement: placement::State::default(),
            form: provision_form::State::default(),
            configure: configure::State::default(),
            status: status::State::default(),
            images: images::State::default(),
            containers: containers::State::default(),
        }
    }
}

/// The external seam name main.rs (and any other caller) binds to — an alias so
/// the rename to [`WorkloadsState`] stays source-compatible.
pub(super) type InfraCodeState = WorkloadsState;

impl WorkloadsState {
    /// Poll the Bus on the shared cadence + keep the repaint heartbeat alive —
    /// the shell calls this each frame while the surface is in view. Resolves any
    /// in-flight mutation reply, then re-folds the `state/cloud` mirror when due
    /// (the refresh cadence or a queued refresh). No blocking await — the mirror
    /// is a cheap local read and the reply is read off the Bus on a later tick.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.android_projection_now_ms = workload_api_now_ms();
        self.resolve_mutation();
        self.resolve_workload_operation();

        let due = self
            .loaded_at
            .is_none_or(|t| now.duration_since(t) >= REFRESH);
        if self.forced || due {
            self.states = self.read_states();
            let catalogs = self.read_android_catalogs();
            self.android_admitted_catalog_digests = self.read_android_catalog_digests(&catalogs);
            self.android_catalogs = catalogs;
            self.loaded_at = Some(now);
            self.forced = false;
        }

        ctx.request_repaint_after(POLL_REPAINT);
    }

    /// Resolve the one in-flight mutation's reply into the note + an audit row
    /// (never a silent op, §7); on a successful mutation, re-fold the mirror so
    /// the change reflects. A no-responder is an honest timeout.
    fn resolve_mutation(&mut self) {
        let Some((ulid, sent, pending_verb)) = self
            .mutation_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent, p.verb.clone()))
        else {
            return;
        };
        if let Some(reply) = self.read_reply(&ulid) {
            if pending_verb == VERB_ANDROID_LIFECYCLE {
                if let Some(receipt) = reply
                    .raw_log
                    .as_deref()
                    .and_then(android_governed::LifecycleReceipt::parse)
                {
                    self.android_lifecycle_receipts
                        .insert(receipt.workload_id.clone(), receipt);
                }
                self.android_lifecycle_pending = None;
            }
            let (note, entry) = fold_mutation(&reply);
            self.record_audit(entry);
            if reply.ok {
                self.forced = true;
                match reply.verb.as_str() {
                    VERB_ANDROID_PROVISION | VERB_SET_DESIRED => {
                        self.provision_progress = ProvisionProgress::DesiredSaved;
                    }
                    VERB_PLAN => self.provision_progress = ProvisionProgress::Planned,
                    _ => {}
                }
            }
            self.note = Some(note);
            self.mutation_pending = None;
        } else if sent.elapsed() >= REQUEST_TIMEOUT {
            self.note = Some(format!(
                "The cloud backend did not answer `{pending_verb}` within five minutes. The \
                     request may still finish; check Audit before retrying."
            ));
            self.mutation_pending = None;
            if pending_verb == VERB_ANDROID_LIFECYCLE {
                self.android_lifecycle_pending = None;
            }
        }
    }

    /// Resolve a typed Workload operation from the authoritative node snapshot.
    /// A missing snapshot is simply pending; a terminal status becomes one
    /// bounded audit entry and never falls back to the legacy cloud reply lane.
    fn resolve_workload_operation(&mut self) {
        let Some((node, workload_id, _sent)) = self.workload_pending.clone() else {
            return;
        };
        let Some(persist) = self.persist() else {
            return;
        };
        let Some(status) = crate::workload_api::read_status(&persist, &node, &workload_id) else {
            return;
        };
        if !status.phase.is_terminal() {
            return;
        }
        let ok = status.phase == mackes_mesh_types::workloads::WorkloadOperationPhase::Completed;
        let detail = status
            .reason
            .clone()
            .unwrap_or_else(|| format!("workload operation reached {:?}", status.phase));
        self.record_audit(AuditEntry {
            verb: "workload-operation".to_string(),
            outcome: if ok {
                AuditOutcome::Applied
            } else {
                AuditOutcome::Failed
            },
            detail: detail.clone(),
        });
        self.note = Some(if ok {
            format!("Workload {workload_id} completed.")
        } else {
            format!("Workload {workload_id} failed: {detail}")
        });
        self.workload_pending = None;
        self.forced = true;
    }

    /// Fold every `state/cloud/<node>` mirror off the Bus into the host-sorted
    /// roster (all nodes). A missing/unopenable Bus is an honest empty fold
    /// (off-mesh, §7); an undecodable body is skipped, never fabricated.
    fn read_states(&self) -> Vec<CloudState> {
        let Some(persist) = self.persist() else {
            return Vec::new();
        };
        let Ok(topics) = persist.list_topics() else {
            return Vec::new();
        };
        let mut states: Vec<CloudState> = topics
            .into_iter()
            .filter(|t| t.starts_with(CLOUD_STATE_PREFIX))
            .filter_map(|topic| {
                let msg = persist.read_latest(&topic).ok().flatten()?;
                let body = msg.body.as_deref()?;
                serde_json::from_str::<CloudState>(body).ok()
            })
            .collect();
        states.sort_by(|a, b| a.host.cmp(&b.host));
        states
    }

    /// Fold admitted signed Android catalogs during polling. The worker has
    /// already verified Ed25519 trust; the shell additionally refuses malformed,
    /// oversized, duplicate-node, or intrinsically invalid projections.
    fn read_android_catalogs(&self) -> Vec<android_governed::CatalogSnapshot> {
        use mackes_mesh_types::android_apps::ANDROID_CATALOG_STATE_TOPIC_PREFIX;

        let Some(persist) = self.persist() else {
            return Vec::new();
        };
        let Ok(topics) = persist.list_topics() else {
            return Vec::new();
        };
        let mut catalogs = BTreeMap::new();
        for topic in topics
            .into_iter()
            .filter(|topic| topic.starts_with(ANDROID_CATALOG_STATE_TOPIC_PREFIX))
            .take(64)
        {
            let Some(body) = persist
                .read_latest(&topic)
                .ok()
                .flatten()
                .and_then(|message| message.body)
            else {
                continue;
            };
            let Some(snapshot) = android_governed::decode_catalog_snapshot(&topic, &body) else {
                continue;
            };
            catalogs.insert(snapshot.node.clone(), snapshot);
        }
        catalogs.into_values().collect()
    }

    fn read_android_catalog_digests(
        &self,
        catalogs: &[android_governed::CatalogSnapshot],
    ) -> BTreeMap<String, String> {
        catalogs
            .iter()
            .filter_map(|snapshot| {
                let path = self
                    .android_catalog_cache_root
                    .join(format!("{}.json", snapshot.node));
                android_governed::read_admitted_catalog_digest(&path, 0)
                    .map(|digest| (snapshot.node.clone(), digest))
            })
            .collect()
    }

    /// Read the reply on `reply/<ulid>`, if one has landed (oldest wins — the RPC
    /// convention).
    fn read_reply(&self, ulid: &str) -> Option<CloudReply> {
        let persist = self.persist()?;
        let msgs = persist.list_since_limit(&reply_topic(ulid), None, 1).ok()?;
        let body = msgs.first()?.body.as_deref()?;
        serde_json::from_str::<CloudReply>(body).ok()
    }

    /// Read a reply with its complete typed payload. Specialized workflows such
    /// as image-build and configure use fields intentionally omitted from the
    /// compact mutation mirror.
    fn read_wire_reply(&self, ulid: &str) -> Option<WireCloudReply> {
        let persist = self.persist()?;
        let msgs = persist.list_since_limit(&reply_topic(ulid), None, 1).ok()?;
        let body = msgs.first()?.body.as_deref()?;
        serde_json::from_str(body).ok()
    }

    /// Open the Bus persist mirror at the client data dir, if reachable
    /// (fail-soft, through the shared [`BusReader`] seam).
    fn persist(&self) -> Option<Persist> {
        BusReader::new(self.bus_root.clone()).open()
    }

    /// Publish one typed request on an explicit Bus topic, answering a pending
    /// handle or an honest error string (a missing Bus degrades, never panics —
    /// §7). Keeping the topic explicit lets the Android launcher use the
    /// audited `action/exec/request` lane without pretending it is a cloud verb.
    fn publish_topic(
        &self,
        topic: &str,
        pending_verb: &str,
        body: Option<&str>,
    ) -> Result<Pending, String> {
        let persist = self
            .persist()
            .ok_or_else(|| "the local mesh Bus is unavailable".to_string())?;
        publish_request(&persist, &topic, Priority::Default, None, body)
            .map(|ulid| Pending {
                ulid,
                verb: pending_verb.to_string(),
                sent: Instant::now(),
            })
            .map_err(|e| e.to_string())
    }

    /// Publish an `action/cloud/<verb>` request through the shared cloud lane.
    fn publish(&self, verb: &str, body: Option<&str>) -> Result<Pending, String> {
        let topic = format!("{}{verb}", mackes_mesh_types::cloud::CLOUD_ACTION_PREFIX);
        self.publish_topic(&topic, verb, body)
    }

    /// Emit a mutation verb and track its reply — the honest outcome lands in the
    /// note. A newly-issued mutation replaces an unresolved one (its reply is
    /// simply never read).
    fn issue(&mut self, verb: &str, body: Option<&str>, label: &str) {
        match self.publish(verb, body) {
            Ok(pending) => {
                self.mutation_pending = Some(pending);
                self.note = Some(format!("Requested {label}\u{2026}"));
            }
            Err(e) => self.note = Some(format!("Could not request {label}: {e}")),
        }
    }

    /// Publish an already-authorized request on a non-cloud typed action lane.
    fn issue_topic(&mut self, topic: &str, pending_verb: &str, body: &str, label: &str) {
        match self.publish_topic(topic, pending_verb, Some(body)) {
            Ok(pending) => {
                self.mutation_pending = Some(pending);
                self.note = Some(format!("Requested {label}\u{2026}"));
            }
            Err(error) => self.note = Some(format!("Could not request {label}: {error}")),
        }
    }

    /// Resolve the placement picker into a non-blank node for an emitted
    /// request. Forms and menubar actions can be reached before a node is
    /// selected; that state must not become a node-agnostic mutation that every
    /// worker is eligible to drain.
    fn require_selected_node(&mut self, label: &str) -> Option<String> {
        if let Some(node) = self
            .selected_node
            .as_deref()
            .map(str::trim)
            .filter(|node| !node.is_empty())
        {
            return Some(node.to_string());
        }
        self.note = Some(format!(
            "Select a placement node before requesting {label}."
        ));
        None
    }

    /// Emit one declarative desired-state write using the worker's required
    /// `{ node, spec }` envelope.
    pub(super) fn set_desired(&mut self, spec: &WorkloadSpec) {
        if spec.node.trim().is_empty() {
            self.note =
                Some("Select a placement node before setting the workload desired.".to_string());
            return;
        }
        let body = set_desired_request_body(spec);
        self.arm_prepared(
            mackes_mesh_types::cloud::VERB_SET_DESIRED,
            spec.node.trim().to_string(),
            format!("desired:{}", spec.name.trim()),
            body,
            format!("set desired for {}", spec.name),
            spec.name.trim().to_string(),
            "Save",
            format!("desired state for workload {}", spec.name),
        );
    }

    /// Open a simple Yes/No review for the dedicated Cuttlefish Android contract.
    /// `android-provision` persists the correctly sized Android desired slice;
    /// live lifecycle remains a typed operation on the resulting Workload row.
    pub(super) fn arm_android_provision(&mut self, name: &str) {
        let Some(node) = self.require_selected_node("Android provisioning") else {
            return;
        };
        let name = name.trim();
        let target = if name.is_empty() {
            format!("android-{node}")
        } else {
            name.to_string()
        };
        self.arm_prepared(
            VERB_ANDROID_PROVISION,
            node.clone(),
            target.clone(),
            android_provision_request_body(&node, name),
            format!("prepare Cuttlefish Android VM ({target})"),
            target.clone(),
            "Prepare",
            format!("Cuttlefish Android desired state for {target}"),
        );
    }

    /// Open the explicit review for one fresh, launch-ready AOSP starter app.
    /// The frozen body is consumed by the audited action worker, which rebuilds
    /// the canonical guest launch intent from the closed app enum. No ADB,
    /// host command, or direct guest claim is emitted by the shell.
    pub(super) fn arm_android_app_launch(
        &mut self,
        target_host: &str,
        workload_id: &str,
        app: AospStarterApp,
    ) {
        let target_host = target_host.trim();
        let workload_id = workload_id.trim();
        if !is_safe_android_action_identity(target_host, false)
            || !is_safe_android_action_identity(workload_id, true)
        {
            self.note = Some(
                "Could not prepare Android launch: the node or workload identity is not a bounded typed identity. Nothing was sent."
                    .to_string(),
            );
            return;
        }
        let target = format!(
            "android-app:{target_host}:{workload_id}:{}",
            app.package_id().as_str()
        );
        if target.len() > 255 {
            self.note = Some(
                "Could not prepare Android launch: the capability target exceeds its bound. Nothing was sent."
                    .to_string(),
            );
            return;
        }
        let body = serde_json::json!({
            "schema_version": 1,
            "kind": "android_app_launch",
            "target_host": target_host,
            "workload_id": workload_id,
            "app": app,
        })
        .to_string();
        self.note = None;
        self.arming = Some(ReviewSheetState {
            action: ArmAction::ExecPrepared {
                node: target_host.to_owned(),
                target,
                body,
                label: format!("launch {} in {workload_id}", app.display_name()),
                echo: app.display_name().to_owned(),
                word: "Launch",
                subject: format!("{} in Android workload {workload_id}", app.display_name()),
            },
            typed: String::new(),
        });
    }

    /// Open an explicit review for one closed Android lifecycle operation.
    /// Generation and app identity come from the already-projected model; no
    /// package, launcher, command, path, or endpoint string is accepted here.
    fn arm_android_lifecycle(
        &mut self,
        node: &str,
        workload_id: &str,
        operation: android_governed::LifecycleOperation,
        app: Option<AospStarterApp>,
        expected_generation: u64,
        approval: String,
    ) {
        let node = node.trim();
        let workload_id = workload_id.trim();
        if !is_safe_android_action_identity(node, false)
            || !is_safe_android_action_identity(workload_id, false)
        {
            self.note = Some(
                "Could not prepare Android lifecycle: placement or workload identity is invalid. Nothing was sent."
                    .to_owned(),
            );
            return;
        }
        if matches!(
            operation,
            android_governed::LifecycleOperation::Start
                | android_governed::LifecycleOperation::Retry
        ) != app.is_some()
        {
            self.note = Some(
                "Could not prepare Android lifecycle: start/retry requires one signed app and stop/cancel accepts none. Nothing was sent."
                    .to_owned(),
            );
            return;
        }
        self.android_request_sequence = self.android_request_sequence.saturating_add(1);
        let request = AndroidLifecycleRequest {
            schema_version: 1,
            node: node.to_owned(),
            workload_id: workload_id.to_owned(),
            request_id: format!(
                "iac-android-{}-{}-{}",
                operation.label().to_ascii_lowercase(),
                expected_generation,
                self.android_request_sequence
            ),
            expected_generation,
            operation,
            app,
        };
        let app_label = app.map(AospStarterApp::display_name);
        let label = app_label.map_or_else(
            || format!("{} {workload_id}", operation.label().to_ascii_lowercase()),
            |app| {
                format!(
                    "{} {app} in {workload_id}",
                    operation.label().to_ascii_lowercase()
                )
            },
        );
        self.note = None;
        self.arming = Some(ReviewSheetState {
            action: ArmAction::AndroidLifecycle {
                request,
                target: workload_id.to_owned(),
                label: label.clone(),
                echo: operation.label().to_owned(),
                word: operation.label(),
                subject: label,
                approval,
            },
            typed: String::new(),
        });
    }

    /// Record one session-audit row, trimming to [`MAX_AUDIT`] newest.
    fn record_audit(&mut self, entry: AuditEntry) {
        self.audit.push(entry);
        let overflow = self.audit.len().saturating_sub(MAX_AUDIT);
        if overflow > 0 {
            self.audit.drain(0..overflow);
        }
    }

    /// The Run route's configure request body — the picked playbook + target group.
    /// (The worker converges `cloud_vm` on `site.yml`; the selection is honest
    /// operator context the reply echoes.) The inputs live in [`configure::State`]
    /// so the U17 worker owns them without touching this struct.
    fn configure_body(&mut self) -> Option<String> {
        let node = self.require_selected_node("configuration")?;
        Some(configure_request_body(
            &node,
            &self.configure.playbook,
            &self.configure.group,
        ))
    }

    /// Load the mint authority. Production accepts only the root DRM shell with
    /// a systemd credential; a windowed/session launch and a missing credential
    /// fail closed before any live request reaches the Bus.
    fn cloud_arm_signer(&self) -> Result<CloudArmSigner, String> {
        #[cfg(test)]
        if let Some(key) = &self.arm_key_override {
            return CloudArmSigner::new(key.clone()).map_err(str::to_string);
        }

        production_cloud_arm_signer()
    }

    /// Insert a locally minted, short-lived, target-bound token into an already
    /// frozen JSON mutation body. The HMAC key itself never enters the Bus.
    fn authorize_body(
        &self,
        body: &str,
        verb: &str,
        node: &str,
        target: &str,
    ) -> Result<String, String> {
        let signer = self.cloud_arm_signer()?;
        authorize_body_with_signer(&signer, body, verb, node, target)
    }

    /// Perform a confirmed action — called only past the review-sheet gate
    /// ([`armed`]). Android launcher requests use the audited execution lane;
    /// the existing cloud lifecycle path remains unchanged for all other arms.
    fn perform(&mut self, action: ArmAction, typed: &str) {
        match action {
            ArmAction::Workload {
                request,
                label,
                echo,
                ..
            } => {
                if !armed(typed, &echo) {
                    self.note =
                        Some("Workload confirmation did not match; nothing was sent.".to_string());
                    return;
                }
                let Some(root) = self.bus_root.as_deref() else {
                    self.note = Some(
                        "Could not request workload: the local mesh Bus is unavailable."
                            .to_string(),
                    );
                    return;
                };
                match crate::workload_api::publish(root, &request) {
                    Ok(_) => {
                        self.workload_pending = Some((
                            request.target_node.clone(),
                            request.workload_id.as_str().to_string(),
                            Instant::now(),
                        ));
                        self.note = Some(format!(
                            "Requested {label}; waiting for Workload readiness."
                        ));
                    }
                    Err(error) => self.note = Some(format!("Could not request {label}: {error}")),
                }
            }
            ArmAction::ExecPrepared {
                node: _node,
                target,
                body,
                label,
                echo,
                ..
            } => {
                if !armed(typed, &echo) {
                    self.note = Some(
                        "Typed Android launch confirmation did not match; nothing was sent."
                            .to_string(),
                    );
                    return;
                }
                match self.authorize_body(&body, EXEC_AUTH_VERB, EXEC_AUTH_NODE, &target) {
                    Ok(body) => {
                        self.issue_topic(EXEC_ACTION_TOPIC, "android-app-launch", &body, &label)
                    }
                    Err(error) => self.note = Some(format!("{error} Nothing was sent.")),
                }
            }
            ArmAction::AndroidLifecycle {
                request,
                target,
                label,
                echo,
                ..
            } => {
                if !armed(typed, &echo) {
                    self.note = Some(
                        "Android lifecycle confirmation did not match; nothing was sent."
                            .to_owned(),
                    );
                    return;
                }
                let body = match request.body() {
                    Ok(body) => body,
                    Err(error) => {
                        self.note = Some(format!("{error} Nothing was sent."));
                        return;
                    }
                };
                match self.authorize_body(&body, VERB_ANDROID_LIFECYCLE, &request.node, &target) {
                    Ok(body) => {
                        self.android_lifecycle_pending = Some(android_governed::PendingLifecycle {
                            workload_id: request.workload_id.clone(),
                            operation: request.operation,
                            generation: request.expected_generation.saturating_add(1),
                            app: request.app,
                        });
                        self.issue(VERB_ANDROID_LIFECYCLE, Some(&body), &label);
                        if self.mutation_pending.is_none() {
                            self.android_lifecycle_pending = None;
                        }
                    }
                    Err(error) => self.note = Some(format!("{error} Nothing was sent.")),
                }
            }
            action => self.perform_cloud(action, typed),
        }
    }

    /// Perform a confirmed `action/cloud/*` mutation. Kept separate from the
    /// execution-lane wrapper so each request's topic and capability authority
    /// stay explicit at the call site.
    fn perform_cloud(&mut self, action: ArmAction, typed: &str) {
        let expected = action.echo();
        if !armed(typed, &expected) {
            self.note = Some("Typed confirmation did not match; nothing was sent.".to_string());
            return;
        }
        let requires_apply_capability = matches!(&action, ArmAction::Configure);
        let (verb, node, target, body, label) = match action {
            ArmAction::Configure => {
                let Some(node) = self.require_selected_node("live configuration") else {
                    return;
                };
                let body =
                    configure_request_body(&node, &self.configure.playbook, &self.configure.group);
                (
                    "configure",
                    node,
                    CLOUD_ARM_NODE_SCOPE.to_string(),
                    body,
                    "live configuration (apply)".to_string(),
                )
            }
            ArmAction::Lifecycle { verb, name, .. } => {
                self.note = Some(format!(
                    "Legacy lifecycle action {verb} for {name} is retired; use the typed Workload operation. Nothing was sent."
                ));
                return;
            }
            ArmAction::Prepared {
                verb,
                node,
                target,
                body,
                label,
                ..
            } => (verb, node, target, body, label),
            ArmAction::Workload { .. } => {
                unreachable!("typed Workload operations are handled before the cloud dispatcher")
            }
            ArmAction::ExecPrepared { .. } => {
                unreachable!("Android execution actions are handled before the cloud dispatcher")
            }
            ArmAction::AndroidLifecycle { .. } => {
                unreachable!("Android lifecycle actions are handled before the cloud dispatcher")
            }
        };
        if requires_apply_capability && !self.selected_node_apply_armed() {
            self.note = Some(
                "Live apply is unavailable: the selected node is plan-only or no longer \
                 reports an armed-apply capability. Nothing was sent."
                    .to_string(),
            );
            return;
        }
        match self.authorize_body(&body, verb, &node, &target) {
            Ok(body) => self.issue(verb, Some(&body), &label),
            Err(error) => self.note = Some(format!("{error} Nothing was sent.")),
        }
    }

    // ── the plan/apply gate seams (§6, shared by the body + the menubar) ──

    /// Emit a dedicated **plan** (dry-run) — direct, no confirm. On a plan-only
    /// node the worker stages a `tofu plan` and returns it in the reply.
    pub(super) fn plan_provision(&mut self) {
        let Some(node) = self.require_selected_node("a provision plan") else {
            return;
        };
        let body = node_request_body(&node);
        self.issue(VERB_PLAN, Some(&body), "provision plan (dry-run)");
    }

    /// Emit a configuration **check** (dry-run `--check`) — direct.
    pub(super) fn check_configure(&mut self) {
        if let Some(body) = self.configure_body() {
            self.issue("configure", Some(&body), "configuration check (dry-run)");
        }
    }

    /// Open the review-sheet confirm for a live configuration **apply**.
    pub(super) fn arm_configure(&mut self) {
        if !self.selected_node_apply_armed() {
            self.note = Some(
                "Live configuration is unavailable: the selected node is plan-only or no longer \
                 reports an armed-apply capability."
                    .to_string(),
            );
            return;
        }
        self.note = None;
        self.arming = Some(ReviewSheetState {
            action: ArmAction::Configure,
            typed: String::new(),
        });
    }

    /// Open a short Yes/No review for the sole typed Workload operation lane.
    /// The legacy cloud/lifecycle verbs are deliberately not used for VM or
    /// Quadlet day-2 controls.
    pub(super) fn issue_workload_direct(
        &mut self,
        verb: &'static str,
        node: &str,
        workload_id: &str,
        delivery: DeliveryType,
        name: &str,
    ) {
        let node = node.trim();
        let workload_id = workload_id.trim();
        if node.is_empty() || workload_id.is_empty() {
            self.note = Some(format!(
                "Could not request {} on {name}: the workload placement or identity is missing.",
                verb_label(verb)
            ));
            return;
        }
        let backend = if delivery == DeliveryType::ServiceContainer {
            WorkloadBackend::QuadletSystemd
        } else {
            WorkloadBackend::LibvirtVirtqemud
        };
        let (action, attachment, label, word) = match verb {
            "instance-start" => (
                WorkloadOperationAction::StartAndAttach,
                Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
                format!("start and attach {name}"),
                "Start",
            ),
            "instance-stop" | "container-stop" => (
                WorkloadOperationAction::Stop,
                None,
                format!("stop {name}"),
                "Stop",
            ),
            "instance-reboot" | "container-restart" => (
                WorkloadOperationAction::Restart,
                Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
                format!("restart {name}"),
                "Restart",
            ),
            "instance-open" => (
                WorkloadOperationAction::Open,
                Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
                format!("open {name}"),
                "Open",
            ),
            "container-logs" => (
                WorkloadOperationAction::Open,
                Some(WorkloadAttachmentProtocol::Logs),
                format!("open logs for {name}"),
                "Open",
            ),
            "instance-delete" | "container-destroy" => (
                WorkloadOperationAction::Destroy,
                None,
                format!("destroy {name}"),
                "Destroy",
            ),
            _ => {
                self.note = Some(format!(
                    "Workload operation {verb} is not available on the typed API. Nothing was sent."
                ));
                return;
            }
        };
        // Workload operations use a durable compare-and-swap generation. Read
        // the authoritative node projection before minting the capability so
        // a second click cannot be rejected merely because the shell retained
        // an old zero-generation draft.
        let expected_generation = self
            .persist()
            .and_then(|persist| crate::workload_api::read_status(&persist, node, workload_id))
            .map(|status| status.generation)
            .unwrap_or(0);
        let request = match crate::workload_api::request(
            workload_id,
            node,
            backend,
            WorkloadProfile::Small.resources(),
            action,
            attachment,
            expected_generation,
            workload_api_now_ms(),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.note = Some(format!("Could not prepare {label}: {error}"));
                return;
            }
        };
        self.arming = Some(ReviewSheetState {
            action: ArmAction::Workload {
                request,
                label: label.clone(),
                echo: name.to_string(),
                word,
                subject: label,
            },
            typed: String::new(),
        });
    }

    /// Issue a typed Workload Open request for the selected presentation. The
    /// authoritative projection carries the expiring Display1 attachment lease;
    /// no cloud console verb or raw endpoint is involved.
    pub(super) fn issue_console_attach(&mut self, node: &str, instance_id: &str, name: &str) {
        let node = node.trim();
        let instance_id = instance_id.trim();
        let name = name.trim();
        if node.is_empty() {
            self.note = Some(format!(
                "Could not request console on {name}: the workload has no placement node."
            ));
            return;
        }
        if instance_id.is_empty() || name.is_empty() {
            self.note = Some(
                "Could not request console: the workload identity is incomplete (instance and \
                 name are required). Nothing was sent."
                    .to_string(),
            );
            return;
        }
        self.issue_workload_direct(
            "instance-open",
            node,
            instance_id,
            DeliveryType::DesktopVm,
            name,
        );
    }

    /// Publish the admitted App VM declaration through the existing session
    /// broker. An incomplete mirror row fails closed; it never falls back to a
    /// host-side desktop launch.
    pub(super) fn issue_app_launch(&mut self, row: &WorkloadRow) {
        let Some(request) = row.app.clone() else {
            self.note = Some(format!(
                "Could not launch an app from {}: no admitted app declaration is available. Nothing was sent.",
                row.name
            ));
            return;
        };
        let Some(bus_root) = self.bus_root.as_deref() else {
            self.note = Some(
                "Could not launch the App VM application: no mesh Bus directory is configured. Nothing was sent."
                    .to_string(),
            );
            return;
        };
        let mut last_error = None;
        match crate::discovery::publish_app_vm_open(
            Some(bus_root),
            &mut last_error,
            &row.node,
            &row.name,
            // The placement node serves the guest, but this shell is the
            // client that owns the rail/VDI surface. Using the placement here
            // made remote App VM launches invisible to the local session rail.
            &crate::discovery::local_peer(),
            request,
        ) {
            Ok(publication) => {
                self.note = Some(format!(
                    "App launch requested for {} (session {}). Waiting for guest readiness.",
                    row.name, publication.id
                ));
            }
            Err(error) => {
                self.note = Some(format!("App launch was not sent: {error}"));
            }
        }
    }

    /// Open the review-sheet confirm for a destructive lifecycle op
    /// (`instance-reboot` / `instance-delete`) — nothing publishes until the
    /// workload name is typed (RUN-006). The resource rows drive this seam.
    pub(super) fn arm_lifecycle(
        &mut self,
        verb: &'static str,
        node: &str,
        instance_id: &str,
        name: &str,
    ) {
        let node = node.trim();
        if node.is_empty() {
            self.note = Some(format!(
                "Could not arm {} on {name}: the workload has no placement node.",
                verb_label(verb)
            ));
            return;
        }
        let instance_id = instance_id.trim();
        let name = name.trim();
        if instance_id.is_empty() || name.is_empty() {
            self.note = Some(format!(
                "Could not arm {}: the workload identity is incomplete (instance and name are \
                 required). Nothing was sent.",
                verb_label(verb)
            ));
            return;
        }
        self.note = None;
        self.arming = Some(ReviewSheetState {
            action: ArmAction::Lifecycle {
                verb,
                node: node.to_string(),
                instance_id: instance_id.to_string(),
                name: name.to_string(),
            },
            typed: String::new(),
        });
    }

    /// Open a Yes/No review for a fully prepared route mutation. The body is
    /// frozen now, preventing form edits from changing what the later token binds.
    pub(super) fn arm_prepared(
        &mut self,
        verb: &'static str,
        node: String,
        target: String,
        body: String,
        label: String,
        echo: String,
        word: &'static str,
        subject: String,
    ) {
        if node.trim().is_empty() {
            self.note = Some(format!(
                "Select a placement node before requesting {label}."
            ));
            return;
        }
        self.note = None;
        self.arming = Some(ReviewSheetState {
            action: ArmAction::Prepared {
                verb,
                node: node.trim().to_string(),
                target,
                body,
                label,
                echo,
                word,
                subject,
            },
            typed: String::new(),
        });
    }

    // ── the nav + menubar seam (§6, one dispatch path shared with the body) ──

    /// The folded per-node `state/cloud` mirror (the menubar status cluster reads
    /// the same fold the body renders — no second read, §7).
    pub(super) fn states(&self) -> &[CloudState] {
        &self.states
    }

    /// Return the admitted, stable Browser VM workload from the folded
    /// Workloads mirror. Browser activation consumes this read model rather
    /// than guessing a placement node or falling back to a host engine.
    pub(super) fn browser_vm_target(&self) -> Option<(&str, &str, &str, bool)> {
        let local_peer = crate::discovery::local_peer();
        self.browser_vm_target_at(&local_peer, unix_now_ms())
    }

    fn browser_vm_target_at<'a>(
        &'a self,
        local_peer: &str,
        now_ms: i64,
    ) -> Option<(&'a str, &'a str, &'a str, bool)> {
        let (state, workload) = self
            .states
            .iter()
            .flat_map(|state| {
                state
                    .workloads
                    .iter()
                    .map(move |workload| (state, workload))
            })
            .filter(|(_, workload)| {
                workload.name == "browser-vm" && workload.delivery_type == DeliveryType::DesktopVm
            })
            .max_by(|(a_state, a), (b_state, b)| {
                compare_browser_vm_candidates(a_state, a, b_state, b, local_peer, now_ms)
            })?;
        Some((
            workload.node.as_str(),
            workload.name.as_str(),
            workload.status.as_str(),
            workload.reachable && cloud_state_is_fresh_at(state, now_ms),
        ))
    }

    /// Every workload of a given delivery type, across every node — the idiom a
    /// delivery view uses to read its own rows from the mirror.
    pub(super) fn workloads_of(
        &self,
        view: DeliveryView,
    ) -> impl Iterator<Item = &WorkloadRow> + '_ {
        let dt = view.delivery_type();
        self.states
            .iter()
            .flat_map(|s| s.workloads.iter())
            .filter(move |w| w.delivery_type == dt)
    }

    /// Which delivery view is showing (test seam; production reads the field).
    #[cfg(test)]
    pub(super) fn view(&self) -> DeliveryView {
        self.view
    }

    /// Switch the active delivery view.
    pub(super) fn set_view(&mut self, view: DeliveryView) {
        if self.view != view {
            self.view = view;
            self.provision_progress = ProvisionProgress::Draft;
            self.provision_draft_key = None;
        }
    }

    /// Which lifecycle route is showing.
    #[cfg(test)]
    pub(super) fn route(&self) -> WorkloadsRoute {
        self.route
    }

    /// Switch the active lifecycle route.
    pub(super) fn set_route(&mut self, route: WorkloadsRoute) {
        self.route = route;
    }

    /// Keep progression bound to the exact delivery/placement/form draft. Any
    /// edit invalidates the previous desired/plan milestone.
    pub(super) fn sync_provision_draft(&mut self, key: String) {
        if self.provision_draft_key.as_deref() != Some(key.as_str()) {
            self.provision_draft_key = Some(key);
            self.provision_progress = ProvisionProgress::Draft;
        }
    }

    /// The next-step renderer reads the current backend-acknowledged milestone.
    pub(super) const fn provision_progress(&self) -> ProvisionProgress {
        self.provision_progress
    }

    /// Whether one cloud request is currently awaiting a typed reply.
    pub(super) const fn mutation_is_pending(&self) -> bool {
        self.mutation_pending.is_some()
    }

    /// Current resource table density.
    #[cfg(test)]
    pub(super) fn density(&self) -> DensityMode {
        self.density
    }

    /// Set resource table density.
    pub(super) fn set_density(&mut self, density: DensityMode) {
        self.density = density;
    }

    /// Current resource table sort.
    #[cfg(test)]
    pub(super) fn resource_sort(&self) -> WorkloadSort {
        self.resource_sort
    }

    /// Toggle a resource-table sort column.
    pub(super) fn toggle_resource_sort(&mut self, column: WorkloadSortColumn) {
        self.resource_sort = self.resource_sort.toggled(column);
    }

    /// Current expanded resource row key.
    #[cfg(test)]
    pub(super) fn expanded_resource(&self) -> Option<&str> {
        self.expanded_resource.as_deref()
    }

    /// Toggle the expanded Plan resource row.
    pub(super) fn toggle_expanded_resource(&mut self, key: String) {
        if self.expanded_resource.as_deref() == Some(key.as_str()) {
            self.expanded_resource = None;
        } else {
            self.expanded_resource = Some(key);
        }
    }

    /// The placement node the provision panel targets, if one is chosen.
    pub(super) fn selected_node(&self) -> Option<&str> {
        self.selected_node.as_deref()
    }

    /// Whether the selected placement node currently reports the armed-token
    /// capability needed for authorized mutations such as Ansible Configure.
    /// A missing node or stale selection fails closed.
    pub(super) fn selected_node_apply_armed(&self) -> bool {
        let Some(selected) = self.selected_node.as_deref() else {
            return false;
        };
        self.states
            .iter()
            .find(|state| state.host == selected)
            .is_some_and(|state| state.apply_armed && cloud_state_is_fresh(state))
    }

    /// Queue an immediate re-fold of the `state/cloud` mirror.
    pub(super) fn request_refresh(&mut self) {
        self.forced = true;
    }

    /// Surface the honest apply-gate + audit posture in the action note (Help).
    pub(super) fn set_help_note(&mut self) {
        self.note = Some(
            "Ansible Configure is capability-gated per node (armed token) and stages as a \
             dry-run otherwise. Workload lifecycle is target-scoped through typed row \
             operations; performed ops land in the Status audit trail."
                .to_string(),
        );
    }

    /// Whether a review-sheet confirm is open (test seam).
    #[cfg(test)]
    pub(super) fn has_arming(&self) -> bool {
        self.arming.is_some()
    }

    /// The current action note text, if any (test seam).
    #[cfg(test)]
    pub(super) fn note_text(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// Fold a settled mutation reply into `(honest note, audit row)` (§7 — the pure
/// seam shared by the poll path and the tests). A successful plan reads as a
/// completed dry-run; other `ok` replies read applied; a `gated` reply reads
/// staged (a dry-run — nothing applied) carrying the plan summary; an error
/// reads failed.
fn fold_mutation(reply: &CloudReply) -> (String, AuditEntry) {
    let verb = if reply.verb.is_empty() {
        "cloud op".to_string()
    } else {
        reply.verb.clone()
    };
    if reply.ok && reply.verb == VERB_PLAN {
        (
            "Dry-run plan completed; nothing was applied. Use the typed Workload row operation \
             when the declared workload is ready."
                .to_string(),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Staged,
                detail: "dry-run plan completed; nothing applied".to_string(),
            },
        )
    } else if reply.ok && reply.verb == VERB_ANDROID_PROVISION {
        let detail =
            "Cuttlefish desired state saved; lifecycle continues through its typed Workload row";
        (
            format!("{verb} saved desired state; no VM was provisioned yet."),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Desired,
                detail: detail.to_string(),
            },
        )
    } else if reply.ok {
        if reply.verb.is_empty() {
            let detail = reply
                .detail
                .clone()
                .unwrap_or_else(|| "typed action accepted".to_string());
            return (
                detail.clone(),
                AuditEntry {
                    verb,
                    outcome: AuditOutcome::Applied,
                    detail,
                },
            );
        }
        let audited = if reply.audited { " (audited)" } else { "" };
        (
            format!("{verb} applied{audited}."),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Applied,
                detail: if reply.audited {
                    "audited to the events plane".to_string()
                } else {
                    "applied".to_string()
                },
            },
        )
    } else if let Some(gated) = &reply.gated {
        (
            format!("{verb} staged (dry-run): {gated}"),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Staged,
                detail: gated.clone(),
            },
        )
    } else {
        let error = reply
            .error
            .clone()
            .unwrap_or_else(|| "unknown error".to_string());
        (
            format!("{verb} failed: {error}"),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Failed,
                detail: error,
            },
        )
    }
}

/// The button/label word for a lifecycle (or mutation) verb.
fn verb_label(verb: &str) -> &'static str {
    match verb {
        "instance-start" => "Start",
        "instance-stop" => "Stop",
        "instance-reboot" => "Reboot",
        "instance-delete" => "Delete",
        "configure" => "Configure",
        "destroy" => "Destroy",
        _ => "Run",
    }
}

// ───────────────────────────────── the render ───────────────────────────────

/// Render the Workloads app into `ui`: the shared MENUBAR-ALL bar, a native
/// lifecycle sidebar, a delivery-type filter, the review sheet + action note,
/// then the active route body with a persistent health rail.
///
/// The name is the stable external entry seam (main.rs binds it); the state type
/// is [`WorkloadsState`] (aliased as `InfraCodeState`).
pub fn infra_code_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_XS);

    let content_height = ui.available_height().max(320.0);
    let show_health_rail = ui.available_width() >= 1_120.0;
    ui.horizontal_top(|ui| {
        ui.set_min_height(content_height);
        ui.vertical(|ui| {
            ui.set_width(190.0);
            egui::ScrollArea::vertical()
                .id_salt("workloads-navigation")
                .max_height(content_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_enabled_ui(state.arming.is_none(), |ui| {
                        route_sidebar(ui, state);
                        ui.add_space(Style::SP_S);
                        ui.separator();
                        ui.add_space(Style::SP_S);
                        delivery_filter_bar(ui, state);
                        ui.add_space(Style::SP_S);
                        density_selector(ui, state);
                    });
                });
        });

        ui.separator();

        ui.vertical(|ui| {
            ui.set_min_width(if show_health_rail { 500.0 } else { 360.0 });
            route_header(ui, state.route);
            ui.add_space(Style::SP_S);
            render_note(ui, state);
            if state.arming.is_some() {
                render_review_sheet(ui, state);
            } else {
                let body_height = ui.available_height().max(220.0);
                egui::ScrollArea::vertical()
                    .id_salt("workloads-route-body")
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| route_body(ui, state));
            }
        });

        if show_health_rail {
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(210.0);
                health_rail(ui, state);
            });
        }
    });
}

fn route_body(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    match state.route {
        WorkloadsRoute::Provision => {
            shared_placement_selector(ui, state);
            provision_form::provision_form(ui, state);
        }
        WorkloadsRoute::Plan => {
            lifecycle_resource_route(ui, state, ResourceTableMode::Plan);
        }
        WorkloadsRoute::Run => {
            shared_placement_selector(ui, state);
            lifecycle_resource_route(ui, state, ResourceTableMode::Run);
            ui.add_space(Style::SP_S);
            configure::configure_panel(ui, state);
        }
        WorkloadsRoute::Drift => {
            lifecycle_resource_route(ui, state, ResourceTableMode::Drift);
            ui.add_space(Style::SP_S);
            status::status_panel(ui, state);
        }
        WorkloadsRoute::Audit => audit_route_panel(ui, state),
        WorkloadsRoute::Images => {
            shared_placement_selector(ui, state);
            images::images_panel(ui, state);
        }
        WorkloadsRoute::Containers => {
            shared_placement_selector(ui, state);
            lifecycle_resource_route_for_delivery(
                ui,
                state,
                ResourceTableMode::Containers,
                DeliveryType::ServiceContainer,
            );
            ui.add_space(Style::SP_S);
            containers::containers_panel(ui, state);
        }
    }
}

fn shared_placement_selector(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    if let Some(node) = placement::placement_picker(ui, state) {
        state.selected_node = Some(node);
    }
    ui.add_space(Style::SP_S);
}

/// Lifecycle sidebar — the primary navigation axis for WL-UX-008.
fn route_sidebar(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    ui.label(
        RichText::new("Lifecycle")
            .size(Style::SMALL)
            .strong()
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    for route in WorkloadsRoute::ALL {
        let selected = state.route == route;
        if sidebar_row(
            ui,
            selected,
            route.icon(),
            route.label(),
            Style::ACCENT_WORKLOADS,
        )
        .clicked()
        {
            state.set_route(route);
        }
    }
}

/// Delivery types are filters, not route tabs.
fn delivery_filter_bar(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    ui.label(
        RichText::new("Delivery filter")
            .size(Style::SMALL)
            .strong()
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    for view in DeliveryView::ALL {
        let selected = state.view == view;
        if sidebar_row(ui, selected, view.icon(), view.label(), Style::ACCENT).clicked() {
            state.set_view(view);
        }
    }
}

fn density_selector(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    ui.label(
        RichText::new("Density")
            .size(Style::SMALL)
            .strong()
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    for density in DensityMode::ALL {
        let selected = state.density == density;
        if sidebar_row(ui, selected, "view-list", density.label(), Style::ACCENT).clicked() {
            state.set_density(density);
        }
    }
}

fn route_header(ui: &mut egui::Ui, route: WorkloadsRoute) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, route.icon(), Style::ICON_L);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(route.label())
                .size(Style::TITLE)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    muted_note(ui, route.blurb());
}

fn lifecycle_resource_route(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    mode: ResourceTableMode,
) {
    lifecycle_resource_route_for_filter(ui, state, mode, None);
}

fn lifecycle_resource_route_for_delivery(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    mode: ResourceTableMode,
    delivery_type: DeliveryType,
) {
    lifecycle_resource_route_for_filter(ui, state, mode, Some(delivery_type));
}

fn lifecycle_resource_route_for_filter(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    mode: ResourceTableMode,
    delivery_type: Option<DeliveryType>,
) {
    mirror_summary(ui, state);
    ui.add_space(Style::SP_XS);
    let delivery_label = delivery_type.map_or_else(|| state.view.label(), DeliveryType::label);
    muted_note(
        ui,
        format!(
            "{} · showing {} resources as the current delivery filter. Row density: {} ({:.0}px).",
            mode.summary(),
            delivery_label,
            state.density.label(),
            state.density.row_height()
        ),
    );
    ui.add_space(Style::SP_S);
    resource_table(ui, state, mode, delivery_type);
    let effective_view = delivery_type
        .map(DeliveryView::from_delivery_type)
        .unwrap_or(state.view);
    if should_show_android_starter_catalog(mode, effective_view) {
        let mut workloads = state
            .workloads_of(DeliveryView::AndroidVm)
            .map(|row| android_governed::WorkloadInput {
                workload_id: row.name.clone(),
                node: row.node.clone(),
                runtime_status: row.status.clone(),
            })
            .collect::<Vec<_>>();
        workloads.sort_by(|left, right| {
            left.workload_id
                .cmp(&right.workload_id)
                .then_with(|| left.node.cmp(&right.node))
        });
        let projections = workloads
            .iter()
            .map(|workload| {
                let cloud = state
                    .states
                    .iter()
                    .find(|cloud| cloud.host == workload.node);
                android_governed::project(android_governed::ModelInput {
                    workload,
                    catalog: state
                        .android_catalogs
                        .iter()
                        .find(|catalog| catalog.node == workload.node),
                    admitted_cache_digest: state
                        .android_admitted_catalog_digests
                        .get(&workload.node)
                        .map(String::as_str),
                    admission: cloud.and_then(|cloud| {
                        cloud
                            .android_provider_admissions
                            .iter()
                            .find(|row| row.workload_id == workload.workload_id)
                    }),
                    inventory: cloud.and_then(|cloud| {
                        cloud
                            .android_inventories
                            .iter()
                            .find(|row| row.workload_id == workload.workload_id)
                    }),
                    vdi_source: cloud.and_then(|cloud| {
                        cloud
                            .android_vdi_sources
                            .iter()
                            .find(|row| row.workload_id == workload.workload_id)
                    }),
                    receipt: state.android_lifecycle_receipts.get(&workload.workload_id),
                    pending: state.android_lifecycle_pending.as_ref(),
                    now_unix_ms: state.android_projection_now_ms,
                })
            })
            .collect::<Vec<_>>();
        if let Some(action) =
            android_governed::panel(ui, &projections, mode == ResourceTableMode::Run)
        {
            match action {
                android_governed::UiAction::Lifecycle {
                    node,
                    workload_id,
                    operation,
                    app,
                    expected_generation,
                    approval,
                } => state.arm_android_lifecycle(
                    &node,
                    &workload_id,
                    operation,
                    app,
                    expected_generation,
                    approval,
                ),
                android_governed::UiAction::Attach { node, source } => {
                    queue_android_vdi_handoff(
                        ui.ctx(),
                        AndroidVdiHandoff {
                            placement_node: node,
                            source,
                        },
                    );
                    state.note = Some(
                        "Android WebRTC source is ready for the remote-session attachment route."
                            .to_owned(),
                    );
                }
            }
        }
    }
}

/// The starter catalog belongs to lifecycle review, not to the retired
/// delivery-view renderer. Drift and container routes remain inventory-only.
const fn should_show_android_starter_catalog(mode: ResourceTableMode, view: DeliveryView) -> bool {
    matches!(mode, ResourceTableMode::Plan | ResourceTableMode::Run)
        && matches!(view, DeliveryView::AndroidVm)
}

fn resource_table(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    mode: ResourceTableMode,
    delivery_type: Option<DeliveryType>,
) {
    let rows = resource_rows_for(state, delivery_type);
    if rows.is_empty() {
        let delivery_label = delivery_type.map_or_else(|| state.view.label(), DeliveryType::label);
        let message = format!(
            "No {} workloads are present in the folded state/cloud mirror.",
            delivery_label
        );
        crate::empty_state::show(ui, mode.empty_title(), &message);
        return;
    }

    card().show(ui, |ui| {
        ui.label(
            RichText::new(mode.heading())
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_XS);
        resource_table_header(ui, state, mode);
        ui.separator();
        ui.add_space(Style::SP_XS);
        for row in rows {
            resource_table_row(ui, state, &row, mode);
        }
    });
}

fn resource_table_header(ui: &mut egui::Ui, state: &mut WorkloadsState, mode: ResourceTableMode) {
    let header_height = DensityMode::Compact.row_height();
    ui.horizontal(|ui| {
        ui.add_sized(
            [78.0, header_height],
            egui::Label::new(
                RichText::new("Details")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            ),
        );
        for column in WorkloadSortColumn::ALL {
            let label = format!("{}{}", column.label(), state.resource_sort.marker(column));
            if ui
                .add_sized(
                    [column_width(column), header_height],
                    egui::Button::new(
                        RichText::new(label)
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    ),
                )
                .clicked()
            {
                state.toggle_resource_sort(column);
            }
        }
        ui.add_sized(
            [188.0, header_height],
            egui::Label::new(
                RichText::new(mode.action_header())
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            ),
        );
    });
}

fn resource_table_row(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    row: &WorkloadRow,
    mode: ResourceTableMode,
) {
    let key = plan_resource_key(row);
    let expanded = state.expanded_resource.as_deref() == Some(key.as_str());
    let row_height = state.density.row_height();
    ui.horizontal(|ui| {
        if ui
            .add_sized(
                [78.0, row_height],
                egui::Button::new(
                    RichText::new(if expanded { "Collapse" } else { "Details" }).size(Style::SMALL),
                ),
            )
            .clicked()
        {
            state.toggle_expanded_resource(key.clone());
        }
        plan_cell(
            ui,
            &row.name,
            column_width(WorkloadSortColumn::Name),
            row_height,
            Style::TEXT,
        );
        plan_cell(
            ui,
            &row.node,
            column_width(WorkloadSortColumn::Node),
            row_height,
            Style::TEXT_DIM,
        );
        plan_cell(
            ui,
            &row.status,
            column_width(WorkloadSortColumn::Status),
            row_height,
            status_tone(&row.status),
        );
        plan_cell(
            ui,
            &format!("{}%", row.cpu_pct),
            column_width(WorkloadSortColumn::Cpu),
            row_height,
            load_tone(row.cpu_pct),
        );
        plan_cell(
            ui,
            &mem_label(row.mem_mb),
            column_width(WorkloadSortColumn::Memory),
            row_height,
            Style::TEXT,
        );
        plan_cell(
            ui,
            &format!("{} GiB", row.disk_gb),
            column_width(WorkloadSortColumn::Disk),
            row_height,
            Style::TEXT,
        );
        plan_cell(
            ui,
            drift_word(row.drift),
            column_width(WorkloadSortColumn::Drift),
            row_height,
            drift_tone(row.drift),
        );
        resource_row_actions(ui, state, row, mode);
    });
    if expanded {
        plan_expanded_row(ui, state, row, mode);
    }
    ui.add_space(Style::SP_XS);
}

fn plan_cell(ui: &mut egui::Ui, text: &str, width: f32, height: f32, color: Color32) {
    ui.add_sized(
        [width, height],
        egui::Label::new(RichText::new(text).size(Style::SMALL).color(color)),
    );
}

fn resource_row_actions(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    row: &WorkloadRow,
    mode: ResourceTableMode,
) {
    if mode == ResourceTableMode::Drift {
        ui.horizontal(|ui| {
            if row_button(ui, "Plan node", false).clicked() {
                let body = node_request_body(&row.node);
                state.issue(
                    VERB_PLAN,
                    Some(&body),
                    &format!("drift plan for {}", row.name),
                );
            }
            if row_button(ui, "Details", false).clicked() {
                state.toggle_expanded_resource(plan_resource_key(row));
            }
        });
        return;
    }

    ui.horizontal(|ui| match row.delivery_type {
        DeliveryType::ServiceContainer => {
            if row_button(ui, "Restart", false).clicked() {
                state.issue_workload_direct(
                    "container-restart",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Logs", false).clicked() {
                state.issue_workload_direct(
                    "container-logs",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.issue_workload_direct(
                    "container-destroy",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
        }
        DeliveryType::ServiceVm => {
            vm_lifecycle_actions(ui, state, row, false);
        }
        DeliveryType::AppVm => {
            let launch_enabled = app_vm_launch_allowed(row, mode);
            let color = if launch_enabled {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            };
            if ui
                .add_enabled(
                    launch_enabled,
                    egui::Button::new(RichText::new("Open app").size(Style::SMALL).color(color)),
                )
                .clicked()
            {
                state.issue_app_launch(row);
            }
            vm_lifecycle_actions(ui, state, row, true);
        }
        DeliveryType::DesktopVm | DeliveryType::AndroidVm => {
            vm_lifecycle_actions(ui, state, row, true);
        }
    });
}

/// The active Workloads table may open an App VM only when the folded row
/// carries a valid typed declaration. Plan and malformed/absent rows stay
/// visibly disabled; the shell never invents a host-side launch fallback.
fn app_vm_launch_allowed(row: &WorkloadRow, mode: ResourceTableMode) -> bool {
    mode == ResourceTableMode::Run
        && row.delivery_type == DeliveryType::AppVm
        && row.app.as_ref().is_some_and(|request| {
            request.validate().is_ok()
                && is_valid_flatpak_app_id(&request.app_id)
                && request
                    .requested_capabilities
                    .iter()
                    .all(|capability| APP_VM_ALLOWED_CAPABILITIES.contains(&capability.as_str()))
        })
}

fn vm_lifecycle_actions(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    row: &WorkloadRow,
    console: bool,
) {
    if console && row_button(ui, "Console", false).clicked() {
        state.issue_console_attach(&row.node, &row.name, &row.name);
    }
    if row_button(ui, "Start", false).clicked() {
        state.issue_workload_direct(
            "instance-start",
            &row.node,
            &row.name,
            row.delivery_type,
            &row.name,
        );
    }
    if row_button(ui, "Stop", false).clicked() {
        state.issue_workload_direct(
            "instance-stop",
            &row.node,
            &row.name,
            row.delivery_type,
            &row.name,
        );
    }
    if row_button(ui, "Reboot\u{2026}", true).clicked() {
        state.issue_workload_direct(
            "instance-reboot",
            &row.node,
            &row.name,
            row.delivery_type,
            &row.name,
        );
    }
    if row_button(ui, "Destroy\u{2026}", true).clicked() {
        state.issue_workload_direct(
            "instance-delete",
            &row.node,
            &row.name,
            row.delivery_type,
            &row.name,
        );
    }
}

fn plan_expanded_row(
    ui: &mut egui::Ui,
    state: &WorkloadsState,
    row: &WorkloadRow,
    mode: ResourceTableMode,
) {
    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(Style::STROKE_HAIRLINE, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                field(
                    ui,
                    "delivery",
                    row.delivery_type.label(),
                    Style::ACCENT_WORKLOADS,
                );
                ui.add_space(Style::SP_M);
                field(ui, "placement", &row.node, Style::TEXT);
                ui.add_space(Style::SP_M);
                field(
                    ui,
                    "metrics",
                    &format!(
                        "{}% cpu · {} · {} GiB",
                        row.cpu_pct,
                        mem_label(row.mem_mb),
                        row.disk_gb
                    ),
                    Style::TEXT,
                );
                ui.add_space(Style::SP_M);
                field(ui, "drift", drift_word(row.drift), drift_tone(row.drift));
                ui.add_space(Style::SP_M);
                field(
                    ui,
                    "mesh",
                    if row.reachable {
                        "reachable"
                    } else {
                        "unreachable"
                    },
                    if row.reachable {
                        Style::OK
                    } else {
                        Style::WARN
                    },
                );
            });
            ui.add_space(Style::SP_XS);
            muted_note(
                ui,
                format!(
                    "Command preview \u{2014} {}: row actions publish typed Bus requests with \
                     node=`{}` and target=`{}`. Live actions require the exact `{}` echo before \
                     anything is sent; current table density is {}.",
                    mode.preview_word(),
                    row.node,
                    row.name,
                    row.name,
                    state.density.label()
                ),
            );
        });
}

fn column_width(column: WorkloadSortColumn) -> f32 {
    match column {
        WorkloadSortColumn::Name => 132.0,
        WorkloadSortColumn::Node => 104.0,
        WorkloadSortColumn::Status => 88.0,
        WorkloadSortColumn::Cpu => 56.0,
        WorkloadSortColumn::Memory => 78.0,
        WorkloadSortColumn::Disk => 68.0,
        WorkloadSortColumn::Drift => 82.0,
    }
}

fn plan_resource_rows(state: &WorkloadsState) -> Vec<WorkloadRow> {
    resource_rows_for(state, None)
}

fn resource_rows_for(
    state: &WorkloadsState,
    delivery_type: Option<DeliveryType>,
) -> Vec<WorkloadRow> {
    let view = delivery_type.map_or(state.view, DeliveryView::from_delivery_type);
    let mut rows: Vec<WorkloadRow> = state.workloads_of(view).cloned().collect();
    let sort = state.resource_sort;
    rows.sort_by(|a, b| {
        let ordering = compare_workload_rows(a, b, sort.column);
        if sort.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

fn compare_workload_rows(a: &WorkloadRow, b: &WorkloadRow, column: WorkloadSortColumn) -> Ordering {
    match column {
        WorkloadSortColumn::Name => a.name.cmp(&b.name),
        WorkloadSortColumn::Node => a.node.cmp(&b.node),
        WorkloadSortColumn::Status => a.status.cmp(&b.status),
        WorkloadSortColumn::Cpu => a.cpu_pct.cmp(&b.cpu_pct),
        WorkloadSortColumn::Memory => a.mem_mb.cmp(&b.mem_mb),
        WorkloadSortColumn::Disk => a.disk_gb.cmp(&b.disk_gb),
        WorkloadSortColumn::Drift => drift_rank(a.drift).cmp(&drift_rank(b.drift)),
    }
}

fn plan_resource_key(row: &WorkloadRow) -> String {
    format!("{}:{}:{}", row.delivery_type.as_str(), row.node, row.name)
}

const fn drift_rank(drift: DriftFlag) -> u8 {
    match drift {
        DriftFlag::InSync => 0,
        DriftFlag::Unknown => 1,
        DriftFlag::Drift => 2,
    }
}

/// The Style tone a live workload status paints.
fn status_tone(status: &str) -> Color32 {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "active" => Style::SUPPORT_SUCCESS,
        "paused" | "pmsuspended" => Style::WARN,
        s if s.contains("error") || s.contains("fail") || s.contains("crash") => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

/// The Style tone a drift flag paints.
const fn drift_tone(drift: DriftFlag) -> Color32 {
    match drift {
        DriftFlag::InSync => Style::SUPPORT_SUCCESS,
        DriftFlag::Drift => Style::SUPPORT_WARNING,
        DriftFlag::Unknown => Style::TEXT_DIM,
    }
}

/// The drift word shown in compact and expanded rows.
const fn drift_word(drift: DriftFlag) -> &'static str {
    match drift {
        DriftFlag::InSync => "in sync",
        DriftFlag::Drift => "drift",
        DriftFlag::Unknown => "unplanned",
    }
}

/// The Style tone a cpu-load percentage paints (amber past 70, red past 90).
const fn load_tone(pct: u16) -> Color32 {
    if pct >= 90 {
        Style::DANGER
    } else if pct >= 70 {
        Style::WARN
    } else {
        Style::TEXT
    }
}

/// A memory figure as MiB, or one-decimal GiB past a gibibyte.
fn mem_label(mb: u32) -> String {
    if mb >= 1024 {
        format!("{}.{} GiB", mb / 1024, (mb % 1024) * 10 / 1024)
    } else {
        format!("{mb} MiB")
    }
}

fn audit_route_panel(ui: &mut egui::Ui, state: &WorkloadsState) {
    mirror_summary(ui, state);
    ui.add_space(Style::SP_S);
    if state.audit.is_empty() {
        crate::empty_state::show(
            ui,
            "No audit rows this session",
            "Plans, Ansible Configure, image actions, and typed Workload operations append here.",
        );
        return;
    }
    audit_table(ui, &state.audit);
}

fn audit_table(ui: &mut egui::Ui, audit: &[AuditEntry]) {
    card().show(ui, |ui| {
        ui.label(
            RichText::new("Audit table")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            audit_cell(
                ui,
                "Outcome",
                112.0,
                DensityMode::Compact.row_height(),
                Style::TEXT_DIM,
            );
            audit_cell(
                ui,
                "Verb",
                150.0,
                DensityMode::Compact.row_height(),
                Style::TEXT_DIM,
            );
            audit_cell(
                ui,
                "Detail",
                520.0,
                DensityMode::Compact.row_height(),
                Style::TEXT_DIM,
            );
        });
        ui.separator();
        for entry in audit.iter().rev() {
            ui.horizontal(|ui| {
                audit_cell(
                    ui,
                    entry.outcome.word(),
                    112.0,
                    DensityMode::Compact.row_height(),
                    entry.outcome.color(),
                );
                audit_cell(
                    ui,
                    &entry.verb,
                    150.0,
                    DensityMode::Compact.row_height(),
                    Style::TEXT,
                );
                audit_cell(
                    ui,
                    &entry.detail,
                    520.0,
                    DensityMode::Compact.row_height(),
                    Style::TEXT_DIM,
                );
            });
        }
    });
}

fn audit_cell(ui: &mut egui::Ui, text: &str, width: f32, height: f32, color: Color32) {
    ui.add_sized(
        [width, height],
        egui::Label::new(RichText::new(text).size(Style::SMALL).color(color)),
    );
}

fn health_rail(ui: &mut egui::Ui, state: &WorkloadsState) {
    ui.label(
        RichText::new("Health")
            .size(Style::SMALL)
            .strong()
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    let nodes = state.states.len();
    let ready = state
        .states
        .iter()
        .filter(|state| cloud_state_is_fresh(state) && state.backend_ready())
        .count();
    let armed = state
        .states
        .iter()
        .filter(|state| cloud_state_is_fresh(state) && state.apply_armed)
        .count();
    let workloads: usize = state.states.iter().map(|state| state.workloads.len()).sum();
    let stale = state
        .states
        .iter()
        .filter(|state| !cloud_state_is_fresh(state))
        .count();
    for (label, value, tone) in [
        ("nodes", nodes.to_string(), Style::TEXT),
        (
            "backend",
            format!("{ready}/{nodes} ready"),
            if ready == nodes && nodes > 0 {
                Style::OK
            } else {
                Style::WARN
            },
        ),
        (
            "apply",
            format!("{armed}/{nodes} armed"),
            if armed > 0 { Style::DANGER } else { Style::OK },
        ),
        ("workloads", workloads.to_string(), Style::TEXT),
        (
            "stale",
            stale.to_string(),
            if stale == 0 { Style::OK } else { Style::WARN },
        ),
        ("audit", state.audit.len().to_string(), Style::TEXT_DIM),
    ] {
        field(ui, label, &value, tone);
        ui.add_space(Style::SP_XS);
    }
}

/// One sidebar row — a clickable icon + label row.
fn sidebar_row(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &str,
    label: &str,
    accent: Color32,
) -> egui::Response {
    let color = if selected { accent } else { Style::TEXT_DIM };
    let row_size = egui::vec2(ui.available_width().max(1.0), Style::SP_L + Style::SP_S);
    let (rect, resp) = ui.allocate_exact_size(row_size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if selected || resp.hovered() {
        ui.painter().rect_filled(
            rect,
            Style::RADIUS_S,
            accent.gamma_multiply(if selected { 0.16 } else { 0.08 }),
        );
    }
    let inner = rect.shrink2(egui::vec2(Style::SP_S, Style::SP_XS));
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row_ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(color);
        carbon_icon(ui, icon, Style::ICON_M);
    });
    row_ui.add_space(Style::SP_XS);
    row_ui.label(RichText::new(label).size(Style::BODY).color(color).strong());
    resp
}

/// A small text button sized for a roster row's inline lifecycle verb.
fn row_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    let color = if danger { Style::DANGER } else { Style::TEXT };
    ui.add(
        egui::Button::new(RichText::new(label).size(Style::SMALL).color(color))
            .min_size(egui::vec2(0.0, Style::density(ui.ctx()).min_hit_target())),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Prominent operation status. Pending and completed results occupy a stable
/// location above the workflow instead of being buried among form controls.
fn render_note(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    let pending = state
        .mutation_pending
        .as_ref()
        .map(|request| (request.verb.clone(), request.sent.elapsed().as_secs()));
    let Some(note) = state.note.clone().or_else(|| {
        pending
            .as_ref()
            .map(|(verb, _)| format!("Waiting for `{verb}`\u{2026}"))
    }) else {
        return;
    };
    let failed = note.contains("failed")
        || note.contains("did not answer")
        || note.contains("Could not")
        || note.contains("unavailable");
    let tone = if failed { Style::DANGER } else { Style::ACCENT };
    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(Style::STROKE_HAIRLINE, tone))
        .corner_radius(Style::RADIUS_S)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let title = if pending.is_some() {
                    "Working"
                } else if failed {
                    "Action required"
                } else {
                    "Step result"
                };
                ui.colored_label(tone, RichText::new(title).size(Style::SMALL).strong());
                let message = pending.as_ref().map_or(note.clone(), |(verb, elapsed)| {
                    format!(
                        "Waiting for the cloud backend to finish `{verb}` ({elapsed}s elapsed)."
                    )
                });
                ui.label(RichText::new(message).size(Style::SMALL).color(Style::TEXT));
                if pending.is_none() && ui.small_button("Dismiss").clicked() {
                    state.note = None;
                }
            });
        });
    ui.add_space(Style::SP_S);
}

/// The pending mutation review sheet. Prepared service/image changes present
/// direct Yes/No controls; Ansible Configure retains the exact typed echo.
/// Nothing reaches the Bus until the operator confirms.
fn render_review_sheet(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    let Some(snapshot) = state.arming.as_ref() else {
        return;
    };
    let echo = snapshot.action.echo();
    let word = snapshot.action.confirm_word();
    let subject = snapshot.action.subject();
    let uses_yes_no_review = snapshot.action.uses_yes_no_review();
    let facts = review_sheet_facts(&snapshot.action, state);
    let mut confirm = false;
    let mut cancel = false;

    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(Style::STROKE_HAIRLINE, Style::DANGER))
        .corner_radius(Style::RADIUS_S)
        .show(ui, |ui| {
            let prompt = if uses_yes_no_review {
                format!(
                    "Review {subject}. Choose Yes to {} it, or No to leave it unchanged. \
                     Nothing is sent until you choose Yes.",
                    word.to_lowercase()
                )
            } else {
                format!(
                    "Review — type \u{201C}{echo}\u{201D} exactly to {} {subject}. Nothing is \
                     sent until it matches.",
                    word.to_lowercase()
                )
            };
            ui.label(
                RichText::new(prompt)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            ui.horizontal_wrapped(|ui| {
                field(ui, "Command", &facts.command, Style::DANGER);
                ui.add_space(Style::SP_M);
                field(ui, "Subject", &facts.subject, Style::TEXT);
                ui.add_space(Style::SP_M);
                field(ui, "Target", &facts.target, Style::TEXT);
                ui.add_space(Style::SP_M);
                field(ui, "Placement node", &facts.node, Style::ACCENT_WORKLOADS);
            });
            ui.horizontal_wrapped(|ui| {
                field(ui, "Body digest", &facts.body_digest, Style::TEXT);
                ui.add_space(Style::SP_M);
                field(ui, "Body summary", &facts.body_summary, Style::TEXT_DIM);
            });
            muted_note(ui, format!("Frozen body: {}", facts.body_preview));
            muted_note(ui, format!("Blast radius: {}", facts.impact));
            ui.add_space(Style::SP_XS);
            if uses_yes_no_review {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            RichText::new("Yes").size(Style::SMALL).color(Style::DANGER),
                        ))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("No").size(Style::SMALL)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            } else {
                let Some(arming) = state.arming.as_mut() else {
                    return;
                };
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut arming.typed)
                            .hint_text(echo.as_str())
                            .desired_width(Style::SP_XL * 5.0),
                    );
                    ui.add_space(Style::SP_S);
                    let is_armed = armed(&arming.typed, &echo);
                    if ui
                        .add_enabled(
                            is_armed,
                            egui::Button::new(
                                RichText::new(word).size(Style::SMALL).color(Style::DANGER),
                            ),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui
                        .add(egui::Button::new(
                            RichText::new("Cancel").size(Style::SMALL),
                        ))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            }
        });
    ui.add_space(Style::SP_S);

    if confirm {
        if let Some(arming) = state.arming.take() {
            let confirmation = if uses_yes_no_review {
                echo.as_str()
            } else {
                arming.typed.as_str()
            };
            state.perform(arming.action, confirmation);
        }
    } else if cancel {
        state.arming = None;
    }
}

// ─────────────────────────── shared panel-body helpers ──────────────────────

/// An honest one-line summary of the folded `state/cloud` mirror — the shared
/// context line the stub panels show so the seam's live data flow is visible
/// even before a panel's body lands.
pub(super) fn mirror_summary(ui: &mut egui::Ui, state: &WorkloadsState) {
    let nodes = state.states.len();
    let workloads: usize = state.states.iter().map(|s| s.workloads.len()).sum();
    let stale = state
        .states
        .iter()
        .filter(|state| !cloud_state_is_fresh(state))
        .count();
    let freshness = if stale == 0 {
        String::new()
    } else {
        format!(" · {stale} stale")
    };
    mde_egui::muted_note(
        ui,
        format!(
            "state/cloud mirror: {nodes} node(s) \u{00B7} {workloads} workload(s) folded{freshness}."
        ),
    );
}

/// The session audit trail — the workspace's honest record of the ops it
/// requested (verb · verdict · detail), newest first. An empty trail reads
/// honestly. The preserved audit machinery renders here (the Status lens's home);
/// U18 enriches the rest of the day-2 view around it.
pub(super) fn render_audit(ui: &mut egui::Ui, audit: &[AuditEntry]) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Audit trail (this session)")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    if audit.is_empty() {
        mde_egui::muted_note(ui, "No ops performed from this workspace yet.");
        return;
    }
    for entry in audit.iter().rev() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                entry.outcome.color(),
                RichText::new(format!("{} {}", entry.verb, entry.outcome.word()))
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("\u{2014} {}", entry.detail)).size(Style::SMALL),
            );
        });
    }
}

// ─────────────────────────── the seam module layout ─────────────────────────

mod android_apps;
mod android_governed;
mod menubar;

mod placement;
mod provision_form;

mod configure;
mod containers;
mod images;
mod status;

#[cfg(test)]
mod browser_vm_target_tests {
    use super::*;
    use mackes_mesh_types::cloud::{CloudProviderAdapter, DriftFlag, DriftSummary, NodeCapacity};

    const NOW_MS: i64 = 1_000_000;

    fn browser_vm(node: &str, status: &str, reachable: bool) -> WorkloadRow {
        WorkloadRow {
            name: "browser-vm".to_owned(),
            delivery_type: DeliveryType::DesktopVm,
            node: node.to_owned(),
            status: status.to_owned(),
            cpu_pct: 0,
            mem_mb: 0,
            disk_gb: 0,
            reachable,
            drift: DriftFlag::Unknown,
            app: None,
        }
    }

    fn cloud_state(host: &str, published_at_ms: i64, workload: WorkloadRow) -> CloudState {
        CloudState {
            host: host.to_owned(),
            role: DeploymentRole::Workstation,
            adapter: CloudProviderAdapter::ConstructCloud,
            health: Vec::new(),
            resources: Vec::new(),
            apply_armed: false,
            published_at_ms,
            workloads: vec![workload],
            drift_summary: DriftSummary::default(),
            node_capacity: NodeCapacity::default(),
            android_inventories: Vec::new(),
            android_provider_admissions: Vec::new(),
            android_vdi_sources: Vec::new(),
        }
    }

    #[test]
    fn ready_local_browser_vm_wins_over_an_earlier_stale_remote_duplicate() {
        let mut state = WorkloadsState::default();
        state.states = vec![
            cloud_state(
                "aaa-old",
                NOW_MS - CLOUD_MIRROR_STALE_AFTER_MS - 1,
                browser_vm("aaa-old", "active", true),
            ),
            cloud_state("dell", NOW_MS, browser_vm("dell", "active", true)),
        ];

        assert_eq!(
            state.browser_vm_target_at("dell", NOW_MS),
            Some(("dell", "browser-vm", "active", true))
        );
    }

    #[test]
    fn ready_remote_browser_vm_wins_over_an_unready_local_duplicate() {
        let mut state = WorkloadsState::default();
        state.states = vec![
            cloud_state("dell", NOW_MS, browser_vm("dell", "defined", false)),
            cloud_state("remote", NOW_MS, browser_vm("remote", "running", true)),
        ];

        assert_eq!(
            state.browser_vm_target_at("dell", NOW_MS),
            Some(("remote", "browser-vm", "running", true))
        );
    }

    #[test]
    fn unavailable_browser_vm_fallback_is_lexical_and_stale_is_not_reachable() {
        let stale = NOW_MS - CLOUD_MIRROR_STALE_AFTER_MS - 1;
        let mut state = WorkloadsState::default();
        state.states = vec![
            cloud_state("zeta", stale, browser_vm("zeta", "active", true)),
            cloud_state("alpha", stale, browser_vm("alpha", "active", true)),
        ];

        assert_eq!(
            state.browser_vm_target_at("no-local-row", NOW_MS),
            Some(("alpha", "browser-vm", "active", false))
        );
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests;
