//! Construct's Browser surface.
//!
//! The host browser runtime was extracted from this crate. Construct keeps the
//! surface and a small typed controller so activation has an explicit guest
//! destination while the native Display1 attachment is supplied by the
//! platform session layer. Chromium, browser chrome, page execution, and guest
//! failures remain inside `browser-vm`.

use mackes_mesh_types::cloud::{CloudState, DeploymentRole};
use mackes_mesh_types::workloads::{
    WorkloadBackend, WorkloadOperationAction, WorkloadOperationPhase, WorkloadProfile,
    WORKLOAD_OPERATION_TOPIC,
};
use mde_bookmarks_egui::{
    bookmarks_panel, real_manager, BookmarksBus, Manager as BookmarksManager,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{self, RichText};
use mde_egui::search_omnibox::SearchItem;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(test)]
use mde_files_egui::transfers::TransfersClient;

const VM_WORKLOAD: &str = "browser-vm";
const BROWSER_VM_RETRY_DELAY: Duration = Duration::from_secs(1);
const BROWSER_VM_LIFECYCLE_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(90);
const BROWSER_VM_PROJECTION_REFRESH: Duration = Duration::from_secs(1);

/// Shell media-key vocabulary retained at the VM boundary. The guest owns
/// playback; these actions are intentionally not translated into host controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaTransportAction {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrowserVmRoute {
    workload: &'static str,
    resume: bool,
}

impl BrowserVmRoute {
    const fn select_resume() -> Self {
        Self {
            workload: VM_WORKLOAD,
            resume: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserVmConnectionState {
    ProvisioningRequired,
    StartingWorkload,
    WaitingForVdi,
    Unavailable,
}

impl BrowserVmConnectionState {
    const fn label(self) -> &'static str {
        match self {
            Self::ProvisioningRequired => "Workloads action required",
            Self::StartingWorkload => "Starting Browser VM",
            Self::WaitingForVdi => "Attaching native display",
            Self::Unavailable => "Browser VM unavailable",
        }
    }
}

/// The only Browser activation input accepted from the Workloads mirror. It is
/// deliberately an identity/status tuple: no command, URL, endpoint, or host
/// engine data can cross the Browser boundary here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserVmTarget {
    pub(crate) serving_peer: String,
    pub(crate) workload: String,
    pub(crate) status: String,
    pub(crate) reachable: bool,
}

impl BrowserVmTarget {
    /// Fold the live instance table from the same fresh `state/cloud/<node>`
    /// Workloads projection over the slower desired-workload row. The cloud
    /// worker republishes this table immediately after a lifecycle action,
    /// while its drift-backed workload rows intentionally refresh less often.
    /// No endpoint or credential is derived here: only the stable domain's
    /// observed power state becomes newer evidence for the existing target.
    pub(crate) fn with_live_workloads_state(mut self, states: &[CloudState]) -> Self {
        let Some(state) = states.iter().find(|state| {
            state.host == self.serving_peer && crate::iac::cloud_state_is_fresh(state)
        }) else {
            return self;
        };
        let live_status = state
            .resources
            .iter()
            .filter(|table| table.service_type == "compute" && table.collection == "instances")
            .flat_map(|table| table.rows.iter())
            .find_map(|row| {
                let name = row.cells.first()?;
                let status = row.cells.get(1)?;
                (name == &self.workload || row.id == self.workload).then_some(status.as_str())
            });
        if let Some(status) = live_status {
            self.status = status
                .trim()
                .chars()
                .take(64)
                .collect::<String>()
                .to_ascii_lowercase();
            self.reachable = matches_status(&self.status, &["active", "running"]);
        }
        self
    }
}

/// A one-shot handoff from the Browser surface to the node-local Display1
/// attachment path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserVmConnect {
    pub(crate) target: BrowserVmTarget,
}

/// The Workloads producer's canonical running-domain status is `active`.
/// Accept the older shell-side `running` spelling during mixed-version rollout,
/// but require the producer's independent reachability proof in both cases.
fn browser_vm_ready(target: &BrowserVmTarget) -> bool {
    target.workload == VM_WORKLOAD
        && safe_segment(&target.serving_peer)
        && target.reachable
        && matches_status(&target.status, &["active", "running"])
}

fn matches_status(status: &str, candidates: &[&str]) -> bool {
    let status = status.trim();
    candidates
        .iter()
        .any(|candidate| status.eq_ignore_ascii_case(candidate))
}

