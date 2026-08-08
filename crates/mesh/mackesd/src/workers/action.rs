//! FRONTDOOR-11 (backend half) — the mackesd typed **action worker** + audit.
//!
//! The Front Door's confirm-gate (design `docs/design/front-door.md`, Q17 + Q26)
//! says approved operator/Copilot actions run "**via a mackesd action worker
//! (typed, audited)**". This is the EXECUTION half of that: a worker that drains
//! a typed action-request topic, executes each request through an EXISTING typed
//! verb mechanism, writes a **hash-chain audit entry** for it (§8), and replies
//! with a typed result. The GUI confirm-gate / preview-diff UI is a separate
//! GUI-track task — there is no UI here.
//!
//! ## §9 — typed verbs only, NO raw shell channel, ever
//!
//! `AI_GOVERNANCE.md` §9 is load-bearing: "Remote execution is **typed verbs +
//! signed job bundles only — no raw shell channel, ever**." This worker therefore
//! does NOT accept a command string. It accepts a TYPED [`ActionRequest`] enum —
//! an allowlisted action KIND with typed params — and maps each allowlisted KIND
//! onto an EXISTING mackesd verb mechanism. The first cut allowlists exactly one:
//!
//! * [`ActionRequest::ServiceLifecycle`] → a signed Workload operation on
//!   [`mackes_mesh_types::workloads::WORKLOAD_OPERATION_TOPIC`]. The node-local
//!   `workload_compute` reconciler is the only VM/container actuator; this
//!   worker never writes a legacy lifecycle request or invokes a backend.
//!
//! * [`ActionRequest::AndroidAppLaunch`] → the admitted Android guest provider
//!   boundary. The request carries only a path-safe target, a bounded workload
//!   identity, and a closed [`AospStarterApp`] value; the provider receives the
//!   canonical [`AndroidGuestRequest::Launch`] envelope and returns a closed
//!   outcome. An unconfigured adapter returns `Unavailable` and is surfaced as
//!   a rejection — this worker never turns the typed request into a shell,
//!   arbitrary intent, or an unverified live-guest claim.
//!
//! * [`ActionRequest::CodeEdit`] (FRONTDOOR-12) → a typed, **path-bounded** file
//!   write + a FIXED-ARG `git commit`. This is the most sensitive AI capability
//!   (the AI editing code/config), so the safety model is non-negotiable and
//!   matches the others: it APPLIES **only** when an explicit operator-approved
//!   request carries a valid, exact-body, single-use capability (never
//!   auto-applied from a Copilot proposal — the Copilot emits a `CodeEdit`
//!   *proposal* on a DISTINCT topic and never publishes to [`ACTION_TOPIC`]). The
//!   target path is validated to be relative and traversal-free, then every
//!   parent is resolved beneath an anchored directory descriptor without
//!   following symlinks. An absolute path, `..` escape, or in-root symlink escape
//!   is a typed rejection (audited), bounding the blast radius. The apply is
//!   TYPED, not shell: a descriptor-relative atomic write of the reviewed
//!   content, then `Command::new("git")` with a CLOSED, FIXED arg vector
//!   (`add -- <validated-relpath>` then `commit -m <fixed-prefix+kind> -- <relpath>`)
//!   — the binary is a literal and the path is the validated in-root relpath, NOT
//!   a free-form command string (§9: no `Command::new(<user string>)`, no shell).
//!   The reviewable full content travels in the proposal so the operator sees the
//!   exact change before approving.
//!
//! An unknown / disallowed KIND, or one whose typed params fail the existing
//! vocabulary gate, is a typed **rejection** ([`ActionReply::rejected`]) — never a
//! panic, never a fallthrough to a generic executor (there is none).
//!
//! ## Audit — hash-chain, the existing plane (§8)
//!
//! Every executed action is recorded through the EXISTING hash-chained audit plane
//! ([`crate::events::append_and_alert`] → the `events` table, whose rows chain via
//! [`crate::audit::next_hash`] and are tamper-verified by `mackesd audit verify`).
//! We write one [`crate::events::EventKind::AdminAction`] row per request carrying
//! the action KIND, the typed params, the requesting node, and the dispatch
//! outcome. We do NOT invent a new audit format — this is the same chain the
//! reconcile/mesh-router writers append to.
//!
//! ## Leader-coordinated + graceful degrade (Q73 / Q33 / §2)
//!
//! Spawned on every node so failover is seamless, but LEADER-gated on the shared
//! `<workgroup_root>/.mackesd-leader.lock` (the same lock every other leader-gated
//! worker contends on): only the elected node dispatches + audits a request, so a
//! multi-node mesh executes + audits each action exactly once. A non-leader
//! advances its cursor and short-circuits without replying (the elected node
//! answers). Every failure path degrades to a typed reply + a log line — the
//! worker never panics, mirroring `copilot` / `dc_jobs`.

#![cfg(feature = "async-services")]

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use mackes_mesh_types::android_apps::{
    AndroidGuestLaunchOutcome, AndroidGuestRequest, AndroidGuestResponse, AospStarterApp,
};
use mackes_mesh_types::workloads::{
    workload_state_topic, WorkloadAttachmentProtocol, WorkloadBackend, WorkloadId,
    WorkloadOperationAction, WorkloadOperationRequest, WorkloadProfile, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    WORKLOAD_OPERATION_TOPIC,
};

use super::cloud::{
    claim_nonce, verify_token, AndroidGuestProvider, AndroidGuestProviderRegistry,
    AndroidGuestProviderRegistryError, HmacTokenSigner, NullSigner, TokenSigner, TokenVerdict,
    DEFAULT_AUTH_ROOT,
};
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
///
/// Locked to the canonical `action/<domain>/<verb>` RPC convention (`rpc.rs`,
/// which rejects any topic outside `action/`) so the workbench publishes via the
/// standard RPC caller. `exec` is the domain, `request` the verb.
pub const ACTION_TOPIC: &str = "action/exec/request";

/// Exact capability verb for every typed administrative execution request.
pub const EXEC_AUTH_VERB: &str = "exec-request";
/// Stable placement value for the leader-coordinated action plane.
pub const EXEC_AUTH_NODE: &str = "fleet-control";

/// Maximum remaining lifetime accepted for an administrative execution
/// capability. A valid signature must not turn into a long-lived bearer token.
const MAX_AUTH_TTL_MS: i64 = 30_000;

/// Poll cadence on the request topic. An action dispatch is local file I/O +
/// one audit insert (sub-millisecond), so a 400 ms poll keeps latency
/// imperceptible while bounding index-read churn (matches `copilot`).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// A TYPED action request — an allowlisted KIND with typed params. There is
/// deliberately **no `Command(String)` / `Shell(String)` variant**: §9 forbids a
/// raw-shell / arbitrary-command channel, so the only way to add an action is to
/// add a typed variant here backed by an existing verb mechanism.
///
/// `serde` tags the variant by `kind` so the wire form is
/// `{"kind":"service_lifecycle", ...typed params...}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionRequest {
    /// Start/stop/restart an EXISTING service (container or VM) on a target node,
    /// via the versioned Workload operation API. The service kind and operation
    /// are mapped to the closed Workload backend/action enums before publication.
    ServiceLifecycle {
        /// The node the action targets (its short hostname or canonical
        /// `peer:<hostname>` Workload target).
        target_host: String,
        /// `container` | `vm` — the typed service kind.
        service_kind: String,
        /// The container/guest name. NOT a command — the target validates it
        /// against its own live probe before acting (no arbitrary passthrough).
        name: String,
        /// `start` | `stop` | `restart` — the typed operation.
        op: String,
    },

    /// Launch one governed AOSP starter app in one admitted Android VM guest.
    /// The target and workload are bounded identities; `app` is a closed enum
    /// whose canonical package/action/category are reconstructed at the guest
    /// boundary. There is intentionally no package string, component, URI,
    /// command, intent extras, or other execution-shaped field.
    AndroidAppLaunch {
        /// The path-safe mesh node that owns the Android VM.
        target_host: String,
        /// The bounded Android VM workload identity.
        workload_id: String,
        /// The governed AOSP starter application identity.
        app: AospStarterApp,
    },

    /// FRONTDOOR-12 — apply a reviewed **code/config edit** to a single file
    /// inside the allowed root, then commit it with FIXED git args. This is the
    /// AI-editing-code capability: it lands ONLY on an exact-body, single-use
    /// operator capability (exactly like `ServiceLifecycle` — never auto-applied
    /// from a Copilot proposal). The `path` is validated to be a relative,
    /// traversal-free path and resolved beneath the allowed root without following
    /// symlinks before any write; `content` is
    /// the full reviewed file body the operator approved (the proposal carries it
    /// in full so the change is reviewable). There is deliberately no shell / diff
    /// / patch-program escape hatch — the worker writes the typed content and runs
    /// `git` with a closed arg vector (§9).
    CodeEdit {
        /// The edit target, as a path RELATIVE to the allowed root. Validated by
        /// [`validate_edit_path`]: rejected if absolute, if it contains a `..`
        /// component or a root/prefix component, or if it is empty. This bounds
        /// the blast radius to the workgroup/repo dir — `/etc`, `~`, and `../`
        /// escapes can never be written.
        path: String,
        /// The full, reviewed file content to write. This is what the operator saw
        /// and approved in the proposal — applied verbatim (a typed file write),
        /// never interpreted as a command or a patch program.
        content: String,
    },
}