fn safe_segment(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserVmObservedState {
    Running,
    Starting,
    Startable,
    Paused,
    ProvisioningRequired,
    Failed,
    Unknown,
}

fn observed_state(status: &str) -> BrowserVmObservedState {
    if matches_status(status, &["active", "running"]) {
        BrowserVmObservedState::Running
    } else if matches_status(status, &["starting", "booting", "provisioning", "resuming"]) {
        BrowserVmObservedState::Starting
    } else if matches_status(
        status,
        &[
            "shutoff", "shut off", "stopped", "inactive", "defined", "crashed",
        ],
    ) {
        BrowserVmObservedState::Startable
    } else if matches_status(status, &["paused", "suspended"]) {
        BrowserVmObservedState::Paused
    } else if matches_status(status, &["absent", "missing", "not found", "not_found"]) {
        BrowserVmObservedState::ProvisioningRequired
    } else if matches_status(status, &["error", "failed", "unknown"]) {
        BrowserVmObservedState::Failed
    } else {
        BrowserVmObservedState::Unknown
    }
}

fn bounded_status(status: &str) -> String {
    status.trim().chars().take(64).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserVmLifecycleKind {
    Start,
    Resume,
}

impl BrowserVmLifecycleKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Resume => "resume",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserVmLifecycleIntent {
    serving_peer: String,
    workload: String,
    kind: BrowserVmLifecycleKind,
}

impl BrowserVmLifecycleIntent {
    fn new(target: &BrowserVmTarget, kind: BrowserVmLifecycleKind) -> Result<Self, String> {
        if target.workload != VM_WORKLOAD {
            return Err("Workloads selected a non-browser workload; nothing was sent.".to_owned());
        }
        if !safe_segment(&target.serving_peer) {
            return Err(
                "The Browser VM placement node is not a capability-safe mesh identity; nothing was sent."
                    .to_owned(),
            );
        }
        Ok(Self {
            serving_peer: target.serving_peer.clone(),
            workload: target.workload.clone(),
            kind,
        })
    }

    const fn operation(&self) -> WorkloadOperationAction {
        match self.kind {
            BrowserVmLifecycleKind::Start => WorkloadOperationAction::Start,
            BrowserVmLifecycleKind::Resume => WorkloadOperationAction::Resume,
        }
    }
}

#[derive(Debug, Clone)]
struct BrowserVmLifecyclePending {
    intent: BrowserVmLifecycleIntent,
    request_id: String,
    bus_root: PathBuf,
    published_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserVmDiagnostic {
    state: BrowserVmConnectionState,
    detail: String,
}

/// The shell-side state retained after the host Browser runtime extraction.
/// This deliberately contains no page, tab, engine, process, or pixel state.
pub(crate) struct WebState {
    route: BrowserVmRoute,
    diagnostic: BrowserVmDiagnostic,
    latest_target: Option<BrowserVmTarget>,
    requested_target: Option<String>,
    browser_vm_connect: Option<BrowserVmConnect>,
    browser_vm_request_issued: bool,
    browser_vm_retry_not_before: Option<Instant>,
    lifecycle_queued: Option<BrowserVmLifecycleIntent>,
    lifecycle_last_attempt: Option<BrowserVmLifecycleIntent>,
    lifecycle_pending: Option<BrowserVmLifecyclePending>,
    projection_refresh_not_before: Option<Instant>,
    projection_refresh_requested: bool,
    open_workloads_requested: bool,
    /// Browser-owned bookmark/provider state. The Browser surface is the only
    /// shell presentation for this state.
    bookmarks: BookmarksManager,
    bookmarks_bus: BookmarksBus,
    #[cfg(test)]
    _transfers: Option<Box<dyn TransfersClient>>,
}

impl Default for WebState {
    fn default() -> Self {
        let route = BrowserVmRoute::select_resume();
        Self {
            route,
            diagnostic: BrowserVmDiagnostic {
                state: BrowserVmConnectionState::WaitingForVdi,
                detail: "The guest Browser VM is selected; attach it directly to this seat."
                    .to_owned(),
            },
            latest_target: None,
            requested_target: None,
            browser_vm_connect: None,
            browser_vm_request_issued: false,
            browser_vm_retry_not_before: None,
            lifecycle_queued: None,
            lifecycle_last_attempt: None,
            lifecycle_pending: None,
            projection_refresh_not_before: None,
            projection_refresh_requested: false,
            open_workloads_requested: false,
            bookmarks: real_manager(),
            bookmarks_bus: BookmarksBus::default(),
            #[cfg(test)]
            _transfers: None,
        }
    }
}

impl WebState {
    /// Keep the shell's existing test seam without reinstating a Browser-owned
    /// transfer ledger. Transfers remain owned by Files/Transfers.
    #[cfg(test)]
    pub(crate) fn with_transfers(mut self, transfers: Box<dyn TransfersClient>) -> Self {
        self._transfers = Some(transfers);
        self
    }

    /// Browser no longer owns host download jobs; the shared shell operation
    /// summary therefore has no Browser-local contribution.
    pub(crate) fn operation_progress_summary(
        &self,
    ) -> Option<mde_files_egui::model::OperationProgressSummary> {
        None
    }

    pub(crate) fn pump_downloads_for_shell_chrome(&mut self) {}

    #[cfg(test)]
    pub(crate) fn mark_downloads_poll_due_for_test(&mut self) {}

    pub(crate) fn open_search_omnibox_target(&mut self, target: &str) {
        let target = target.trim();
        self.requested_target = (!target.is_empty()).then(|| target.chars().take(512).collect());
    }

    /// Fold the typed Workloads projection into Browser activation. A VM that
    /// is not yet reachable remains unavailable; this path never starts a host
    /// helper as a fallback.
    pub(crate) fn sync_browser_vm_target(&mut self, target: Option<BrowserVmTarget>) {
        self.sync_browser_vm_target_at(target, Instant::now());
    }

    fn sync_browser_vm_target_at(&mut self, target: Option<BrowserVmTarget>, now: Instant) {
        let target_changed = self.latest_target.as_ref() != target.as_ref();
        self.latest_target = target.clone();
        if target_changed {
            // A new projection is new evidence; do not make it inherit an old
            // target's retry delay.
            self.browser_vm_retry_not_before = None;
        }
        if self.browser_vm_request_issued || self.browser_vm_connect.is_some() {
            return;
        }
        if self
            .browser_vm_retry_not_before
            .is_some_and(|deadline| now < deadline)
        {
            return;
        }
        self.browser_vm_retry_not_before = None;
        let Some(target) = target else {
            self.diagnostic = BrowserVmDiagnostic {
                state: if self.lifecycle_pending.is_some() {
                    BrowserVmConnectionState::StartingWorkload
                } else {
                    BrowserVmConnectionState::ProvisioningRequired
                },
                detail: if self.lifecycle_pending.is_some() {
                    "The admitted Browser VM temporarily disappeared from the Workloads projection; waiting for fresh observed state."
                        .to_owned()
                } else {
                    "No admitted browser-vm workload is present. Open Workloads to choose placement and an immutable guest image; Browser cannot invent first provisioning."
                        .to_owned()
                },
            };
            return;
        };
        if target.workload != VM_WORKLOAD || !safe_segment(&target.serving_peer) {
            self.lifecycle_queued = None;
            self.diagnostic = BrowserVmDiagnostic {
                state: BrowserVmConnectionState::Unavailable,
                detail: "The Workloads projection did not provide the stable browser-vm on a capability-safe placement node; nothing was sent."
                    .to_owned(),
            };
            return;
        }
        match observed_state(&target.status) {
            BrowserVmObservedState::Running if browser_vm_ready(&target) => {
                self.clear_lifecycle_attempt();
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::WaitingForVdi,
                    detail: "The Browser VM is reachable and active in Workloads; attaching its native Display1 surface to this seat."
                        .to_owned(),
                };
                self.browser_vm_connect = Some(BrowserVmConnect { target });
            }
            BrowserVmObservedState::Running => {
                self.clear_lifecycle_attempt();
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::WaitingForVdi,
                    detail: format!(
                        "Workload `{}` on `{}` is active but has not supplied a native Display1-ready lease; waiting for authoritative readiness.",
                        target.workload, target.serving_peer
                    ),
                };
            }
            BrowserVmObservedState::Startable => {
                self.queue_lifecycle(&target, BrowserVmLifecycleKind::Start, now);
            }
            BrowserVmObservedState::Paused => {
                self.queue_lifecycle(&target, BrowserVmLifecycleKind::Resume, now);
            }
            BrowserVmObservedState::Starting => {
                self.lifecycle_queued = None;
                self.request_projection_refresh_at(now);
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::StartingWorkload,
                    detail: format!(
                        "Workload `{}` on `{}` reports {}; waiting for a reachable active/running Workloads projection.",
                        target.workload,
                        target.serving_peer,
                        bounded_status(&target.status)
                    ),
                };
            }
            BrowserVmObservedState::ProvisioningRequired => {
                self.clear_lifecycle_attempt();
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::ProvisioningRequired,
                    detail: format!(
                        "Workload `{}` is admitted on `{}` but no live domain exists. Open Workloads to review and apply first provisioning; Browser has no image digest or authority to invent it.",
                        target.workload, target.serving_peer
                    ),
                };
            }
            BrowserVmObservedState::Failed | BrowserVmObservedState::Unknown => {
                self.lifecycle_queued = None;
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::Unavailable,
                    detail: format!(
                        "Workload `{}` on `{}` reports unsupported state `{}`; inspect it in Workloads. Nothing was sent.",
                        target.workload,
                        target.serving_peer,
                        bounded_status(&target.status)
                    ),
                };
            }
        }
    }

    fn queue_lifecycle(
        &mut self,
        target: &BrowserVmTarget,
        kind: BrowserVmLifecycleKind,
        now: Instant,
    ) {
        let intent = match BrowserVmLifecycleIntent::new(target, kind) {
            Ok(intent) => intent,
            Err(detail) => {
                self.lifecycle_queued = None;
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::Unavailable,
                    detail,
                };
                return;
            }
        };
        if self.lifecycle_pending.is_some() {
            self.request_projection_refresh_at(now);
            self.diagnostic = BrowserVmDiagnostic {
                state: BrowserVmConnectionState::StartingWorkload,
                detail: format!(
                    "A capability-bound {} request is already in flight; waiting for Workloads to report browser-vm reachable and active/running.",
                    kind.label()
                ),
            };
            return;
        }
        if self.lifecycle_last_attempt.as_ref() == Some(&intent) {
            if self.diagnostic.state != BrowserVmConnectionState::Unavailable {
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::StartingWorkload,
                    detail: format!(
                        "The one-shot {} request was already issued; waiting for fresh Workloads evidence rather than publishing every frame.",
                        kind.label()
                    ),
                };
            }
            return;
        }
        self.lifecycle_queued = Some(intent);
        self.diagnostic = BrowserVmDiagnostic {
            state: BrowserVmConnectionState::StartingWorkload,
            detail: format!(
                "Preparing one capability-bound {} intent for admitted workload `{}` on `{}`.",
                kind.label(),
                target.workload,
                target.serving_peer
            ),
        };
    }

    fn clear_lifecycle_attempt(&mut self) {
        self.lifecycle_queued = None;
        self.lifecycle_last_attempt = None;
        self.lifecycle_pending = None;
        self.projection_refresh_not_before = None;
        self.projection_refresh_requested = false;
    }

    /// Publish at most one lifecycle intent for the current observed state and
    /// fold any Workloads reply. Authorization is minted by the existing root
    /// shell credential loader and remains exact-body, target, node, verb, TTL,
    /// and nonce bound. A failed publish is retained as an explicit diagnostic;
    /// it is never retried on every paint frame.
    pub(crate) fn drive_browser_vm_lifecycle(&mut self, bus_root: Option<&Path>) {
        self.drive_browser_vm_lifecycle_at(bus_root, Instant::now());
    }

    fn drive_browser_vm_lifecycle_at(&mut self, bus_root: Option<&Path>, now: Instant) {
        self.poll_lifecycle_reply_at(now);
        let Some(intent) = self.lifecycle_queued.take() else {
            self.request_projection_refresh_at(now);
            return;
        };
        self.lifecycle_last_attempt = Some(intent.clone());
        let result = self.publish_lifecycle(&intent, bus_root, now);
        match result {
            Ok(pending) => {
                let kind = pending.intent.kind;
                self.lifecycle_pending = Some(pending);
                self.projection_refresh_not_before = Some(now);
                self.projection_refresh_requested = true;
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::StartingWorkload,
                    detail: format!(
                        "Authorized {} requested once; waiting for Workloads to observe browser-vm reachable and active/running.",
                        kind.label()
                    ),
                };
            }
            Err(error) => {
                self.lifecycle_pending = None;
                self.projection_refresh_not_before = None;
                self.diagnostic = BrowserVmDiagnostic {
                    state: BrowserVmConnectionState::Unavailable,
                    detail: format!("Browser VM lifecycle request was not sent: {error}"),
                };
            }
        }
    }

    fn publish_lifecycle(
        &self,
        intent: &BrowserVmLifecycleIntent,
        bus_root: Option<&Path>,
        now: Instant,
    ) -> Result<BrowserVmLifecyclePending, String> {
        let root =
            bus_root.ok_or_else(|| "the local mesh Bus directory is unavailable".to_owned())?;
        let persist = Persist::open(root.to_path_buf())
            .map_err(|error| format!("the local mesh Bus could not be opened: {error}"))?;
        let expected_generation = crate::workload_api::read_status(
            &persist,
            &intent.serving_peer,
            &intent.workload,
        )
        .map(|status| status.generation)
        .unwrap_or(0);
        let request = crate::workload_api::request(
            &intent.workload,
            &intent.serving_peer,
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            intent.operation(),
            None,
            expected_generation,
            workload_now_ms(),
        )?;
        let body = serde_json::to_string(&request)
            .map_err(|error| format!("Workload request could not be encoded: {error}"))?;
        persist
            .write(WORKLOAD_OPERATION_TOPIC, Priority::Default, None, Some(&body))
            .map_err(|error| format!("Workload operation rejected by the local Bus: {error}"))?;
        Ok(BrowserVmLifecyclePending {
            intent: intent.clone(),
            request_id: request.request_id,
            bus_root: root.to_path_buf(),
            published_at: now,
        })
    }

    fn poll_lifecycle_reply_at(&mut self, now: Instant) {
        let Some(snapshot) = self.lifecycle_pending.as_ref().map(|pending| {
            (
                pending.intent.clone(),
                pending.request_id.clone(),
                pending.bus_root.clone(),
                pending.published_at,
            )
        }) else {
            return;
        };
        let (intent, request_id, bus_root, published_at) = snapshot;
        let elapsed = now.saturating_duration_since(published_at);
        if let Ok(persist) = Persist::open(bus_root.clone()) {
            if let Some(status) = crate::workload_api::read_status(
                &persist,
                &intent.serving_peer,
                &intent.workload,
            ) {
                // The state projection is keyed by workload, not operation.
                // Ignore a terminal row from an older or foreign operation;
                // otherwise a stale completion could make this one-shot
                // Browser start/resume appear to have succeeded.
                if status.request_id == request_id && status.phase.is_terminal() {
                    self.lifecycle_pending = None;
                    self.projection_refresh_not_before = None;
                    if status.phase == WorkloadOperationPhase::Completed {
                        self.diagnostic = BrowserVmDiagnostic {
                            state: BrowserVmConnectionState::StartingWorkload,
                            detail: "Workload operation completed; waiting for the fresh Browser VM projection before attaching VDI."
                                .to_owned(),
                        };
                    } else {
                        let reason = status
                            .reason
                            .unwrap_or_else(|| format!("operation reached {:?}", status.phase));
                        self.diagnostic = BrowserVmDiagnostic {
                            state: BrowserVmConnectionState::Unavailable,
                            detail: format!("Workload operation failed: {reason}"),
                        };
                    }
                    return;
                }
            }
        }
        if elapsed >= BROWSER_VM_LIFECYCLE_OBSERVATION_TIMEOUT {
            self.lifecycle_pending = None;
            self.projection_refresh_not_before = None;
            self.diagnostic = BrowserVmDiagnostic {
                state: BrowserVmConnectionState::Unavailable,
                detail: format!(
                    "Browser VM {} was issued, but Workloads did not report a reachable active/running workload within the bounded observation window. Retry is operator-controlled.",
                    intent.kind.label()
                ),
            };
            return;
        }
        self.request_projection_refresh_at(now);
    }

    fn request_projection_refresh_at(&mut self, now: Instant) {
        if self.lifecycle_pending.is_none() {
            return;
        }
        if self
            .projection_refresh_not_before
            .is_none_or(|deadline| now >= deadline)
        {
            self.projection_refresh_requested = true;
            self.projection_refresh_not_before = Some(now + BROWSER_VM_PROJECTION_REFRESH);
        }
    }

    pub(crate) fn take_browser_vm_projection_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.projection_refresh_requested)
    }

    fn retry_browser_vm_lifecycle(&mut self) {
        self.lifecycle_queued = None;
        self.lifecycle_last_attempt = None;
        self.lifecycle_pending = None;
        self.projection_refresh_not_before = None;
        self.projection_refresh_requested = false;
        if let Some(target) = self.latest_target.clone() {
            self.sync_browser_vm_target_at(Some(target), Instant::now());
        }
    }

    fn can_retry_browser_vm_lifecycle(&self) -> bool {
        self.diagnostic.state == BrowserVmConnectionState::Unavailable
            && self.latest_target.as_ref().is_some_and(|target| {
                matches!(
                    observed_state(&target.status),
                    BrowserVmObservedState::Startable | BrowserVmObservedState::Paused
                )
            })
    }

    pub(crate) fn take_open_workloads_request(&mut self) -> bool {
        std::mem::take(&mut self.open_workloads_requested)
    }

    /// Begin the one-shot native attachment request. The caller either completes
    /// signing and Bus publication or reports the failed attempt
    /// through [`Self::browser_vm_unavailable`]. A reported failure rolls this
    /// optimistic commit back after a bounded delay; no report leaves the
    /// successful request committed exactly once.
    pub(crate) fn take_browser_vm_connect(&mut self) -> Option<BrowserVmConnect> {
        let request = self.browser_vm_connect.take()?;
        self.browser_vm_request_issued = true;
        self.browser_vm_retry_not_before = None;
        Some(request)
    }

    /// Forget only the shell-side Display1 attachment after the operator explicitly
    /// leaves the Browser surface. The stable guest workload remains untouched;
    /// returning to Browser must be able to request a fresh attachment to that
    /// same VM session.
    pub(crate) fn note_display1_attachment_detached(&mut self) {
        self.browser_vm_request_issued = false;
        self.browser_vm_connect = None;
        self.browser_vm_retry_not_before = None;
        self.diagnostic = BrowserVmDiagnostic {
            state: BrowserVmConnectionState::WaitingForVdi,
            detail: "The guest Browser VM is selected; attach it directly to this seat.".to_owned(),
        };
    }

    /// Surface an explicit native-display attachment failure without inventing a
    /// page or a host-rendered fallback. Every fallible post-take call site
    /// reports here, so roll back the optimistic one-shot commit and permit one
    /// new attempt after a short delay rather than consuming Browser activation
    /// permanently or publishing on every paint frame.
    pub(crate) fn browser_vm_unavailable(&mut self, detail: impl Into<String>) {
        self.browser_vm_unavailable_at(detail, Instant::now());
    }

    fn browser_vm_unavailable_at(&mut self, detail: impl Into<String>, now: Instant) {
        let detail = detail.into();
        self.browser_vm_request_issued = false;
        self.browser_vm_connect = None;
        self.browser_vm_retry_not_before = Some(now + BROWSER_VM_RETRY_DELAY);
        self.diagnostic = BrowserVmDiagnostic {
            state: BrowserVmConnectionState::Unavailable,
            detail: format!("Native Display1 attachment failed: {detail}"),
        };
    }

    /// Browser search suggestions are guest-owned after the host runtime
    /// extraction; the shell contributes no page/history candidates.
    pub(crate) fn search_omnibox_items(&self, _query: &str) -> Vec<SearchItem<String>> {
        Vec::new()
    }

    /// Media hotkeys remain accepted by the shell dispatcher but are forwarded
    /// only after a future typed VDI input lane exists.
    pub(crate) fn selected_media_transport(&mut self, _action: MediaTransportAction) {}

    /// Surface occlusion has no host page/pixel worker to pause. Keep this
    /// compatibility seam so callers do not invent a second Browser lifecycle.
    pub(crate) fn note_surface_foreground(&mut self, _foreground: bool) {}

    fn diagnostic(&self) -> &BrowserVmDiagnostic {
        &self.diagnostic
    }
}