impl ActionRequest {
    /// Stable kind tag for logs + the audit record (matches the serde tag).
    #[must_use]
    pub const fn kind_tag(&self) -> &'static str {
        match self {
            ActionRequest::ServiceLifecycle { .. } => "service_lifecycle",
            ActionRequest::AndroidAppLaunch { .. } => "android_app_launch",
            ActionRequest::CodeEdit { .. } => "code_edit",
        }
    }
}

/// The commit-message prefix every applied [`ActionRequest::CodeEdit`] carries.
/// Fixed (not operator/AI-supplied) so the `git commit -m` arg is a constant — the
/// only variable part is the validated in-root relpath, appended by the worker.
const CODE_EDIT_COMMIT_PREFIX: &str = "mackesd code-edit (FD-12, operator-approved):";

/// Validate a [`ActionRequest::CodeEdit`] target path and resolve it to the
/// absolute on-disk path INSIDE `root`.
///
/// Pure + unit-testable: this is the path-bound enforcement that runs BEFORE any
/// write. The contract is strict — the path must be:
///
/// * non-empty,
/// * **relative** (an absolute path or a Windows-style prefix/root is rejected —
///   no `/etc`, no drive roots),
/// * free of any `..` (`ParentDir`) component (no traversal escape),
/// * composed only of plain `Normal` components (no bare `.` cur-dir, no root).
///
/// On success it returns `root.join(rel)` — guaranteed lexically within `root`.
/// On any violation it returns a typed rejection reason. This is the pure lexical
/// half; [`write_code_edit_beneath`] separately walks every parent through
/// `openat(O_NOFOLLOW)` so an in-root symlink cannot escape the boundary.
///
/// # Errors
///
/// A human-readable reason suitable for an [`ActionReply`]'s `error` field.
pub fn validate_edit_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("code_edit: empty path".to_string());
    }
    let rel = Path::new(path);
    // Reject anything that is not a pure sequence of normal path segments. This
    // single pass catches absolute paths, `..` traversal, bare `.`, and any
    // root/prefix component — the union of every escape we must bound.
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(format!(
                    "code_edit: path `{path}` contains a `..` traversal component (rejected)"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "code_edit: path `{path}` is absolute / has a root component (must be relative to the allowed root)"
                ));
            }
            Component::CurDir => {
                return Err(format!(
                    "code_edit: path `{path}` contains a `.` component (rejected)"
                ));
            }
        }
    }
    let joined = root.join(rel);
    // Belt-and-suspenders: the joined path must still start with the root prefix.
    // A components-only relative join can't escape, but this makes the invariant
    // explicit and survives any future change to the loop above.
    if !joined.starts_with(root) {
        return Err(format!(
            "code_edit: path `{path}` resolves outside the allowed root (rejected)"
        ));
    }
    Ok(joined)
}

/// Atomically write one relative code-edit path beneath an already-existing
/// root without following symlinks in any component. Directory file descriptors
/// keep resolution anchored even if a replicated peer races a rename; the final
/// temp file is `O_EXCL|O_NOFOLLOW`, synced, renamed within the same parent, and
/// the parent directory is synced before success returns.
fn write_code_edit_beneath(root: &Path, path: &str, content: &str) -> Result<(), String> {
    use rand::RngCore as _;
    use rustix::fs::{AtFlags, Mode, OFlags};
    use std::ffi::OsString;
    use std::io::Write as _;

    validate_edit_path(root, path)?;
    let mut components: Vec<OsString> = Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect();
    let file_name = components
        .pop()
        .ok_or_else(|| "code_edit: empty path".to_string())?;
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open(root, directory_flags, Mode::empty())
        .map_err(|error| format!("code_edit: open allowed root failed: {error}"))?;
    for component in components {
        directory =
            match rustix::fs::openat(&directory, &component, directory_flags, Mode::empty()) {
                Ok(next) => next,
                Err(rustix::io::Errno::NOENT) => {
                    match rustix::fs::mkdirat(
                        &directory,
                        &component,
                        Mode::RUSR | Mode::WUSR | Mode::XUSR,
                    ) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(error) => {
                            return Err(format!(
                                "code_edit: create path component {:?} failed: {error}",
                                component
                            ));
                        }
                    }
                    rustix::fs::openat(&directory, &component, directory_flags, Mode::empty())
                        .map_err(|error| {
                            format!(
                                "code_edit: secure path component {:?} failed: {error}",
                                component
                            )
                        })?
                }
                Err(error) => {
                    return Err(format!(
                        "code_edit: path component {:?} is not a safe directory: {error}",
                        component
                    ));
                }
            };
    }

    let mut random = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let temp_name = format!(
        ".mde-codeedit-{}.tmp",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let file_flags =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let temp = rustix::fs::openat(
        &directory,
        temp_name.as_str(),
        file_flags,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| format!("code_edit: create secure temporary file failed: {error}"))?;
    let mut temp_file: std::fs::File = temp.into();
    if let Err(error) = temp_file
        .write_all(content.as_bytes())
        .and_then(|()| temp_file.sync_all())
    {
        drop(temp_file);
        let _ = rustix::fs::unlinkat(&directory, temp_name.as_str(), AtFlags::empty());
        return Err(format!("code_edit: persist temporary file failed: {error}"));
    }
    drop(temp_file);
    if let Err(error) = rustix::fs::renameat(
        &directory,
        temp_name.as_str(),
        &directory,
        file_name.as_os_str(),
    ) {
        let _ = rustix::fs::unlinkat(&directory, temp_name.as_str(), AtFlags::empty());
        return Err(format!("code_edit: finalize `{path}` failed: {error}"));
    }
    let directory_file: std::fs::File = directory.into();
    directory_file
        .sync_all()
        .map_err(|error| format!("code_edit: sync parent of `{path}` failed: {error}"))
}

/// The typed reply published to `reply/<request-ulid>`.
///
/// `ok` mirrors the `dc/*` reply convention (`{"ok":true}` ⇒ success) so the
/// existing `dc_jobs` status classifier and any tile reads it uniformly.
/// `detail` carries a human-readable note on success; `error` is set (and `ok`
/// is false) on a rejection / dispatch-failure degrade path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActionReply {
    /// `true` once the typed action was accepted + dispatched through the verb
    /// mechanism (the target then executes + reports via its own result lane).
    pub ok: bool,
    /// Human-readable success note (e.g. "dispatched container restart to oak").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Why the action was rejected / could not be dispatched, on a degrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionReply {
    /// An accepted + dispatched reply.
    #[must_use]
    pub fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: Some(detail.into()),
            error: None,
        }
    }

    /// A typed rejection / degrade reply (unknown KIND, vocabulary violation,
    /// malformed body, or a dispatch I/O fault). Never a panic.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: None,
            error: Some(reason.into()),
        }
    }

    /// JSON body for the `reply/<ulid>` lane. Infallible — a serialize failure
    /// (impossible for this shape) degrades to a fixed rejection body.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"reply encode failed"}"#.to_string())
    }
}

/// Parse a typed action-request JSON body.
///
/// # Errors
///
/// Any serde-json failure surfaces as a `"malformed action request: …"` string
/// suitable for the `error` field of an [`ActionReply`]. A body that is valid
/// JSON but carries an unknown `kind` tag fails here too (serde rejects the
/// untagged variant) — so an un-allowlisted KIND can never reach a dispatcher.
pub fn parse_action_request(body: &str) -> Result<ActionRequest, String> {
    // `schema_version` and `armed_token` belong to the execution envelope, not
    // to the typed action kind. Strip only those two envelope fields before the
    // closed enum deserializer runs; every other unknown field is rejected by
    // `deny_unknown_fields` (including command/intent-shaped additions to the
    // Android launch variant).
    let mut value = serde_json::from_str::<serde_json::Value>(body)
        .map_err(|e| format!("malformed action request: {e}"))?;
    let Some(object) = value.as_object_mut() else {
        return Err("malformed action request: expected a JSON object with a kind tag".to_string());
    };
    object.remove("schema_version");
    object.remove("armed_token");
    serde_json::from_value(value).map_err(|e| format!("malformed action request: {e}"))
}

/// Build the closed Workload operation for one service-lifecycle action.
///
/// The action surface predates the Workload API and therefore does not carry
/// resource values or a generation. We use the safe small profile for a first
/// registration, then reuse the authoritative node projection when it already
/// contains this workload. A missing image is intentionally left absent: the
/// Workload actuator will reject an attempt to define an unknown VM rather than
/// guessing at a host path.
fn plan_workload_service_lifecycle(
    req: &ActionRequest,
    request_id: &str,
    state: Option<&mackes_mesh_types::workloads::WorkloadOperationStatus>,
    now_ms: u64,
) -> Result<WorkloadOperationRequest, String> {
    let ActionRequest::ServiceLifecycle {
        target_host,
        service_kind,
        name,
        op,
    } = req
    else {
        return Err("service_lifecycle: request kind mismatch".to_string());
    };
    let target_host = target_host.trim();
    let name = name.trim();
    if target_host.is_empty() {
        return Err("service_lifecycle: empty target_host".to_string());
    }
    if name.is_empty() {
        return Err("service_lifecycle: empty name".to_string());
    }
    let (backend, kind_prefix) = match service_kind.as_str() {
        "container" => (WorkloadBackend::QuadletSystemd, "container"),
        "vm" => (WorkloadBackend::LibvirtVirtqemud, "vm"),
        _ => {
            return Err(format!(
                "service_lifecycle: kind `{service_kind}` not allowlisted (container|vm)"
            ))
        }
    };
    let action = match op.as_str() {
        "start" => WorkloadOperationAction::Start,
        "stop" => WorkloadOperationAction::Stop,
        "restart" => WorkloadOperationAction::Restart,
        _ => {
            return Err(format!(
                "service_lifecycle: op `{op}` not allowlisted (start|stop|restart)"
            ))
        }
    };
    let target_node = if target_host.starts_with("peer:") {
        target_host.to_owned()
    } else {
        format!("peer:{target_host}")
    };
    let workload_id = WorkloadId::new(format!("{kind_prefix}:{target_node}:{name}"))
        .map_err(|error| format!("service_lifecycle: invalid workload identity: {error}"))?;
    let (resources, expected_generation, image_ref) = state
        .filter(|status| status.workload_id == workload_id && status.backend == backend)
        .map(|status| {
            (
                status.resources,
                status.generation,
                None::<String>,
            )
        })
        .unwrap_or((WorkloadProfile::Small.resources(), 0, None));
    let deadline_at_ms = now_ms
        .saturating_add(MAX_AUTH_TTL_MS as u64)
        .saturating_sub(250);
    Ok(WorkloadOperationRequest {
        schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
        request_id: request_id.to_owned(),
        workload_id,
        backend,
        resources,
        image_ref,
        target_node,
        expected_generation,
        action,
        target_request_id: None,
        deadline_at_ms,
        preferred_attachment: Some(WorkloadAttachmentProtocol::Logs),
        armed_token: None,
    })
}

fn workload_action_label(action: WorkloadOperationAction) -> &'static str {
    match action {
        WorkloadOperationAction::StartAndAttach => "start_and_attach",
        WorkloadOperationAction::Start => "start",
        WorkloadOperationAction::Stop => "stop",
        WorkloadOperationAction::Restart => "restart",
        WorkloadOperationAction::Destroy => "destroy",
        WorkloadOperationAction::Pause => "pause",
        WorkloadOperationAction::Resume => "resume",
        WorkloadOperationAction::Open => "open",
        WorkloadOperationAction::Reconcile => "reconcile",
        WorkloadOperationAction::Cancel => "cancel",
    }
}

/// Maximum target-host identity size accepted by the Android action. This is a
/// filesystem-safe component bound, matching the platform's cloud path keys.
const MAX_ANDROID_TARGET_HOST_BYTES: usize = 255;

/// Validate and construct the canonical Android guest launch request for an
/// admitted action. The target host is kept separately because it selects the
/// owning mesh node; the guest boundary validates the workload/request ids and
/// reconstructs the canonical launcher intent from the closed app enum.
pub fn plan_android_app_launch(
    req: &ActionRequest,
    request_id: &str,
) -> Result<AndroidGuestRequest, String> {
    let ActionRequest::AndroidAppLaunch {
        target_host,
        workload_id,
        app,
    } = req
    else {
        return Err("plan_android_app_launch: not an android_app_launch request".to_string());
    };
    if !is_safe_android_target_host(target_host) {
        return Err(
            "android_app_launch: target_host must be one path-safe [A-Za-z0-9._-] segment"
                .to_string(),
        );
    }
    AndroidGuestRequest::launch(request_id, workload_id, *app).map_err(|error| {
        format!("android_app_launch: guest request identity is invalid: {error:?}")
    })
}