fn workload_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Render the Construct-owned VM boundary. The guest supplies the Browser UI
/// after a real VDI attachment; this placeholder never pretends to be a page.
pub(crate) fn web_panel(ui: &mut egui::Ui, state: &mut WebState) {
    ui.vertical(|ui| {
        ui.push_id("browser-guest-boundary", |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Browser VM");
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Guest-owned Chromium is available through the dedicated VM.")
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(format!("Workload: {}", state.route.workload));
                ui.label("Display: native Display1 on this seat");
                ui.add_space(8.0);
                ui.colored_label(
                    mde_egui::Style::TEXT_DIM,
                    format!(
                        "{}: {}",
                        state.diagnostic().state.label(),
                        state.diagnostic().detail
                    ),
                );
                match state.diagnostic().state {
                    BrowserVmConnectionState::ProvisioningRequired => {
                        ui.add_space(8.0);
                        if ui.button("Open Workloads").clicked() {
                            state.open_workloads_requested = true;
                        }
                    }
                    BrowserVmConnectionState::Unavailable
                        if state.can_retry_browser_vm_lifecycle() =>
                    {
                        ui.add_space(8.0);
                        if ui.button("Retry lifecycle request").clicked() {
                            state.retry_browser_vm_lifecycle();
                        }
                    }
                    _ => {}
                }
                if let Some(target) = state.requested_target.as_deref() {
                    ui.add_space(4.0);
                    ui.label(format!("Requested after VM attachment: {target}"));
                }
                ui.add_space(8.0);
                ui.label("No host page engine or host-rendered Browser UI is available.");
            });
        });
        ui.separator();
        ui.push_id("browser-bookmarks", |ui| {
            state.bookmarks_bus.pump(&mut state.bookmarks);
            bookmarks_panel(ui, &mut state.bookmarks);
        });
    });
}