fn is_safe_android_target_host(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.trim() == value
        && value.len() <= MAX_ANDROID_TARGET_HOST_BYTES
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

/// Stable semantic target for one administrative capability. The capability's
/// request digest independently binds every byte-significant JSON field.
fn action_authorization_target(req: &ActionRequest) -> String {
    match req {
        ActionRequest::ServiceLifecycle {
            target_host,
            service_kind,
            name,
            op,
        } => format!("service:{target_host}:{service_kind}:{name}:{op}"),
        ActionRequest::AndroidAppLaunch {
            target_host,
            workload_id,
            app,
        } => format!(
            "android-app:{target_host}:{workload_id}:{}",
            app.package_id().as_str()
        ),
        ActionRequest::CodeEdit { path, .. } => format!("code:{path}"),
    }
}

fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The leader-only typed action worker. Drains [`ACTION_TOPIC`], dispatches each
/// allowlisted typed action through the existing verb mechanism, audits it on the
/// hash-chain plane, and replies. Best-effort + graceful degrade.
pub struct ActionWorker {
    /// Shared leader lock (`<workgroup_root>/.mackesd-leader.lock`) — the same
    /// file every leader-gated worker contends on.
    leader_lock: PathBuf,
    /// Workgroup root — the replicated volume used for the leader lock, audit,
    /// and typed action support files. Workload operations use the Bus root.
    workgroup_root: PathBuf,
    /// This node's id (the lease holder + the `from`/actor on audit records).
    node_id: String,
    /// Root-credential verifier. Missing credentials install [`NullSigner`] and
    /// make every administrative mutation fail closed.
    signer: Arc<dyn TokenSigner>,
    /// The same root credential, retained as a mint-capable signer for the
    /// internal handoff from the authenticated action envelope to the typed
    /// Workload operation envelope. Missing credentials leave this `None` and
    /// the handoff fails closed.
    workload_signer: Option<Arc<HmacTokenSigner>>,
    /// Shared host-local spent-nonce ledger used by every privileged action lane.
    auth_root: PathBuf,
    /// The hash-chained audit DB (the `events` table). Defaults to
    /// [`crate::default_db_path`]; tests point it at a tempdir.
    db_path: PathBuf,
    /// Request topic poll cadence.
    poll_interval: Duration,
    /// Override the Bus spool root. Tests point this at a tempdir.
    bus_root_override: Option<PathBuf>,
    /// Bounded, workload-identity-keyed Android guest provider adapters. An
    /// empty registry is the honest production default: inventory stays
    /// pending and launches remain explicitly unavailable until a real guest
    /// adapter is configured by startup wiring.
    android_guest_providers: AndroidGuestProviderRegistry,
}

impl ActionWorker {
    /// Construct with production defaults: the shared leader lock under
    /// `workgroup_root`, the canonical audit DB path, the default Bus root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        let workload_signer = match HmacTokenSigner::from_systemd_credential() {
            Ok(signer) => Some(Arc::new(signer)),
            Err(error) => {
                tracing::error!(
                    target: "mackesd::action",
                    %error,
                    "typed administrative authorization unavailable; actions are disabled"
                );
                None
            }
        };
        let signer: Arc<dyn TokenSigner> = workload_signer
            .as_ref()
            .map(|signer| Arc::clone(signer) as Arc<dyn TokenSigner>)
            .unwrap_or_else(|| Arc::new(NullSigner));
        Self {
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            workgroup_root,
            node_id,
            signer,
            workload_signer,
            auth_root: PathBuf::from(DEFAULT_AUTH_ROOT),
            db_path: crate::default_db_path(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            android_guest_providers: AndroidGuestProviderRegistry::new(),
        }
    }

    /// Override the audit DB path. Tests point this at a tempdir so the
    /// hash-chain insert is exercised without touching `/var/lib/mde`.
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the Bus spool root. Tests point this at a tempdir.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence. Tests use a shorter value.
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Register one workload-scoped Android guest provider during startup or
    /// test construction. Duplicate, invalid, and over-capacity identities
    /// are returned as typed errors before the worker can run.
    pub(crate) fn with_android_guest_provider(
        mut self,
        workload_id: impl Into<String>,
        provider: Arc<dyn AndroidGuestProvider>,
    ) -> Result<Self, AndroidGuestProviderRegistryError> {
        self.android_guest_providers
            .register(workload_id, provider)?;
        Ok(self)
    }

    /// Inject deterministic verifier/ledger state for hostile request tests.
    #[cfg(test)]
    #[must_use]
    fn with_authorization(mut self, signer: Arc<dyn TokenSigner>, root: PathBuf) -> Self {
        self.signer = signer;
        self.workload_signer = None;
        self.auth_root = root;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_workload_signer(mut self, signer: Arc<HmacTokenSigner>) -> Self {
        self.workload_signer = Some(signer);
        self
    }

    /// Only the directory leader executes actions (Q73; no-fixed-center: any
    /// eligible node can be it, the elected one acts). Reuses the shared leader
    /// lock — synchronous, called once per observed request.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    /// Write the hash-chain audit row for one action (request + outcome) through
    /// the EXISTING audit plane. Best-effort: `append_and_alert` logs + swallows
    /// any store fault, so an audit-write hiccup never wedges the action lane.
    fn audit(&self, kind_tag: &str, req_summary: serde_json::Value, outcome: &ActionReply) {
        let detail = serde_json::json!({
            "action": kind_tag,
            "request": req_summary,
            "ok": outcome.ok,
            "detail": outcome.detail,
            "error": outcome.error,
        });
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            detail,
        );
    }

    /// A compact JSON summary of a typed request for the audit record (the typed
    /// params, not a command — there is no command).
    fn request_summary(req: &ActionRequest) -> serde_json::Value {
        match req {
            ActionRequest::ServiceLifecycle {
                target_host,
                service_kind,
                name,
                op,
            } => serde_json::json!({
                "target_host": target_host,
                "service_kind": service_kind,
                "name": name,
                "op": op,
            }),
            ActionRequest::AndroidAppLaunch {
                target_host,
                workload_id,
                app,
            } => serde_json::json!({
                "target_host": target_host,
                "workload_id": workload_id,
                "app": app.package_id().as_str(),
                "intent": "canonical_main_launcher",
            }),
            ActionRequest::CodeEdit { path, content } => serde_json::json!({
                // The full content is recorded — the audit IS the durable record of
                // exactly what was applied (§8) — alongside the path and a size so
                // the trail is greppable without re-reading the file.
                "path": path,
                "content_len": content.len(),
                "content": content,
            }),
        }
    }

    /// Verify and durably consume the exact-body capability before a typed action
    /// can reach its dispatcher. This is the public-Bus security boundary; UI
    /// confirmation alone is never treated as authority.
    fn authorize_wire_request(&self, body: &str, req: &ActionRequest) -> TokenVerdict {
        let token = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("armed_token")?.as_str().map(str::to_string));
        let target = action_authorization_target(req);
        let now = wall_now_ms();
        let verdict = verify_token(
            token.as_deref(),
            EXEC_AUTH_VERB,
            EXEC_AUTH_NODE,
            &target,
            body,
            now,
            self.signer.as_ref(),
        );
        if !verdict.is_valid() {
            return verdict;
        }
        let Some(token) = token
            .as_deref()
            .and_then(mackes_mesh_types::cloud::CloudArmedToken::parse)
        else {
            return TokenVerdict::Malformed;
        };
        if token.expires_at_ms > now.saturating_add(MAX_AUTH_TTL_MS) {
            return TokenVerdict::LifetimeTooLong;
        }
        match claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms, now) {
            Ok(true) => TokenVerdict::Valid,
            Ok(false) => TokenVerdict::Replayed,
            Err(_) => TokenVerdict::ReplayStoreUnavailable,
        }
    }

    /// Public wire entry: bound body cap → typed parse → capability consume →
    /// allowlisted dispatcher. Unauthorized bodies are audited without copying a
    /// caller-controlled code payload into the audit database.
    fn handle_wire_action(&self, ulid: &str, body: &str) -> ActionReply {
        if !crate::ipc::body_within_cap(Some(body)) {
            let reply = ActionReply::rejected("typed action request body exceeds the 64 KiB cap");
            self.audit(
                "unknown",
                serde_json::json!({ "authorization": "refused" }),
                &reply,
            );
            return reply;
        }
        let envelope = match serde_json::from_str::<serde_json::Value>(body) {
            Ok(value) => value,
            Err(_) => return self.handle_action(ulid, body),
        };
        if envelope
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            let reply =
                ActionReply::rejected("typed action requires schema_version 1 — nothing changed");
            self.audit(
                "unknown",
                serde_json::json!({ "authorization": "refused" }),
                &reply,
            );
            return reply;
        }
        let req = match parse_action_request(body) {
            Ok(req) => req,
            Err(_) => return self.handle_action(ulid, body),
        };
        let verdict = self.authorize_wire_request(body, &req);
        if !verdict.is_valid() {
            let reply = ActionReply::rejected(format!(
                "typed action is not authorized ({}) — nothing changed",
                verdict.reason()
            ));
            self.audit(
                req.kind_tag(),
                serde_json::json!({ "authorization": "refused" }),
                &reply,
            );
            return reply;
        }
        self.handle_action(ulid, body)
    }

    /// Post-authorization typed action handler: parse → validate against the
    /// existing allowlist → dispatch via the existing verb mechanism → audit.
    /// Direct calls exist only in focused pure/dispatcher tests; production Bus
    /// traffic always enters through [`Self::handle_wire_action`].
    fn handle_action(&self, ulid: &str, body: &str) -> ActionReply {
        let req = match parse_action_request(body) {
            Ok(r) => r,
            Err(e) => {
                // Un-allowlisted / malformed: a typed rejection, audited as a
                // refused admin action. No dispatcher is ever reached.
                let reply = ActionReply::rejected(e);
                self.audit("unknown", serde_json::json!({ "raw": "rejected" }), &reply);
                return reply;
            }
        };
        let kind_tag = req.kind_tag();
        let summary = Self::request_summary(&req);
        let reply = self.dispatch(ulid, &req);
        self.audit(kind_tag, summary, &reply);
        reply
    }

    /// Re-arm an already authenticated action as a short-lived Workload API
    /// capability. The incoming action token is consumed by
    /// `handle_wire_action`; this handoff gets a fresh nonce and binds the
    /// exact unsigned Workload body before it is published.
    fn arm_workload_request(
        &self,
        request: &WorkloadOperationRequest,
        now_ms: u64,
    ) -> Result<String, String> {
        let signer = self
            .workload_signer
            .as_ref()
            .ok_or_else(|| "Workload capability minting is unavailable".to_string())?;
        let unsigned = serde_json::to_string(request)
            .map_err(|error| format!("encode Workload operation: {error}"))?;
        let digest = mackes_mesh_types::cloud::cloud_request_digest(&unsigned)
            .map_err(|error| format!("digest Workload operation: {error}"))?;
        let expires_at_ms = i64::try_from(
            request
                .deadline_at_ms
                .min(now_ms.saturating_add(MAX_AUTH_TTL_MS as u64)),
        )
        .unwrap_or(i64::MAX);
        let token = mackes_mesh_types::cloud::CloudArmedToken::mint(
            signer.as_ref(),
            &format!("action-workload-{}", request.request_id),
            expires_at_ms,
            crate::workers::workload_compute::AUTH_VERB,
            &request.target_node,
            &format!("workload:{}", request.workload_id.as_str()),
            &digest,
        )
        .encode();
        let mut value: serde_json::Value = serde_json::from_str(&unsigned)
            .map_err(|error| format!("encode Workload envelope: {error}"))?;
        value["armed_token"] = serde_json::Value::String(token);
        serde_json::to_string(&value)
            .map_err(|error| format!("serialize Workload operation: {error}"))
    }

    /// Map an allowlisted typed request onto its typed mechanism and dispatch
    /// it. `ServiceLifecycle` publishes one signed Workload operation (no
    /// backend call). `CodeEdit` applies a path-bounded typed file write + a
    /// FIXED-ARG `git commit` (the only spawned process, a literal binary with a
    /// closed arg vector — never a shell or a command string, §9). A vocabulary
    /// violation, an out-of-bounds path, or an I/O fault becomes a typed rejection.
    fn dispatch(&self, ulid: &str, req: &ActionRequest) -> ActionReply {
        match req {
            ActionRequest::ServiceLifecycle { target_host, .. } => {
                let root = self
                    .bus_root_override
                    .clone()
                    .or_else(default_bus_root)
                    .ok_or_else(|| "no Bus root is configured for Workload operations".to_string());
                let root = match root {
                    Ok(root) => root,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                let persist = match Persist::open(root) {
                    Ok(persist) => persist,
                    Err(error) => {
                        return ActionReply::rejected(format!(
                            "service_lifecycle: open Workload Bus failed: {error}"
                        ))
                    }
                };
                let now_ms = u64::try_from(wall_now_ms()).unwrap_or(0);
                let seed = match plan_workload_service_lifecycle(req, ulid, None, now_ms) {
                    Ok(request) => request,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                let state = persist
                    .read_latest(&workload_state_topic(&seed.target_node))
                    .ok()
                    .flatten()
                    .and_then(|message| message.body)
                    .and_then(|body| {
                        serde_json::from_str::<mackes_mesh_types::workloads::WorkloadStateSnapshot>(
                            &body,
                        )
                        .ok()
                    })
                    .and_then(|snapshot| {
                        snapshot.validate(now_ms).ok()?;
                        snapshot
                            .workloads
                            .into_iter()
                            .find(|status| status.workload_id == seed.workload_id)
                    });
                let request = match plan_workload_service_lifecycle(req, ulid, state.as_ref(), now_ms) {
                    Ok(request) => request,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                let body = match self.arm_workload_request(&request, now_ms) {
                    Ok(body) => body,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                match persist.write(
                    WORKLOAD_OPERATION_TOPIC,
                    Priority::Default,
                    Some("Workload service lifecycle"),
                    Some(&body),
                ) {
                    Ok(_) => ActionReply::ok(format!(
                        "queued Workload {} `{}` on {} (request {})",
                        workload_action_label(request.action),
                        request.workload_id.as_str(),
                        target_host,
                        request.request_id
                    )),
                    Err(error) => ActionReply::rejected(format!(
                        "service_lifecycle: publish Workload operation failed: {error}"
                    )),
                }
            }
            ActionRequest::AndroidAppLaunch {
                target_host,
                workload_id,
                app,
            } => {
                let request = match plan_android_app_launch(req, ulid) {
                    Ok(request) => request,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                let response = match self.android_guest_providers.dispatch(request) {
                    Ok(response) => response,
                    Err(error) => {
                        return ActionReply::rejected(format!(
                            "android_app_launch: provider boundary rejected the request: {error:?}"
                        ));
                    }
                };
                let AndroidGuestResponse::Launch(response) = response else {
                    return ActionReply::rejected(
                        "android_app_launch: provider returned the wrong response operation",
                    );
                };
                match response.outcome {
                    AndroidGuestLaunchOutcome::Started => ActionReply::ok(format!(
                        "admitted Android app `{}` launch for `{workload_id}` on `{target_host}`",
                        app.package_id().as_str()
                    )),
                    AndroidGuestLaunchOutcome::AlreadyRunning => ActionReply::ok(format!(
                        "Android app `{}` is already running for `{workload_id}` on `{target_host}`",
                        app.package_id().as_str()
                    )),
                    AndroidGuestLaunchOutcome::Unavailable => ActionReply::rejected(
                        "android_app_launch: provider unavailable; no guest launch was claimed",
                    ),
                    AndroidGuestLaunchOutcome::Rejected => ActionReply::rejected(
                        "android_app_launch: provider rejected the launch; no guest launch was claimed",
                    ),
                }
            }
            ActionRequest::CodeEdit { path, content } => self.apply_code_edit(path, content),
        }
    }

    /// Apply an operator-approved [`ActionRequest::CodeEdit`] (FRONTDOOR-12): a
    /// path-bounded, TYPED file write of the reviewed content, then a FIXED-ARG
    /// `git` commit. Reaching here means the wire entry already consumed the
    /// exact-body operator capability — a Copilot *proposal* never lands here.
    /// Every failure (out-of-bounds path, traversal, symlink, write fault, commit
    /// fault) is a typed rejection; `handle_action` audits whatever this returns,
    /// so an apply AND a rejection both write a hash-chain row.
    ///
    /// §9: no shell, no `Command::new(<user string>)`. The only spawned process is
    /// `git` (a literal binary) with a CLOSED arg vector whose only variable is the
    /// validated in-root relpath; the commit message is the fixed
    /// [`CODE_EDIT_COMMIT_PREFIX`] plus the kind tag.
    fn apply_code_edit(&self, path: &str, content: &str) -> ActionReply {
        // 1–2. PATH BOUND + TYPED WRITE — validate lexically, then walk/create
        // parents relative to the allowed-root descriptor with O_NOFOLLOW. The
        // sibling temporary is exclusive, synced, atomically renamed, and the
        // parent is synced before returning.
        if let Err(reason) = write_code_edit_beneath(&self.workgroup_root, path, content) {
            return ActionReply::rejected(reason);
        }
        // 3. FIXED-ARG GIT COMMIT — stage the one validated relpath, then commit it
        //    with a fixed message. The binary is the literal "git"; the only
        //    variable arg is `path` (already validated in-root). `--` fences the
        //    pathspec so a leading-dash path can't be read as a flag.
        if let Err(reason) = self.git_commit_edit(path) {
            // The file is written but not committed — surface it as a rejection so
            // the operator/audit see the commit didn't land (the write is recorded
            // in the audit summary regardless).
            return ActionReply::rejected(reason);
        }
        ActionReply::ok(format!("applied + committed code edit to `{path}`"))
    }

    /// Stage + commit the single validated relpath with FIXED git args. Returns a
    /// typed rejection reason on any non-zero/spawn failure. §9: `git` is a
    /// literal binary, the arg vector is closed (`add`/`commit`/`-m`/`--`), and the
    /// only data values are the validated relpath + the fixed commit message —
    /// there is no shell and no command string from the request.
    fn git_commit_edit(&self, rel_path: &str) -> Result<(), String> {
        let run = |args: &[&str]| -> Result<(), String> {
            let out = std::process::Command::new("git")
                .current_dir(&self.workgroup_root)
                .args(args)
                .output()
                .map_err(|e| format!("code_edit: `git {}` spawn failed: {e}", args.join(" ")))?;
            if out.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(format!(
                    "code_edit: `git {}` failed: {}",
                    args.join(" "),
                    stderr.trim()
                ))
            }
        };
        run(&["add", "--", rel_path])?;
        let message = format!("{CODE_EDIT_COMMIT_PREFIX} {rel_path}");
        run(&["commit", "-m", &message, "--", rel_path])
    }
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

#[async_trait::async_trait]
impl Worker for ActionWorker {
    fn name(&self) -> &'static str {
        "action"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::action", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::action",
                    error = %e,
                    "persist open failed; worker idle",
                );
                return Ok(());
            }
        };
        // Seed the cursor at the tail so a restart doesn't replay + re-execute
        // stale action requests (running an old action twice is worse than
        // dropping it on a restart).
        let mut cursor: Option<String> = persist.latest_ulid(ACTION_TOPIC).ok().flatten();
        let mut tick = tokio::time::interval(self.poll_interval);
        // Burn the immediate first tick so we wait a full interval on startup.
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // Handling is fully synchronous (a typed file write + a
                    // synchronous audit insert — no `.await`), so unlike copilot
                    // there is no async phase and the `&Persist` borrow is held
                    // for the whole sweep without breaking `Send`.
                    self.sweep(&persist, &mut cursor);
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

impl ActionWorker {
    /// One poll pass: read net-new requests since `cursor`, advance the cursor,
    /// and — only on the elected leader — handle + reply to each. A non-leader
    /// advances the cursor and replies to nothing (the elected node acts), so
    /// failover is seamless without double-execution.
    fn sweep(&self, persist: &Persist, cursor: &mut Option<String>) {
        let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(target: "mackesd::action", error = %e, "list_since failed");
                return;
            }
        };
        for msg in msgs {
            *cursor = Some(msg.ulid.clone());
            if !self.is_leader() {
                tracing::debug!(
                    target: "mackesd::action",
                    ulid = %msg.ulid,
                    "not the leader; skipping action (the elected node executes)",
                );
                continue;
            }
            let body = msg.body.unwrap_or_default();
            let reply = self.handle_wire_action(&msg.ulid, &body);
            // PERF-8: one INFO line per drained action scales with every user
            // interaction and is redundant telemetry — the reply body is durably
            // persisted to the reply topic immediately below. DEBUG keeps it opt-in;
            // the failure path stays at WARN.
            tracing::debug!(
                target: "mackesd::action",
                ulid = %msg.ulid,
                ok = reply.ok,
                "action handled",
            );
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply.to_body()),
            ) {
                tracing::warn!(
                    target: "mackesd::action",
                    ulid = %msg.ulid,
                    error = %e,
                    "reply write failed",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mackes_mesh_types::android_apps::{
        AndroidAppInventory, AndroidGuestInventoryRequest, AndroidGuestLaunchRequest,
    };

    use super::super::cloud::AndroidGuestProvider;

    fn lifecycle_req(target: &str, kind: &str, name: &str, op: &str) -> String {
        serde_json::json!({
            "schema_version": 1,
            "kind": "service_lifecycle",
            "target_host": target,
            "service_kind": kind,
            "name": name,
            "op": op,
        })
        .to_string()
    }

    fn read_workload_operation(tmp: &std::path::Path) -> Option<WorkloadOperationRequest> {
        let persist = Persist::open(tmp.join("bus")).ok()?;
        let body = persist
            .read_latest(WORKLOAD_OPERATION_TOPIC)
            .ok()??
            .body?;
        let now = u64::try_from(wall_now_ms()).ok()?;
        WorkloadOperationRequest::from_json(&body, now).ok()
    }

    fn android_launch_req(target: &str, workload: &str, app: &str) -> String {
        serde_json::json!({
            "schema_version": 1,
            "kind": "android_app_launch",
            "target_host": target,
            "workload_id": workload,
            "app": app,
        })
        .to_string()
    }

    struct ReadyActionProvider;

    impl AndroidGuestProvider for ReadyActionProvider {
        fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
            AndroidAppInventory::pending(request.workload_id.clone())
        }

        fn launch(&self, _request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
            AndroidGuestLaunchOutcome::Started
        }
    }

    fn armed_wire_with_ttl(
        body: &str,
        nonce: &str,
        signer: &HmacTokenSigner,
        ttl_ms: i64,
    ) -> String {
        let request = parse_action_request(body).expect("typed request");
        let token = mackes_mesh_types::cloud::CloudArmedToken::mint(
            signer,
            nonce,
            wall_now_ms().saturating_add(ttl_ms),
            EXEC_AUTH_VERB,
            EXEC_AUTH_NODE,
            &action_authorization_target(&request),
            &mackes_mesh_types::cloud::cloud_request_digest(body).expect("request digest"),
        )
        .encode();
        let mut value: serde_json::Value = serde_json::from_str(body).expect("request json");
        value["armed_token"] = serde_json::Value::String(token);
        value.to_string()
    }

    fn armed_wire(body: &str, nonce: &str, signer: &HmacTokenSigner) -> String {
        armed_wire_with_ttl(body, nonce, signer, MAX_AUTH_TTL_MS)
    }

    #[test]
    fn action_topic_is_canonical_three_segments() {
        // Locks the action/<domain>/<verb> shape so the workbench RPC caller
        // (which rejects topics outside `action/`) can publish to it.
        assert!(ACTION_TOPIC.starts_with("action/"));
        let parts: Vec<&str> = ACTION_TOPIC.split('/').collect();
        assert_eq!(parts, vec!["action", "exec", "request"]);
    }

    #[test]
    fn public_bus_action_requires_an_exact_single_use_capability() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-auth-test-key".to_vec()));
        let worker = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_bus_root(tmp.path().join("bus"))
            .with_authorization(signer.clone(), tmp.path().join("auth"))
            .with_workload_signer(signer.clone());
        let unsigned = lifecycle_req("oak", "container", "nginx", "restart");

        let refused = worker.handle_wire_action("01NOAUTH", &unsigned);
        assert!(!refused.ok);
        assert!(refused.error.unwrap().contains("not authorized"));
        assert!(read_workload_operation(tmp.path()).is_none());

        let mut legacy: serde_json::Value = serde_json::from_str(&unsigned).unwrap();
        legacy.as_object_mut().unwrap().remove("schema_version");
        let legacy = armed_wire(
            &legacy.to_string(),
            "action-legacy-schema-nonce",
            signer.as_ref(),
        );
        let refused = worker.handle_wire_action("01LEGACY", &legacy);
        assert!(!refused.ok);
        assert!(refused.error.unwrap().contains("schema_version 1"));
        assert!(read_workload_operation(tmp.path()).is_none());

        let armed = armed_wire(&unsigned, "action-single-use-nonce", signer.as_ref());
        let accepted = worker.handle_wire_action("01ARMED", &armed);
        assert!(accepted.ok, "{accepted:?}");
        let request = read_workload_operation(tmp.path()).expect("published Workload operation");
        assert_eq!(request.request_id, "01ARMED");
        assert_eq!(request.target_node, "peer:oak");
        assert_eq!(request.backend, WorkloadBackend::QuadletSystemd);
        assert_eq!(request.action, WorkloadOperationAction::Restart);

        let replay = worker.handle_wire_action("01REPLAY", &armed);
        assert!(!replay.ok);
        assert!(replay.error.unwrap().contains("already used"));
        assert_eq!(
            read_workload_operation(tmp.path())
                .expect("original Workload operation remains")
                .request_id,
            "01ARMED"
        );
    }

    #[test]
    fn action_capability_is_bound_to_the_complete_body() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-auth-test-key".to_vec()));
        let worker = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_bus_root(tmp.path().join("bus"))
            .with_authorization(signer.clone(), tmp.path().join("auth"))
            .with_workload_signer(signer.clone());
        let unsigned = lifecycle_req("oak", "container", "nginx", "start");
        let armed = armed_wire(&unsigned, "action-body-bound-nonce", signer.as_ref());
        let mut altered: serde_json::Value = serde_json::from_str(&armed).unwrap();
        altered["op"] = serde_json::Value::String("stop".to_string());

        let refused = worker.handle_wire_action("01ALTERED", &altered.to_string());
        assert!(!refused.ok);
        assert!(read_workload_operation(tmp.path()).is_none());
    }

    #[test]
    fn overlong_action_capability_reaches_no_dispatcher() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-auth-test-key".to_vec()));
        let worker = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_bus_root(tmp.path().join("bus"))
            .with_authorization(signer.clone(), tmp.path().join("auth"))
            .with_workload_signer(signer.clone());
        let unsigned = lifecycle_req("oak", "container", "nginx", "restart");
        let armed = armed_wire_with_ttl(
            &unsigned,
            "action-overlong-capability-nonce",
            signer.as_ref(),
            MAX_AUTH_TTL_MS + 30_000,
        );

        let refused = worker.handle_wire_action("01OVERLONG", &armed);
        assert!(!refused.ok);
        assert!(refused
            .error
            .unwrap()
            .contains("exceeds the 30-second lifetime"));
        assert!(read_workload_operation(tmp.path()).is_none());
    }

    #[test]
    fn parse_allowlisted_service_lifecycle() {
        let req = parse_action_request(&lifecycle_req("oak", "container", "nginx", "restart"))
            .expect("parse");
        assert_eq!(req.kind_tag(), "service_lifecycle");
        match req {
            ActionRequest::ServiceLifecycle {
                target_host,
                service_kind,
                name,
                op,
            } => {
                assert_eq!(target_host, "oak");
                assert_eq!(service_kind, "container");
                assert_eq!(name, "nginx");
                assert_eq!(op, "restart");
            }
            other => panic!("expected ServiceLifecycle, got {other:?}"),
        }
    }

    #[test]
    fn parse_and_plan_allowlisted_android_app_launch() {
        let req = parse_action_request(&android_launch_req("t480", "android-t480", "browser"))
            .expect("parse");
        assert_eq!(req.kind_tag(), "android_app_launch");
        let guest_request = plan_android_app_launch(&req, "01ANDROID").expect("canonical plan");
        match guest_request {
            AndroidGuestRequest::Launch(request) => {
                assert_eq!(request.request_id, "01ANDROID");
                assert_eq!(request.workload_id, "android-t480");
                assert_eq!(request.app, AospStarterApp::Browser);
                assert_eq!(request.intent, AospStarterApp::Browser.launch_intent());
            }
            AndroidGuestRequest::Inventory(_) => panic!("launch action planned inventory"),
        }
    }

    #[test]
    fn android_app_launch_rejects_arbitrary_command_or_intent_fields() {
        for extra in [
            serde_json::json!({"command": "sh -c id"}),
            serde_json::json!({
                "intent": {"package_id": "com.android.browser", "action": "main"}
            }),
        ] {
            let mut value: serde_json::Value =
                serde_json::from_str(&android_launch_req("t480", "android-t480", "browser"))
                    .expect("request json");
            let key = extra.as_object().unwrap().keys().next().unwrap().clone();
            value[key.clone()] = extra[key.clone()].clone();
            let error = parse_action_request(&value.to_string())
                .expect_err("closed Android launch must reject execution-shaped fields");
            assert!(error.contains("unknown field"), "{error}");
        }

        let error = parse_action_request(&android_launch_req(
            "t480",
            "android-t480",
            "org.example.arbitrary",
        ))
        .expect_err("only governed starter apps are admitted");
        assert!(error.contains("unknown variant"), "{error}");
    }

    #[test]
    fn android_app_launch_rejects_unsafe_target_and_workload_identity() {
        let unsafe_target =
            parse_action_request(&android_launch_req("../../t480", "android-t480", "browser"))
                .expect("serde admission");
        assert!(plan_android_app_launch(&unsafe_target, "01ANDROID").is_err());

        let unsafe_workload =
            parse_action_request(&android_launch_req("t480", "android/t480", "browser"))
                .expect("serde admission");
        assert!(plan_android_app_launch(&unsafe_workload, "01ANDROID").is_err());
    }

    #[test]
    fn parse_rejects_unknown_kind_no_executor_reached() {
        // An un-allowlisted KIND fails to deserialize (serde rejects the tag) —
        // it can never reach a dispatcher. This is the §9 backstop: there is no
        // generic/shell fallthrough.
        let body = serde_json::json!({ "kind": "raw_shell", "cmd": "rm -rf /" }).to_string();
        let err = parse_action_request(&body).expect_err("unknown kind must reject");
        assert!(err.contains("malformed action request"), "{err}");
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = parse_action_request("not json").expect_err("should fail");
        assert!(err.contains("malformed"), "{err}");
    }

    #[test]
    fn plan_workload_lifecycle_maps_closed_backend_and_action() {
        let req = parse_action_request(&lifecycle_req("oak", "vm", "win11", "stop")).unwrap();
        let plan = plan_workload_service_lifecycle(&req, "01HZX", None, 1_000).expect("planned");
        assert_eq!(plan.request_id, "01HZX");
        assert_eq!(plan.target_node, "peer:oak");
        assert_eq!(plan.backend, WorkloadBackend::LibvirtVirtqemud);
        assert_eq!(plan.action, WorkloadOperationAction::Stop);
        assert_eq!(plan.workload_id.as_str(), "vm:peer:oak:win11");
        assert!(plan.armed_token.is_none());
    }

    #[test]
    fn plan_workload_lifecycle_rejects_bad_op_and_kind() {
        let bad_op =
            parse_action_request(&lifecycle_req("oak", "container", "x", "explode")).unwrap();
        assert!(plan_workload_service_lifecycle(&bad_op, "u", None, 1_000).is_err());
        let bad_kind = parse_action_request(&lifecycle_req("oak", "kernel", "x", "start")).unwrap();
        assert!(plan_workload_service_lifecycle(&bad_kind, "u", None, 1_000).is_err());
    }

    #[test]
    fn plan_workload_lifecycle_rejects_empty_target_and_name() {
        let no_target =
            parse_action_request(&lifecycle_req("", "container", "x", "start")).unwrap();
        assert!(plan_workload_service_lifecycle(&no_target, "u", None, 1_000).is_err());
        let no_name =
            parse_action_request(&lifecycle_req("oak", "container", "", "start")).unwrap();
        assert!(plan_workload_service_lifecycle(&no_name, "u", None, 1_000).is_err());
    }

    #[test]
    fn reply_ok_and_rejected_shapes() {
        let ok = ActionReply::ok("dispatched");
        let v: serde_json::Value = serde_json::from_str(&ok.to_body()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["detail"], "dispatched");
        assert!(!v.as_object().unwrap().contains_key("error"));

        let rej = ActionReply::rejected("nope");
        let v: serde_json::Value = serde_json::from_str(&rej.to_body()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "nope");
        assert!(!v.as_object().unwrap().contains_key("detail"));
    }

    #[test]
    fn handle_dispatches_allowlisted_action_publishes_workload_operation() {
        // The end-to-end allowlisted path publishes one typed Workload operation
        // to the Bus; only workload_compute may actuate it.
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-workload-test-key".to_vec()));
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_bus_root(tmp.path().join("bus"))
            .with_workload_signer(signer);
        let reply = w.handle_action(
            "01HZX",
            &lifecycle_req("oak", "container", "nginx", "restart"),
        );
        assert!(reply.ok, "{reply:?}");
        let got = read_workload_operation(tmp.path()).expect("Workload operation");
        assert_eq!(got.request_id, "01HZX");
        assert_eq!(got.workload_id.as_str(), "container:peer:oak:nginx");
        assert_eq!(got.action, WorkloadOperationAction::Restart);
    }

    #[test]
    fn handle_dispatches_android_launch_through_fail_closed_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"));
        let reply = w.handle_action(
            "01ANDROID",
            &android_launch_req("t480", "android-t480", "browser"),
        );
        assert!(
            !reply.ok,
            "no real Android adapter is configured: {reply:?}"
        );
        assert!(reply
            .error
            .expect("fail-closed error")
            .contains("provider unavailable"));
    }

    #[test]
    fn authorized_android_launch_reaches_provider_then_stays_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-auth-test-key".to_vec()));
        let worker = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_authorization(signer.clone(), tmp.path().join("auth"));
        let unsigned = android_launch_req("t480", "android-t480", "browser");
        let armed = armed_wire(&unsigned, "android-launch-nonce", signer.as_ref());
        let reply = worker.handle_wire_action("01ANDROID", &armed);
        assert!(
            !reply.ok,
            "provider is intentionally unconfigured: {reply:?}"
        );
        assert!(reply
            .error
            .expect("unavailable error")
            .contains("provider unavailable"));
    }

    #[test]
    fn authorized_android_launch_uses_the_workload_scoped_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = Arc::new(HmacTokenSigner::new(b"action-auth-test-key".to_vec()));
        let worker = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_authorization(signer.clone(), tmp.path().join("auth"))
            .with_android_guest_provider("android-t480", Arc::new(ReadyActionProvider))
            .expect("valid workload provider registration");
        let unsigned = android_launch_req("t480", "android-t480", "browser");
        let armed = armed_wire(&unsigned, "android-scoped-provider-nonce", signer.as_ref());
        let reply = worker.handle_wire_action("01ANDROIDREADY", &armed);
        assert!(
            reply.ok,
            "registered provider should admit the launch: {reply:?}"
        );
        assert!(reply
            .detail
            .expect("success detail")
            .contains("admitted Android app"));
    }

    #[test]
    fn handle_rejects_disallowed_action_without_dispatch() {
        // A vocabulary-violating request (valid KIND, bad op) is a typed
        // rejection and publishes no Workload operation.
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"));
        let reply = w.handle_action("01HZX", &lifecycle_req("oak", "container", "x", "explode"));
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("not allowlisted"));
        assert!(read_workload_operation(tmp.path()).is_none());
    }

    #[test]
    fn handle_writes_hash_chain_audit_row_per_action() {
        // Every handled action (accepted OR rejected) appends a tamper-verifiable
        // hash-chain row to the EXISTING events plane.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("audit.db");
        let signer = Arc::new(HmacTokenSigner::new(b"action-audit-test-key".to_vec()));
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone())
            .with_bus_root(tmp.path().join("bus"))
            .with_workload_signer(signer);
        // One accepted + one rejected → two audit rows, an intact chain.
        let _ = w.handle_action("01A", &lifecycle_req("oak", "container", "nginx", "start"));
        let _ = w.handle_action("01B", &lifecycle_req("oak", "container", "x", "explode"));
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 2, "one audit row per handled action");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_bus_root(tmp.path().join("bus"))
            .with_db_path(tmp.path().join("audit.db"))
            .with_poll_interval(Duration::from_millis(20));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }

    #[test]
    fn worker_name_is_locked() {
        let w = ActionWorker::new(PathBuf::from("/tmp/x"), "peer:self".into());
        assert_eq!(w.name(), "action");
    }

    // ===================== FRONTDOOR-12: code-edit apply =====================

    fn code_edit_req(path: &str, content: &str) -> String {
        serde_json::json!({
            "schema_version": 1,
            "kind": "code_edit",
            "path": path,
            "content": content,
        })
        .to_string()
    }

    /// Init a real git repo in `root` so the FIXED-ARG `git add`/`git commit` the
    /// apply handler runs has somewhere to land. Sets a local identity so commit
    /// doesn't fail on a CI box with no global git config.
    fn git_init(root: &std::path::Path) {
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .expect("git spawn");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        // An initial commit so HEAD exists (not strictly required, but keeps the
        // repo in a normal state).
        std::fs::write(root.join("README"), "seed\n").unwrap();
        run(&["add", "--", "README"]);
        run(&["commit", "-q", "-m", "seed"]);
    }

    #[test]
    fn code_edit_parses_as_allowlisted_kind() {
        // The FD-12 variant deserializes from the wire form FD-11's allowlist
        // gate (parse_action_request) accepts — so the copilot's extract_proposal,
        // which routes through that SAME gate, can now propose a code_edit.
        let req =
            parse_action_request(&code_edit_req("config/app.toml", "x = 1\n")).expect("parse");
        assert_eq!(req.kind_tag(), "code_edit");
        match req {
            ActionRequest::CodeEdit { path, content } => {
                assert_eq!(path, "config/app.toml");
                assert_eq!(content, "x = 1\n");
            }
            other => panic!("expected CodeEdit, got {other:?}"),
        }
    }

    #[test]
    fn validate_edit_path_accepts_in_bounds_relative() {
        let root = Path::new("/srv/workgroup");
        let p = validate_edit_path(root, "config/app.toml").expect("in-bounds");
        assert_eq!(p, Path::new("/srv/workgroup/config/app.toml"));
        // A nested relative path is fine too.
        let p2 = validate_edit_path(root, "a/b/c.rs").expect("nested in-bounds");
        assert!(p2.starts_with(root));
    }

    #[test]
    fn validate_edit_path_rejects_absolute_escape() {
        let root = Path::new("/srv/workgroup");
        let err = validate_edit_path(root, "/etc/passwd").expect_err("absolute must reject");
        assert!(
            err.contains("absolute") || err.contains("root component"),
            "{err}"
        );
    }

    #[test]
    fn validate_edit_path_rejects_parent_traversal() {
        let root = Path::new("/srv/workgroup");
        let err = validate_edit_path(root, "../../etc/shadow").expect_err("traversal must reject");
        assert!(err.contains("traversal") || err.contains(".."), "{err}");
        // Even a traversal that re-enters the root by name is rejected — we bound
        // lexically, before the filesystem, so a symlink race can't widen it.
        assert!(validate_edit_path(root, "config/../../escape").is_err());
    }

    #[test]
    fn validate_edit_path_rejects_empty_and_curdir() {
        let root = Path::new("/srv/workgroup");
        assert!(validate_edit_path(root, "").is_err());
        assert!(validate_edit_path(root, "   ").is_err());
        assert!(validate_edit_path(root, "./config/x").is_err());
    }

    #[test]
    fn apply_in_bounds_writes_commits_and_audits() {
        // The post-authorization dispatcher writes the reviewed content to an
        // in-root path, commits it with FIXED git args, and audits the apply on
        // the hash-chain plane. The wire-level capability gate is covered above;
        // no shell or command string exists anywhere in this path.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let reply = w.handle_action("01HZX", &code_edit_req("config/app.toml", "answer = 42\n"));
        assert!(reply.ok, "{reply:?}");

        // The file landed with EXACTLY the reviewed content.
        let written = std::fs::read_to_string(tmp.path().join("config/app.toml")).unwrap();
        assert_eq!(written, "answer = 42\n");

        // It was committed (the working tree is clean for that path).
        let status = std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(["status", "--porcelain", "--", "config/app.toml"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "edit should be committed (clean status)"
        );

        // And it wrote a hash-chain audit row recording the path + content.
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "one audit row for the apply");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
        // The audit payload carries the path (reviewable trail, §8).
        let payload = String::from_utf8_lossy(&rows[0].payload);
        assert!(payload.contains("config/app.toml"), "{payload}");
    }

    #[test]
    fn apply_out_of_bounds_absolute_is_rejected_and_audited_no_write() {
        // An absolute path escapes the allowed root → typed rejection, NO file
        // written, and the rejection is still audited (a refused edit is on the
        // chain too).
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let target = tmp.path().join("pwned-marker");
        // Use an absolute path OUTSIDE the root (a tempdir sibling) — the handler
        // must refuse it.
        let outside = tmp.path().parent().unwrap().join("escape.txt");
        let reply = w.handle_action("01OOB", &code_edit_req(outside.to_str().unwrap(), "x"));
        assert!(!reply.ok);
        assert!(
            reply.error.as_deref().unwrap().contains("absolute")
                || reply.error.as_deref().unwrap().contains("root"),
            "{reply:?}"
        );
        assert!(!outside.exists(), "out-of-bounds path must NOT be written");
        assert!(!target.exists());

        // The rejection is audited (one row).
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "a rejected apply is still audited");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
    }

    #[test]
    fn apply_traversal_is_rejected_and_audited_no_write() {
        // A `..` traversal that would write outside the root → typed rejection,
        // nothing written, audited.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let escape = tmp.path().parent().unwrap().join("traversed.txt");
        let reply = w.handle_action("01TRV", &code_edit_req("../traversed.txt", "x"));
        assert!(!reply.ok);
        assert!(
            reply.error.as_deref().unwrap().contains("traversal")
                || reply.error.as_deref().unwrap().contains(".."),
            "{reply:?}"
        );
        assert!(!escape.exists(), "traversal target must NOT be written");

        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn apply_rejects_an_in_root_symlink_escape_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        git_init(root.path());
        symlink(outside.path(), root.path().join("linked")).unwrap();
        let db = root.path().join("audit.db");
        let worker = ActionWorker::new(root.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let reply = worker.handle_action(
            "01SYMLINK",
            &code_edit_req("linked/escape.txt", "must not escape\n"),
        );
        assert!(!reply.ok, "{reply:?}");
        assert!(
            !outside.path().join("escape.txt").exists(),
            "descriptor-relative path resolution must never follow the symlink"
        );
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "the refusal remains hash-chain audited");
    }

    #[test]
    fn apply_chain_stays_intact_across_mixed_outcomes() {
        // A success + a rejection both audit; the hash-chain stays tamper-intact
        // across both, proving §8 holds for the new code-edit action too.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let _ = w.handle_action("01A", &code_edit_req("notes.md", "hello\n"));
        let _ = w.handle_action("01B", &code_edit_req("/etc/escape", "no"));
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(
            rows.len(),
            2,
            "one row per handled apply (success + rejection)"
        );
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }

    // ── mackesd-01/-04: is_leader now routes through the substrate-aware LeaderGate ──
    //
    // action is representative of the standard `is_leader` swap shared by dc_health,
    // dc_jobs, dc_promote, farm_orchestrator, service_onboard, session_roaming (and,
    // via `self.node`, adfilter). The core split-brain regression is proven centrally
    // in leader_gate; here we prove the worker DELEGATES to it — fs behavior preserved
    // pre-cutover, etcd branch taken (and fail-closed) once endpoints exist.

    #[test]
    fn is_leader_true_on_the_uncontended_fs_lease() {
        // No etcd endpoints in the test env ⇒ is_leader delegates to LeaderGate's fs
        // path; an uncontended lease ⇒ this node leads (old try_acquire behavior kept).
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into());
        assert!(w.is_leader());
    }

    #[test]
    fn is_leader_false_when_another_node_holds_the_fs_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into());
        // A peer grabs the shared lease first ⇒ we are a follower.
        assert!(matches!(
            crate::leader::try_acquire(&w.leader_lock, "peer:other"),
            Ok(crate::leader::AcquireResult::Acquired)
        ));
        assert!(!w.is_leader());
    }

    #[test]
    fn is_leader_takes_the_etcd_branch_and_fails_closed_when_endpoints_present() {
        // The gate the worker's is_leader builds (from its OWN leader_lock + node_id):
        // with etcd endpoints present but unreachable it observes `/mesh/leader`, fails
        // closed (NOT leader), and — crucially — never falls back to acquiring the
        // per-node fs lock that caused the split-brain. Mirrors leader_gate's
        // etcd_path_fails_closed_and_never_touches_the_fs_lock, at the worker boundary.
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into());
        let gate = crate::leader_gate::LeaderGate::from_lock_path(
            w.leader_lock.clone(),
            w.node_id.clone(),
        )
        .with_endpoints(vec!["http://127.0.0.1:1".into()]);
        assert!(gate.uses_etcd(), "endpoints present ⇒ etcd branch");
        assert!(
            !gate.is_leader(),
            "unreachable etcd ⇒ fail-closed, not leader"
        );
        assert!(
            !w.leader_lock.exists(),
            "etcd branch must NOT fall back to the per-node fs acquire"
        );
    }
}