/// Retained as a stable shell construction seam; Browser media control is now
/// guest-owned and has no host process to expose through MPRIS.
#[derive(Debug, Default)]
pub(crate) struct BrowserMprisHandle;

pub(crate) fn spawn_browser_mpris() -> BrowserMprisHandle {
    BrowserMprisHandle
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::{
        CloudProviderAdapter, DriftSummary, NodeCapacity, ResourceRow, ResourceTable,
    };
    use mackes_mesh_types::workloads::{WorkloadOperationAction, WorkloadOperationRequest};
    use mde_egui::egui::Context;

    fn target(status: &str, reachable: bool) -> BrowserVmTarget {
        BrowserVmTarget {
            serving_peer: "dell".to_owned(),
            workload: VM_WORKLOAD.to_owned(),
            status: status.to_owned(),
            reachable,
        }
    }

    fn fresh_cloud_state(status: &str) -> CloudState {
        let published_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as i64;
        CloudState {
            host: "dell".to_owned(),
            role: DeploymentRole::Workstation,
            adapter: CloudProviderAdapter::ConstructCloud,
            health: Vec::new(),
            resources: vec![ResourceTable {
                service_type: "compute".to_owned(),
                collection: "instances".to_owned(),
                columns: vec!["name".to_owned(), "status".to_owned()],
                rows: vec![ResourceRow {
                    id: "a1100000-0000-0000-0000-000000000000".to_owned(),
                    cells: vec![VM_WORKLOAD.to_owned(), status.to_owned()],
                }],
            }],
            apply_armed: true,
            published_at_ms,
            workloads: Vec::new(),
            drift_summary: DriftSummary::default(),
            node_capacity: NodeCapacity::default(),
            android_inventories: Vec::new(),
            android_provider_admissions: Vec::new(),
            android_vdi_sources: Vec::new(),
        }
    }

    fn first_body(root: &Path, topic: &str) -> serde_json::Value {
        let persist = Persist::open(root.to_path_buf()).expect("test Bus");
        let messages = persist.list_since(topic, None).expect("topic history");
        assert_eq!(messages.len(), 1, "one lifecycle publication");
        serde_json::from_str(messages[0].body.as_deref().expect("request body"))
            .expect("typed JSON")
    }

    #[test]
    fn default_browser_state_selects_only_the_typed_guest_route() {
        let state = WebState::default();
        assert_eq!(state.route, BrowserVmRoute::select_resume());
        assert_eq!(state.route.workload, VM_WORKLOAD);
        assert!(state.route.resume);
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::WaitingForVdi
        );
    }

    #[test]
    fn browser_panel_paints_guest_boundary_without_host_runtime() {
        let ctx = Context::default();
        // The Browser-owned bookmark panel uses the shared named heading/nav
        // font families, so install the same Carbon font mapping the shell
        // installs before asking a headless context to lay it out.
        mde_egui::Style::install(&ctx);
        let mut state = WebState::default();
        let out = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| web_panel(ui, &mut state));
        });
        assert!(!out.shapes.is_empty());
        assert_eq!(state.requested_target, None);
    }

    #[test]
    fn front_door_target_is_retained_until_guest_attachment() {
        let mut state = WebState::default();
        state.open_search_omnibox_target("  https://example.test/  ");
        assert_eq!(
            state.requested_target.as_deref(),
            Some("https://example.test/")
        );
    }

    #[test]
    fn missing_browser_vm_requires_explicit_workloads_admission() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(None);
        assert!(state.browser_vm_connect.is_none());
        assert!(state.lifecycle_queued.is_none());
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::ProvisioningRequired
        );
    }

    #[test]
    fn admitted_but_absent_browser_vm_requires_first_provisioning() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(Some(target("absent", false)));
        assert!(state.browser_vm_connect.is_none());
        assert!(state.lifecycle_queued.is_none());
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::ProvisioningRequired
        );
        assert!(state.diagnostic().detail.contains("no live domain"));
    }

    #[test]
    fn stopped_browser_vm_publishes_one_capability_bound_workloads_start() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let now = Instant::now();
        let stopped = target("shutoff", false);
        let mut state = WebState::default();

        state.sync_browser_vm_target_at(Some(stopped.clone()), now);
        assert_eq!(
            state.lifecycle_queued.as_ref().map(|intent| intent.kind),
            Some(BrowserVmLifecycleKind::Start)
        );
        state.drive_browser_vm_lifecycle_at(Some(tmp.path()), now);

        let body = first_body(tmp.path(), WORKLOAD_OPERATION_TOPIC);
        let request: WorkloadOperationRequest = serde_json::from_value(body).expect("request");
        assert_eq!(request.action, WorkloadOperationAction::Start);
        assert_eq!(request.target_node, "dell");
        assert_eq!(request.workload_id.as_str(), VM_WORKLOAD);
        assert!(request.armed_token.is_some());

        state.sync_browser_vm_target_at(Some(stopped), now + Duration::from_millis(10));
        state.drive_browser_vm_lifecycle_at(Some(tmp.path()), now + Duration::from_millis(10));
        let persist = Persist::open(tmp.path().to_path_buf()).expect("test Bus");
        assert_eq!(
            persist
                .list_since(WORKLOAD_OPERATION_TOPIC, None)
                .expect("topic history")
                .len(),
            1,
            "paint frames must not republish the mutation"
        );
    }

    #[test]
    fn paused_browser_vm_publishes_one_typed_resume_without_a_cloud_start_alias() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let now = Instant::now();
        let paused = target("paused", false);
        let mut state = WebState::default();

        state.sync_browser_vm_target_at(Some(paused.clone()), now);
        assert_eq!(
            state.lifecycle_queued.as_ref().map(|intent| intent.kind),
            Some(BrowserVmLifecycleKind::Resume)
        );
        state.drive_browser_vm_lifecycle_at(Some(tmp.path()), now);

        let body = first_body(tmp.path(), WORKLOAD_OPERATION_TOPIC);
        let request: WorkloadOperationRequest = serde_json::from_value(body).expect("request");
        assert_eq!(request.action, WorkloadOperationAction::Resume);
        assert_eq!(request.target_node, "dell");
        assert_eq!(request.workload_id.as_str(), VM_WORKLOAD);
        assert!(request.armed_token.is_some());
        let persist = Persist::open(tmp.path().to_path_buf()).expect("test Bus");
        assert_eq!(
            persist
                .list_since(WORKLOAD_OPERATION_TOPIC, None)
                .expect("typed topic")
                .len(),
            1
        );

        state.sync_browser_vm_target_at(Some(paused), now + Duration::from_millis(10));
        state.drive_browser_vm_lifecycle_at(Some(tmp.path()), now + Duration::from_millis(10));
        assert_eq!(
            persist
                .list_since(WORKLOAD_OPERATION_TOPIC, None)
                .expect("resume history")
                .len(),
            1,
            "paint frames must not republish resume"
        );
    }

    #[test]
    fn lifecycle_publish_failure_is_explicit_and_not_retried_per_frame() {
        let now = Instant::now();
        let stopped = target("stopped", false);
        let mut state = WebState::default();
        state.sync_browser_vm_target_at(Some(stopped.clone()), now);
        state.drive_browser_vm_lifecycle_at(None, now);
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::Unavailable
        );
        assert!(state.diagnostic().detail.contains("was not sent"));

        state.sync_browser_vm_target_at(Some(stopped), now + Duration::from_millis(10));
        assert!(state.lifecycle_queued.is_none());
    }

    #[test]
    fn fresh_workloads_instance_projection_unlocks_display1_only_after_active() {
        let shutoff = target("shutoff", false);
        let observed = shutoff.with_live_workloads_state(&[fresh_cloud_state("ACTIVE")]);
        assert_eq!(observed.status, "active");
        assert!(observed.reachable);

        let mut state = WebState::default();
        state.sync_browser_vm_target(Some(observed));
        assert!(state.take_browser_vm_connect().is_some());
    }

    #[test]
    fn active_but_unreachable_browser_vm_neither_mutates_nor_attaches() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(Some(target("active", false)));
        assert!(state.lifecycle_queued.is_none());
        assert!(state.take_browser_vm_connect().is_none());
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::WaitingForVdi
        );
    }

    #[test]
    fn reachable_active_browser_vm_crosses_only_the_native_display1_seam() {
        let mut state = WebState::default();
        let target = BrowserVmTarget {
            serving_peer: "eagle".to_owned(),
            workload: "browser-vm".to_owned(),
            status: "active".to_owned(),
            reachable: true,
        };
        state.sync_browser_vm_target(Some(target.clone()));
        let request = state.take_browser_vm_connect().expect("Display1 handoff");
        assert_eq!(request.target.workload, "browser-vm");
        assert!(state.browser_vm_connect.is_none());
        state.sync_browser_vm_target(Some(target.clone()));
        assert!(
            state.take_browser_vm_connect().is_none(),
            "a committed handoff must not be published twice"
        );

        state.note_display1_attachment_detached();
        state.sync_browser_vm_target(Some(target));
        assert_eq!(
            state
                .take_browser_vm_connect()
                .expect("returning to Browser reattaches the stable VM")
                .target
                .workload,
            "browser-vm"
        );
    }

    #[test]
    fn transient_browser_vm_failure_retries_then_commits_without_duplicates() {
        let now = Instant::now();
        let target = BrowserVmTarget {
            serving_peer: "dell".to_owned(),
            workload: "browser-vm".to_owned(),
            status: "active".to_owned(),
            reachable: true,
        };
        let mut state = WebState::default();

        state.sync_browser_vm_target_at(Some(target.clone()), now);
        assert!(state.take_browser_vm_connect().is_some(), "first attempt");
        state.browser_vm_unavailable_at("transient Display1 or Bus failure", now);

        state.sync_browser_vm_target_at(Some(target.clone()), now + BROWSER_VM_RETRY_DELAY / 2);
        assert!(
            state.take_browser_vm_connect().is_none(),
            "retry is bounded instead of running every paint frame"
        );

        state.sync_browser_vm_target_at(Some(target.clone()), now + BROWSER_VM_RETRY_DELAY);
        assert!(state.take_browser_vm_connect().is_some(), "retry attempt");

        state.sync_browser_vm_target_at(
            Some(target),
            now + BROWSER_VM_RETRY_DELAY + BROWSER_VM_RETRY_DELAY,
        );
        assert!(
            state.take_browser_vm_connect().is_none(),
            "the successful retry remains committed exactly once"
        );
    }

    #[test]
    fn active_workload_automatically_requests_native_display1() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(Some(BrowserVmTarget {
            serving_peer: "dell".to_owned(),
            workload: "browser-vm".to_owned(),
            status: "active".to_owned(),
            reachable: true,
        }));
        let attachment = state.take_browser_vm_connect().expect("Display1 handoff");
        assert_eq!(attachment.target.serving_peer, "dell");
        assert_eq!(attachment.target.workload, VM_WORKLOAD);
    }

    #[test]
    fn browser_vm_readiness_accepts_mixed_version_running_but_rejects_unreachable() {
        let mut target = BrowserVmTarget {
            serving_peer: "dell".to_owned(),
            workload: "browser-vm".to_owned(),
            status: "running".to_owned(),
            reachable: true,
        };
        assert!(browser_vm_ready(&target));
        target.reachable = false;
        assert!(!browser_vm_ready(&target));
        target.reachable = true;
        target.status = "defined".to_owned();
        assert!(!browser_vm_ready(&target));
    }
}
