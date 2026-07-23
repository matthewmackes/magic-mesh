//! KDC-MESH-6 — phone remote-input seat consumer.
//!
//! `kdc_host` owns the KDE Connect protocol and publishes sanitized
//! `action/seat/remote-input` rows. This worker owns the seated desktop side of
//! that handoff: validate the local Bus payload, invoke the configured
//! seat/uinput injector when present, and publish honest retained state/events.
//! A missing injector is an explicit unavailable state, never a fake success.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

use super::proc::status_with_timeout;
use super::{ShutdownToken, Worker};

/// KDC-owned remote-input handoff topic.
pub const ACTION_TOPIC: &str = "action/seat/remote-input";

/// Seated-user arm/disarm consent topic (security-7). The shell publishes an
/// explicit `arm` grant with a bounded TTL here when the seated user consents
/// to phone remote input; injection is refused unless a live arm is present.
pub const ARM_TOPIC: &str = "action/seat/remote-input-arm";

/// Retained-latest status topic prefix for this node.
pub const STATE_PREFIX: &str = "state/seat-remote-input/";

/// Retained-latest "is this seat being remotely driven" indicator prefix
/// (security-7). A shell overlay subscribes to `{INDICATOR_PREFIX}{node}` so
/// the seated user can see, in the moment, that their keyboard/mouse is (or is
/// not) under phone control.
pub const INDICATOR_PREFIX: &str = "state/seat/remote-input/";

/// Per-event injection result / audit topic prefix for this node.
pub const RESULT_PREFIX: &str = "event/seat-remote-input/";

/// Capability verb bound to every phone-originated seat handoff. The producer
/// and consumer must agree on this exact string; it is deliberately distinct
/// from the arm/disarm consent control, which cannot itself inject input.
pub const REMOTE_INPUT_AUTH_VERB: &str = "seat-remote-input";

/// Default poll cadence. Phone touchpad input is interactive, so stay below the
/// control-poller cadence while still using the Bus as the handoff contract.
pub const DEFAULT_TICK: Duration = Duration::from_millis(40);

const MAX_PHONE_CHARS: usize = 128;
const MAX_TEXT_CHARS: usize = 16;
const MAX_DELTA: f64 = 4096.0;
const DEFAULT_HELPER: &str = "/usr/libexec/mackesd/seat-remote-input";
const ENV_HELPER: &str = "MDE_SEAT_REMOTE_INPUT_COMMAND";
const INJECT_CMD_TIMEOUT: Duration = Duration::from_millis(500);

/// Hard ceiling on a single arm grant's lifetime. A longer request is clamped
/// so a forged/over-broad arm cannot leave the seat drivable indefinitely.
const MAX_ARM_TTL_MS: u64 = 300_000;
/// Arm lifetime used when a grant omits an explicit `ttl_ms`.
const DEFAULT_ARM_TTL_MS: u64 = 120_000;
/// Window after the last injected event during which the indicator reports
/// `active` (a phone is live-driving the seat right now), not merely `armed`.
const ACTIVE_WINDOW_MS: u64 = 2_000;

type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Keyboard modifiers attached to a remote-input key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemoteInputModifiers {
    /// Shift modifier.
    pub shift: bool,
    /// Control modifier.
    pub ctrl: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Super/meta modifier.
    #[serde(rename = "super")]
    pub super_key: bool,
}

/// Validated remote-input event for the seat injector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeatRemoteInputEvent {
    /// Relative pointer motion.
    Move {
        /// Bounded x movement.
        dx: f64,
        /// Bounded y movement.
        dy: f64,
    },
    /// Relative scroll movement.
    Scroll {
        /// Bounded scroll delta.
        delta: f64,
    },
    /// Mouse-button click.
    Button {
        /// Button name: `primary`, `secondary`, or `middle`.
        button: String,
        /// Number of clicks, currently 1 or 2.
        clicks: u8,
    },
    /// Text key token.
    Text {
        /// Text to inject.
        text: String,
        /// Active modifiers.
        modifiers: RemoteInputModifiers,
    },
    /// Special-key code token.
    SpecialKey {
        /// Bounded special key code.
        special_key: i64,
        /// Active modifiers.
        modifiers: RemoteInputModifiers,
    },
}

impl SeatRemoteInputEvent {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Move { .. } => "move",
            Self::Scroll { .. } => "scroll",
            Self::Button { .. } => "button",
            Self::Text { .. } => "text",
            Self::SpecialKey { .. } => "special_key",
        }
    }
}

/// Parsed and validated Bus handoff row.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteInputRequest {
    /// Request id from the Bus ULID.
    pub id: String,
    /// Paired phone id that originated the event.
    pub phone: String,
    /// Timestamp from `kdc_host`.
    pub ts_unix_ms: u64,
    /// Normalized seat event.
    pub event: SeatRemoteInputEvent,
}

/// Retained status for this node's remote-input consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputStatus {
    /// Node identifier that owns this status record.
    pub node: String,
    /// Outcome state: `idle`, `injected`, `unavailable`, or `error`.
    pub state: String,
    /// Most recent accepted request id.
    pub last_request_id: Option<String>,
    /// Phone id from the most recent accepted request.
    pub last_phone: Option<String>,
    /// Event kind from the most recent accepted request.
    pub last_kind: Option<String>,
    /// Last failure reason, if any.
    pub last_error: Option<String>,
    /// Accepted requests since worker start.
    pub accepted: u64,
    /// Successfully injected requests since worker start.
    pub injected: u64,
    /// Requests rejected as malformed.
    pub rejected: u64,
    /// Well-formed requests refused by an authorization or arm/consent gate
    /// (security-7): the capability was missing/expired/replayed/tampered, the
    /// seat was un-armed, the arm had expired, or the injection's phone did not
    /// match the armed phone.
    pub dropped: u64,
    /// Valid requests whose injector failed or was unavailable.
    pub failed: u64,
    /// Timestamp of the most recent accepted request.
    pub last_event_ms: Option<u64>,
    /// Timestamp of the most recent status publication.
    pub updated_ms: u64,
}

/// Authoritative "remote-input active" indicator for the seated user's shell
/// (security-7). Published on `{INDICATOR_PREFIX}{node}` on every arm, disarm,
/// TTL expiry, and active-window transition so a shell overlay can honestly
/// show whether — and by whom — this seat is being remotely driven.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputIndicator {
    /// Node whose seat this indicator describes.
    pub node: String,
    /// True while a live arm grant permits phone injection into this seat.
    pub armed: bool,
    /// True while armed AND a phone injected within the active window — i.e. a
    /// phone is driving the seat right now, not merely permitted to.
    pub active: bool,
    /// Controlling source label from the arm grant, when armed.
    pub source: Option<String>,
    /// Phone id the arm grant is bound to, when the seated user named one.
    pub phone: Option<String>,
    /// Wall-clock ms at which the current arm auto-disarms, when armed.
    pub armed_until_ms: Option<u64>,
    /// Timestamp of this indicator publication.
    pub updated_ms: u64,
}

/// Live arm grant: phone injection is permitted until `armed_until_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArmState {
    /// Wall-clock ms at which this grant auto-disarms.
    armed_until_ms: u64,
    /// Controlling source label recorded for the indicator/audit.
    source: String,
    /// Phone the grant is bound to, when the seated user named one.
    phone: Option<String>,
}

/// Parsed arm-control command drained from [`ARM_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArmCommand {
    /// Arm the seat for remote input for `ttl_ms` (clamped to `MAX_ARM_TTL_MS`).
    Arm {
        /// Requested arm lifetime in ms.
        ttl_ms: u64,
        /// Controlling source label.
        source: String,
        /// Optional phone binding: only this phone may then inject.
        phone: Option<String>,
    },
    /// Explicitly disarm the seat now.
    Disarm {
        /// Source that requested the disarm.
        source: String,
    },
}

/// Error from the seat input injector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputInjectError {
    /// No live injector helper is configured or installed.
    Unavailable(String),
    /// The configured injector helper failed.
    Failed(String),
}

impl std::fmt::Display for InputInjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(msg) | Self::Failed(msg) => f.write_str(msg),
        }
    }
}

/// Injectable seam for the live seat/uinput backend.
pub trait SeatInputInjector: Send + Sync {
    /// Inject one validated remote-input event into the local seat.
    fn inject(&self, event: &SeatRemoteInputEvent) -> Result<(), InputInjectError>;
}

/// Command-backed live injector. The helper receives one JSON event argument.
/// Operators can set `MDE_SEAT_REMOTE_INPUT_COMMAND`; otherwise the packaged
/// `/usr/libexec/mackesd/seat-remote-input` helper is used when present.
#[derive(Debug, Default)]
pub struct CommandSeatInputInjector {
    helper: Option<PathBuf>,
}

impl CommandSeatInputInjector {
    /// Resolve the configured or packaged helper.
    #[must_use]
    pub fn new() -> Self {
        let helper = std::env::var_os(ENV_HELPER)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| {
                let p = PathBuf::from(DEFAULT_HELPER);
                p.exists().then_some(p)
            });
        Self { helper }
    }

    /// Build with an explicit helper path for tests.
    #[must_use]
    pub fn with_helper(helper: PathBuf) -> Self {
        Self {
            helper: Some(helper),
        }
    }
}

impl SeatInputInjector for CommandSeatInputInjector {
    fn inject(&self, event: &SeatRemoteInputEvent) -> Result<(), InputInjectError> {
        let Some(helper) = self.helper.as_ref() else {
            return Err(InputInjectError::Unavailable(format!(
                "no seat input helper configured ({ENV_HELPER}) or installed at {DEFAULT_HELPER}"
            )));
        };
        let body = serde_json::to_string(event)
            .map_err(|e| InputInjectError::Failed(format!("serialize input event: {e}")))?;
        let mut cmd = Command::new(helper);
        cmd.arg(body);
        let status = status_with_timeout(cmd, INJECT_CMD_TIMEOUT)
            .map_err(|e| InputInjectError::Failed(format!("run {}: {e}", helper.display())))?;
        classify_helper_status(helper, status)
    }
}

fn classify_helper_status(
    helper: &std::path::Path,
    status: ExitStatus,
) -> Result<(), InputInjectError> {
    if status.success() {
        Ok(())
    } else if matches!(status.code(), Some(69 | 78)) {
        Err(InputInjectError::Unavailable(format!(
            "{} reported unavailable with {status}",
            helper.display()
        )))
    } else {
        Err(InputInjectError::Failed(format!(
            "{} exited with {status}",
            helper.display()
        )))
    }
}

/// Daemon worker for KDC remote-input handoffs.
pub struct SeatRemoteInputWorker {
    node: String,
    cursor: Option<String>,
    tick: Duration,
    now_fn: NowFn,
    bus_root_override: Option<PathBuf>,
    injector: Arc<dyn SeatInputInjector>,
    /// Exact-body capability verifier for the privileged uinput handoff.
    /// Missing production credentials install a fail-closed verifier.
    authorizer: Arc<ActionAuthorizer>,
    status: RemoteInputStatus,
    /// Current arm/consent grant, if the seat is armed for remote input.
    arm: Option<ArmState>,
    /// Bus cursor for the arm-control topic.
    arm_cursor: Option<String>,
    /// Timestamp of the last successful injection, for the `active` indicator.
    last_injected_ms: Option<u64>,
    /// Last-published indicator, so we only re-publish on a real transition.
    last_indicator: Option<RemoteInputIndicator>,
    /// Last-published status, so an idle 40 ms tick does not flood retained
    /// state and race the persistence verifier between file write and row insert.
    last_published_status: Option<RemoteInputStatus>,
}

impl SeatRemoteInputWorker {
    /// Create a remote-input worker for one node.
    #[must_use]
    pub fn new(node: String) -> Self {
        Self::with_injector(node, Arc::new(CommandSeatInputInjector::new()))
    }

    /// Create with an injected input backend.
    #[must_use]
    pub fn with_injector(node: String, injector: Arc<dyn SeatInputInjector>) -> Self {
        let now_fn: NowFn = Arc::new(default_now);
        let updated_ms = now_fn();
        Self {
            node: node.clone(),
            cursor: None,
            tick: DEFAULT_TICK,
            now_fn,
            bus_root_override: None,
            injector,
            authorizer: Arc::new(ActionAuthorizer::production()),
            status: RemoteInputStatus {
                node,
                state: "idle".to_owned(),
                last_request_id: None,
                last_phone: None,
                last_kind: None,
                last_error: None,
                accepted: 0,
                injected: 0,
                rejected: 0,
                dropped: 0,
                failed: 0,
                last_event_ms: None,
                updated_ms,
            },
            arm: None,
            arm_cursor: None,
            last_injected_ms: None,
            last_indicator: None,
            last_published_status: None,
        }
    }

    /// Override the worker polling interval.
    #[must_use]
    pub const fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Override the clock used for deterministic tests.
    #[must_use]
    pub fn with_now_fn(mut self, now: NowFn) -> Self {
        self.now_fn = now;
        self
    }

    /// Override the Bus root used by `Persist`.
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the capability verifier for deterministic hostile-wire tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    fn now_ms(&self) -> u64 {
        (self.now_fn)()
    }

    fn drain_requests(&mut self, persist: &Persist) {
        let msgs = match persist.list_since(ACTION_TOPIC, self.cursor.as_deref()) {
            Ok(msgs) => msgs,
            Err(e) => {
                tracing::debug!(target: "mackesd::seat_remote_input", error = %e, "list_since failed");
                return;
            }
        };
        for msg in msgs {
            self.cursor = Some(msg.ulid.clone());
            let body = msg.body.unwrap_or_default();
            if !crate::ipc::body_within_cap(Some(&body)) {
                self.status.rejected = self.status.rejected.saturating_add(1);
                self.status.state = "error".to_owned();
                self.status.last_error = Some("remote-input request body too large".to_owned());
                self.status.updated_ms = self.now_ms();
                self.publish_status(persist);
                continue;
            }
            match parse_request(&body, &msg.ulid) {
                Ok(request) => self.apply_request(persist, request, &body),
                Err(e) => {
                    self.status.rejected = self.status.rejected.saturating_add(1);
                    self.status.state = "error".to_owned();
                    self.status.last_error = Some(e);
                    self.status.updated_ms = self.now_ms();
                    self.publish_status(persist);
                }
            }
        }
    }

    fn apply_request(&mut self, persist: &Persist, request: RemoteInputRequest, body: &str) {
        let now = self.now_ms();

        // security-7 consent gate: injection is refused unless the seated user
        // has ARMED this seat for remote input and that grant is still live. An
        // un-armed or expired seat drops the event with a durable audit row —
        // it never reaches the root-capable uinput helper. This is the primary
        // control: the local Bus carries no cryptographic identity
        // (THREAT_MODEL §6.2.3), so a forged injection alone is inert.
        let arm = self.arm.clone().filter(|a| now < a.armed_until_ms);
        let Some(arm) = arm else {
            self.record_drop(
                persist,
                &request,
                now,
                "dropped_unarmed",
                "seat not armed for remote input",
            );
            return;
        };

        // Source sanity: when the seated user bound the arm to a specific phone,
        // an injection claiming a different phone is dropped. The Bus gives no
        // authenticated identity, so this only binds the observable arm to the
        // phone the user consented to — it is not a cryptographic check.
        if let Some(bound) = arm.phone.as_deref() {
            if bound != request.phone {
                self.record_drop(
                    persist,
                    &request,
                    now,
                    "dropped_source_mismatch",
                    "injection phone does not match armed phone",
                );
                return;
            }
        }

        // The Bus spool is intentionally writable across UIDs. Pairing and the
        // seated-user arm are useful safety signals, but neither is an
        // administrative identity. Require the root-only, exact-body capability
        // before the request reaches the root/uinput injector. The request's
        // phone is part of the semantic target as well as the canonical body
        // digest, so a token for one paired device cannot authorize another.
        let context = MutationContext {
            verb: REMOTE_INPUT_AUTH_VERB,
            node: &self.node,
            target: &request.phone,
        };
        if let Err(reason) = self.authorizer.authorize(body, context) {
            let reason = format!("remote-input authorization refused: {reason}");
            self.record_drop(persist, &request, now, "dropped_unauthorized", &reason);
            return;
        }

        self.status.accepted = self.status.accepted.saturating_add(1);
        self.status.last_request_id = Some(request.id.clone());
        self.status.last_phone = Some(request.phone.clone());
        self.status.last_kind = Some(request.event.kind_name().to_owned());
        self.status.last_event_ms = Some(now);
        self.status.updated_ms = now;

        match self.injector.inject(&request.event) {
            Ok(()) => {
                self.status.injected = self.status.injected.saturating_add(1);
                self.status.state = "injected".to_owned();
                self.status.last_error = None;
                self.last_injected_ms = Some(now);
                self.publish_event(persist, &request, now, "injected", None);
                self.refresh_indicator(persist);
            }
            Err(InputInjectError::Unavailable(e)) => {
                self.status.failed = self.status.failed.saturating_add(1);
                self.status.state = "unavailable".to_owned();
                self.status.last_error = Some(e.clone());
                self.publish_event(persist, &request, now, "unavailable", Some(&e));
            }
            Err(InputInjectError::Failed(e)) => {
                self.status.failed = self.status.failed.saturating_add(1);
                self.status.state = "error".to_owned();
                self.status.last_error = Some(e.clone());
                self.publish_event(persist, &request, now, "error", Some(&e));
            }
        }
        self.publish_status(persist);
    }

    /// Record a consent-gated drop: no injection happened, but leave a durable
    /// audit row and updated status so the refusal is observable.
    fn record_drop(
        &mut self,
        persist: &Persist,
        request: &RemoteInputRequest,
        now: u64,
        result: &str,
        reason: &str,
    ) {
        self.status.dropped = self.status.dropped.saturating_add(1);
        self.status.state = "dropped".to_owned();
        self.status.last_request_id = Some(request.id.clone());
        self.status.last_phone = Some(request.phone.clone());
        self.status.last_kind = Some(request.event.kind_name().to_owned());
        self.status.last_error = Some(reason.to_owned());
        self.status.last_event_ms = Some(now);
        self.status.updated_ms = now;
        tracing::warn!(
            target: "mackesd::seat_remote_input",
            request_id = %request.id,
            phone = %request.phone,
            result,
            "dropped remote-input injection: {reason}"
        );
        self.publish_event(persist, request, now, result, Some(reason));
        self.publish_status(persist);
    }

    /// Drain seated-user arm/disarm consent grants from [`ARM_TOPIC`].
    fn drain_arm(&mut self, persist: &Persist) {
        let msgs = match persist.list_since(ARM_TOPIC, self.arm_cursor.as_deref()) {
            Ok(msgs) => msgs,
            Err(e) => {
                tracing::debug!(target: "mackesd::seat_remote_input", error = %e, "arm list_since failed");
                return;
            }
        };
        for msg in msgs {
            self.arm_cursor = Some(msg.ulid.clone());
            let body = msg.body.unwrap_or_default();
            match parse_arm(&body) {
                Ok(ArmCommand::Arm {
                    ttl_ms,
                    source,
                    phone,
                }) => {
                    let now = self.now_ms();
                    let ttl = ttl_ms.min(MAX_ARM_TTL_MS);
                    let armed_until_ms = now.saturating_add(ttl);
                    self.arm = Some(ArmState {
                        armed_until_ms,
                        source: source.clone(),
                        phone: phone.clone(),
                    });
                    tracing::info!(
                        target: "mackesd::seat_remote_input",
                        %source, armed_until_ms,
                        "seat armed for remote input"
                    );
                    self.publish_arm_audit(
                        persist,
                        "arm",
                        &source,
                        phone.as_deref(),
                        Some(armed_until_ms),
                        now,
                    );
                    self.refresh_indicator(persist);
                }
                Ok(ArmCommand::Disarm { source }) => {
                    if self.arm.take().is_some() {
                        let now = self.now_ms();
                        tracing::info!(target: "mackesd::seat_remote_input", %source, "seat disarmed");
                        self.publish_arm_audit(persist, "disarm", &source, None, None, now);
                        self.refresh_indicator(persist);
                    }
                }
                Err(e) => {
                    tracing::debug!(target: "mackesd::seat_remote_input", error = %e, "ignored malformed arm control");
                }
            }
        }
    }

    /// Auto-disarm once the live grant's TTL has elapsed, auditing the lapse.
    fn expire_arm(&mut self, persist: &Persist) {
        let now = self.now_ms();
        let expired = self.arm.as_ref().is_some_and(|a| now >= a.armed_until_ms);
        if expired {
            let source = self
                .arm
                .as_ref()
                .map(|a| a.source.clone())
                .unwrap_or_default();
            self.arm = None;
            tracing::info!(target: "mackesd::seat_remote_input", %source, "seat arm expired (TTL)");
            self.publish_arm_audit(persist, "disarm_ttl", &source, None, None, now);
            self.refresh_indicator(persist);
        }
    }

    /// Publish the authoritative remote-input indicator, but only when a real
    /// transition (armed/active/source/phone/expiry) has occurred.
    fn refresh_indicator(&mut self, persist: &Persist) {
        let now = self.now_ms();
        let live = self.arm.as_ref().filter(|a| now < a.armed_until_ms);
        let active = live.is_some()
            && self
                .last_injected_ms
                .is_some_and(|t| now.saturating_sub(t) <= ACTIVE_WINDOW_MS);
        let indicator = RemoteInputIndicator {
            node: self.node.clone(),
            armed: live.is_some(),
            active,
            source: live.map(|a| a.source.clone()),
            phone: live.and_then(|a| a.phone.clone()),
            armed_until_ms: live.map(|a| a.armed_until_ms),
            updated_ms: now,
        };
        let changed = self.last_indicator.as_ref().map_or(true, |prev| {
            prev.armed != indicator.armed
                || prev.active != indicator.active
                || prev.source != indicator.source
                || prev.phone != indicator.phone
                || prev.armed_until_ms != indicator.armed_until_ms
        });
        if changed {
            let topic = format!("{INDICATOR_PREFIX}{}", self.node);
            if let Ok(body) = serde_json::to_string(&indicator) {
                let _ = persist.write(&topic, Priority::Min, None, Some(&body));
            }
            self.last_indicator = Some(indicator);
        }
    }

    /// Durable audit row for an arm-state transition.
    fn publish_arm_audit(
        &self,
        persist: &Persist,
        action: &str,
        source: &str,
        phone: Option<&str>,
        armed_until_ms: Option<u64>,
        now: u64,
    ) {
        let topic = format!("{RESULT_PREFIX}{}", self.node);
        let body = serde_json::json!({
            "op": "seat_remote_input_arm",
            "source": "seat_remote_input",
            "node": self.node,
            "action": action,
            "arm_source": source,
            "phone": phone,
            "armed_until_ms": armed_until_ms,
            "updated_ms": now,
        })
        .to_string();
        let _ = persist.write(&topic, Priority::Default, None, Some(&body));
    }

    /// One consume-drain-publish cycle: refresh arm state, expire stale grants,
    /// drain pending injections through the consent gate, and republish state.
    fn tick_once(&mut self, persist: &mut Persist) {
        persist.reopen_if_index_changed();
        self.drain_arm(persist);
        self.expire_arm(persist);
        self.drain_requests(persist);
        self.refresh_indicator(persist);
        self.publish_status(persist);
    }

    fn publish_status(&mut self, persist: &Persist) {
        if self.last_published_status.as_ref() == Some(&self.status) {
            return;
        }
        let topic = format!("{STATE_PREFIX}{}", self.node);
        if let Ok(body) = serde_json::to_string(&self.status) {
            let _ = persist.write(&topic, Priority::Min, None, Some(&body));
        }
        self.last_published_status = Some(self.status.clone());
    }

    fn publish_event(
        &self,
        persist: &Persist,
        request: &RemoteInputRequest,
        applied_ms: u64,
        result: &str,
        error: Option<&str>,
    ) {
        let topic = format!("{RESULT_PREFIX}{}", self.node);
        let body = serde_json::json!({
            "op": "seat_remote_input",
            "source": "seat_remote_input",
            "node": self.node,
            "request_id": &request.id,
            "phone": &request.phone,
            "kind": request.event.kind_name(),
            "event": &request.event,
            "result": result,
            "error": error,
            "phone_ts_unix_ms": request.ts_unix_ms,
            "applied_ms": applied_ms,
            "updated_ms": self.now_ms(),
        })
        .to_string();
        let _ = persist.write(&topic, Priority::Default, None, Some(&body));
    }
}

#[async_trait::async_trait]
impl Worker for SeatRemoteInputWorker {
    fn name(&self) -> &'static str {
        "seat_remote_input"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::seat_remote_input", "no bus root; worker idle");
            return Ok(());
        };
        let mut persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::seat_remote_input", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.publish_status(&persist);
        // Publish the initial (disarmed) indicator so a shell overlay has a
        // baseline "not being driven" state to render from.
        self.refresh_indicator(&persist);
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once(&mut persist);
                }
                () = shutdown.wait() => break,
            }
        }
        // Fail closed on shutdown: drop any live arm and clear its indicator so
        // a restart never inherits a stale "armed" grant.
        self.arm = None;
        self.refresh_indicator(&persist);
        self.publish_status(&persist);
        Ok(())
    }
}

fn parse_request(body: &str, id: &str) -> Result<RemoteInputRequest, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("remote-input JSON: {e}"))?;
    if v.get("op").and_then(serde_json::Value::as_str) != Some("kdc_remote_input") {
        return Err("wrong op".to_owned());
    }
    if v.get("source").and_then(serde_json::Value::as_str) != Some("kdc_host") {
        return Err("wrong source".to_owned());
    }
    let phone = required_string(&v, "phone", MAX_PHONE_CHARS)?;
    if !valid_phone(&phone) {
        return Err("invalid phone".to_owned());
    }
    let ts_unix_ms = v
        .get("ts_unix_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "missing ts_unix_ms".to_owned())?;
    let kind = v
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing kind".to_owned())?;
    let event = match kind {
        "move" => SeatRemoteInputEvent::Move {
            dx: required_delta(&v, "dx")?,
            dy: required_delta(&v, "dy")?,
        },
        "scroll" => {
            let delta = required_delta(&v, "delta")?;
            if delta == 0.0 {
                return Err("zero scroll".to_owned());
            }
            SeatRemoteInputEvent::Scroll { delta }
        }
        "button" => {
            let button = required_string(&v, "button", 16)?;
            if !matches!(button.as_str(), "primary" | "secondary" | "middle") {
                return Err("invalid button".to_owned());
            }
            let clicks = v
                .get("clicks")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "missing clicks".to_owned())?;
            if !(1..=2).contains(&clicks) {
                return Err("invalid clicks".to_owned());
            }
            SeatRemoteInputEvent::Button {
                button,
                clicks: u8::try_from(clicks).unwrap_or(1),
            }
        }
        "text" => {
            let text = required_string(&v, "text", MAX_TEXT_CHARS)?;
            if text.is_empty() {
                return Err("empty text".to_owned());
            }
            SeatRemoteInputEvent::Text {
                text,
                modifiers: modifiers(&v),
            }
        }
        "special_key" => {
            let code = v
                .get("special_key")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| "missing special_key".to_owned())?;
            if !(0..=255).contains(&code) {
                return Err("invalid special_key".to_owned());
            }
            SeatRemoteInputEvent::SpecialKey {
                special_key: code,
                modifiers: modifiers(&v),
            }
        }
        _ => return Err("unsupported kind".to_owned()),
    };

    Ok(RemoteInputRequest {
        id: id.to_owned(),
        phone,
        ts_unix_ms,
        event,
    })
}

fn parse_arm(body: &str) -> Result<ArmCommand, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("arm JSON: {e}"))?;
    let op = v
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing op".to_owned())?;
    let source = required_string(&v, "source", MAX_PHONE_CHARS)?;
    if source.is_empty() {
        return Err("empty source".to_owned());
    }
    match op {
        "arm" | "seat_remote_input_arm" => {
            let ttl_ms = v
                .get("ttl_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(DEFAULT_ARM_TTL_MS);
            if ttl_ms == 0 {
                return Err("zero ttl".to_owned());
            }
            let phone = match v.get("phone") {
                None | Some(serde_json::Value::Null) => None,
                Some(_) => {
                    let p = required_string(&v, "phone", MAX_PHONE_CHARS)?;
                    if !valid_phone(&p) {
                        return Err("invalid phone".to_owned());
                    }
                    Some(p)
                }
            };
            Ok(ArmCommand::Arm {
                ttl_ms,
                source,
                phone,
            })
        }
        "disarm" | "seat_remote_input_disarm" => Ok(ArmCommand::Disarm { source }),
        _ => Err("unsupported arm op".to_owned()),
    }
}

fn required_string(v: &serde_json::Value, key: &str, max_chars: usize) -> Result<String, String> {
    let value = v
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("missing {key}"))?;
    let trimmed = value.trim();
    if trimmed.chars().count() > max_chars {
        return Err(format!("{key} is too long"));
    }
    Ok(trimmed.to_owned())
}

fn required_delta(v: &serde_json::Value, key: &str) -> Result<f64, String> {
    let value = v
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("missing {key}"))?;
    if !value.is_finite() || value.abs() > MAX_DELTA {
        return Err(format!("invalid {key}"));
    }
    Ok(value)
}

fn modifiers(v: &serde_json::Value) -> RemoteInputModifiers {
    let m = v.get("modifiers").unwrap_or(&serde_json::Value::Null);
    RemoteInputModifiers {
        shift: m
            .get("shift")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        ctrl: m
            .get("ctrl")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        alt: m
            .get("alt")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        super_key: m
            .get("super")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

fn valid_phone(phone: &str) -> bool {
    !phone.is_empty()
        && phone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
}

fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use crate::ipc::action_auth::{authorize_test_body, MutationContext};

    use super::*;

    const AUTH_KEY: &[u8] = b"seat-remote-input-test-key";
    const AUTH_NOW: i64 = 1_000;

    #[derive(Default)]
    struct RecordingInjector {
        calls: Mutex<Vec<SeatRemoteInputEvent>>,
        error: Option<InputInjectError>,
    }

    /// Deterministic advanceable clock for TTL/expiry tests.
    fn clock(start: u64) -> (Arc<AtomicU64>, NowFn) {
        let c = Arc::new(AtomicU64::new(start));
        let reader = c.clone();
        (c, Arc::new(move || reader.load(Ordering::SeqCst)))
    }

    /// An unbound arm grant with the given TTL.
    fn arm_body(ttl_ms: u64) -> String {
        serde_json::json!({
            "op": "arm",
            "source": "shell:seat-user",
            "ttl_ms": ttl_ms,
        })
        .to_string()
    }

    fn live_arm() -> Option<ArmState> {
        Some(ArmState {
            armed_until_ms: u64::MAX,
            source: "shell:test".to_owned(),
            phone: None,
        })
    }

    fn latest_indicator(persist: &Persist, node: &str) -> RemoteInputIndicator {
        let topic = format!("{INDICATOR_PREFIX}{node}");
        let rows = persist.list_since(&topic, None).expect("indicator rows");
        let body = rows
            .last()
            .expect("an indicator row")
            .body
            .clone()
            .expect("body");
        serde_json::from_str(&body).expect("indicator JSON")
    }

    impl SeatInputInjector for RecordingInjector {
        fn inject(&self, event: &SeatRemoteInputEvent) -> Result<(), InputInjectError> {
            self.calls.lock().unwrap().push(event.clone());
            if let Some(error) = self.error.clone() {
                Err(error)
            } else {
                Ok(())
            }
        }
    }

    fn move_body() -> String {
        serde_json::json!({
            "op": "kdc_remote_input",
            "source": "kdc_host",
            "phone": "phone-1",
            "kind": "move",
            "dx": 12.5,
            "dy": -2.0,
            "ts_unix_ms": 12345_u64,
        })
        .to_string()
    }

    fn test_authorizer(root: &std::path::Path) -> Arc<ActionAuthorizer> {
        Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            root.join("auth"),
            AUTH_NOW,
        ))
    }

    fn signed_body(node: &str, raw: &str, nonce: &str, expires_at_ms: i64) -> String {
        let mut value: serde_json::Value = serde_json::from_str(raw).expect("request JSON");
        value["schema_version"] = serde_json::json!(crate::ipc::action_auth::ACTION_SCHEMA_VERSION);
        let unsigned = value.to_string();
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: REMOTE_INPUT_AUTH_VERB,
                node,
                target: "phone-1",
            },
            nonce,
            expires_at_ms,
        )
    }

    fn signed_move_body(node: &str, nonce: &str) -> String {
        signed_body(node, &move_body(), nonce, AUTH_NOW + 30_000)
    }

    #[test]
    fn parse_request_accepts_kdc_motion_click_and_text_payloads() {
        let request = parse_request(&move_body(), "req-1").expect("valid move");
        assert_eq!(request.phone, "phone-1");
        assert_eq!(request.ts_unix_ms, 12345);
        assert_eq!(
            request.event,
            SeatRemoteInputEvent::Move { dx: 12.5, dy: -2.0 }
        );

        let click = parse_request(
            &serde_json::json!({
                "op": "kdc_remote_input",
                "source": "kdc_host",
                "phone": "phone-1",
                "kind": "button",
                "button": "secondary",
                "clicks": 1,
                "ts_unix_ms": 2,
            })
            .to_string(),
            "req-2",
        )
        .expect("valid click");
        assert_eq!(
            click.event,
            SeatRemoteInputEvent::Button {
                button: "secondary".into(),
                clicks: 1,
            }
        );

        let text = parse_request(
            &serde_json::json!({
                "op": "kdc_remote_input",
                "source": "kdc_host",
                "phone": "phone-1",
                "kind": "text",
                "text": "A",
                "modifiers": {"shift": true, "ctrl": true},
                "ts_unix_ms": 3,
            })
            .to_string(),
            "req-3",
        )
        .expect("valid text");
        assert_eq!(
            text.event,
            SeatRemoteInputEvent::Text {
                text: "A".into(),
                modifiers: RemoteInputModifiers {
                    shift: true,
                    ctrl: true,
                    ..Default::default()
                },
            }
        );
    }

    #[test]
    fn parse_request_rejects_untrusted_or_out_of_bounds_payloads() {
        assert!(parse_request(r#"{"op":"wrong"}"#, "x").is_err());
        assert!(parse_request(&move_body().replace("12.5", "5000.0"), "x").is_err());
        assert!(parse_request(&move_body().replace("phone-1", "../bad"), "x").is_err());
        assert!(parse_request(
            &serde_json::json!({
                "op": "kdc_remote_input",
                "source": "kdc_host",
                "phone": "phone-1",
                "kind": "text",
                "text": "this-token-is-far-too-long",
                "ts_unix_ms": 3,
            })
            .to_string(),
            "x",
        )
        .is_err());
    }

    #[test]
    fn apply_request_injects_and_publishes_status_and_event() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(Arc::new(|| 777))
            .with_authorizer(test_authorizer(bus.path()));
        worker.arm = live_arm();
        let body = signed_move_body("node-a", "apply-request-1");
        let request = parse_request(&body, "req-1").expect("request");

        worker.apply_request(&persist, request, &body);

        assert_eq!(
            *injector.calls.lock().unwrap(),
            vec![SeatRemoteInputEvent::Move { dx: 12.5, dy: -2.0 }]
        );
        let status_body = persist
            .list_since("state/seat-remote-input/node-a", None)
            .expect("status")[0]
            .body
            .clone()
            .expect("body");
        let status: RemoteInputStatus = serde_json::from_str(&status_body).expect("status JSON");
        assert_eq!(status.state, "injected");
        assert_eq!(status.accepted, 1);
        assert_eq!(status.injected, 1);
        assert_eq!(status.last_kind.as_deref(), Some("move"));

        let event_body = persist
            .list_since("event/seat-remote-input/node-a", None)
            .expect("event")[0]
            .body
            .clone()
            .expect("body");
        let event: serde_json::Value = serde_json::from_str(&event_body).expect("event JSON");
        assert_eq!(event["op"], "seat_remote_input");
        assert_eq!(event["request_id"], "req-1");
        assert_eq!(event["result"], "injected");
        assert_eq!(event["kind"], "move");
    }

    #[test]
    fn unavailable_injector_is_honest_state_not_fake_success() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let injector = Arc::new(RecordingInjector {
            calls: Mutex::default(),
            error: Some(InputInjectError::Unavailable("no helper".into())),
        });
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 888))
            .with_authorizer(test_authorizer(bus.path()));
        worker.arm = live_arm();
        let body = signed_move_body("node-a", "unavailable-1");
        let request = parse_request(&body, "req-1").expect("request");

        worker.apply_request(&persist, request, &body);

        assert_eq!(worker.status.state, "unavailable");
        assert_eq!(worker.status.failed, 1);
        assert_eq!(worker.status.injected, 0);
        let event_body = persist
            .list_since("event/seat-remote-input/node-a", None)
            .expect("event")[0]
            .body
            .clone()
            .expect("body");
        let event: serde_json::Value = serde_json::from_str(&event_body).expect("event JSON");
        assert_eq!(event["result"], "unavailable");
        assert_eq!(event["error"], "no helper");
    }

    #[test]
    fn drain_requests_tracks_rejections_and_does_not_replay() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"op":"wrong"}"#),
            )
            .expect("write bad");
        let body = signed_move_body("node-a", "drain-1");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("write good");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 999))
            .with_authorizer(test_authorizer(bus.path()));
        // Armed so the well-formed request is dispatched (not consent-dropped),
        // keeping this test focused on malformed-rejection + no-replay.
        worker.arm = live_arm();

        worker.drain_requests(&persist);
        worker.drain_requests(&persist);

        assert_eq!(worker.status.rejected, 1);
        assert_eq!(worker.status.accepted, 1);
        let events = persist
            .list_since("event/seat-remote-input/node-a", None)
            .expect("events");
        assert_eq!(events.len(), 1, "cursor prevents replay");
    }

    #[test]
    fn unarmed_seat_drops_injection_and_audits() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&move_body()))
            .expect("write injection");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(Arc::new(|| 1000));

        // No arm grant present.
        worker.tick_once(&mut persist);

        assert!(
            injector.calls.lock().unwrap().is_empty(),
            "un-armed seat must not reach the uinput injector"
        );
        assert_eq!(worker.status.dropped, 1);
        assert_eq!(worker.status.injected, 0);
        assert_eq!(worker.status.accepted, 0);

        let event_body = persist
            .list_since("event/seat-remote-input/node-a", None)
            .expect("event")[0]
            .body
            .clone()
            .expect("body");
        let event: serde_json::Value = serde_json::from_str(&event_body).expect("event JSON");
        assert_eq!(event["result"], "dropped_unarmed");
        assert_eq!(event["error"], "seat not armed for remote input");
    }

    #[test]
    fn armed_seat_delivers_phone_injection() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        persist
            .write(ARM_TOPIC, Priority::Default, None, Some(&arm_body(60_000)))
            .expect("write arm");
        let body = signed_move_body("node-a", "armed-1");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("write injection");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(Arc::new(|| 1000))
            .with_authorizer(test_authorizer(bus.path()));

        worker.tick_once(&mut persist);

        assert_eq!(
            *injector.calls.lock().unwrap(),
            vec![SeatRemoteInputEvent::Move { dx: 12.5, dy: -2.0 }],
            "armed seat delivers the injection"
        );
        assert_eq!(worker.status.injected, 1);
        assert_eq!(worker.status.dropped, 0);
    }

    #[test]
    fn hostile_handoffs_never_reach_uinput() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let unsigned = {
            let mut value: serde_json::Value = serde_json::from_str(&move_body()).unwrap();
            value["schema_version"] =
                serde_json::json!(crate::ipc::action_auth::ACTION_SCHEMA_VERSION);
            value.to_string()
        };
        let expired = signed_body("node-a", &move_body(), "expired-1", AUTH_NOW - 1);
        let valid = signed_move_body("node-a", "replay-1");
        let mut tampered_value: serde_json::Value = serde_json::from_str(&valid).unwrap();
        tampered_value["dx"] = serde_json::json!(99.0);
        let tampered = tampered_value.to_string();

        for body in [&unsigned, &expired, &valid, &valid, &tampered] {
            persist
                .write(ACTION_TOPIC, Priority::Default, None, Some(body))
                .expect("write hostile handoff");
        }

        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(Arc::new(|| AUTH_NOW as u64))
            .with_authorizer(test_authorizer(bus.path()));
        worker.arm = live_arm();
        worker.tick_once(&mut persist);

        assert_eq!(
            worker.status.accepted, 1,
            "only the first valid handoff applies"
        );
        assert_eq!(worker.status.injected, 1);
        assert_eq!(worker.status.dropped, 4);
        assert_eq!(
            injector.calls.lock().unwrap().len(),
            1,
            "unsigned, expired, replayed, and body-tampered rows never reach uinput"
        );
        let events = persist
            .list_since("event/seat-remote-input/node-a", None)
            .expect("audit events");
        let errors: Vec<String> = events
            .iter()
            .filter_map(|row| row.body.as_deref())
            .filter_map(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .filter_map(|body| body["error"].as_str().map(str::to_owned))
            .collect();
        assert!(errors.iter().any(|e| e.contains("no armed token")));
        assert!(errors.iter().any(|e| e.contains("expired")));
        assert!(errors.iter().any(|e| e.contains("already used")));
        assert!(errors.iter().any(|e| e.contains("request body")));
    }

    #[test]
    fn oversized_handoff_is_rejected_before_json_parse() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let body = format!(
            r#"{{"op":"kdc_remote_input","padding":"{}"}}"#,
            "x".repeat(65_537)
        );
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("write oversized handoff");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(Arc::new(|| AUTH_NOW as u64))
            .with_authorizer(test_authorizer(bus.path()));
        worker.tick_once(&mut persist);
        assert_eq!(worker.status.rejected, 1);
        assert_eq!(worker.status.dropped, 0);
        assert!(injector.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn arm_auto_disarms_after_ttl() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let (clk, now_fn) = clock(1_000);
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector.clone())
            .with_now_fn(now_fn)
            .with_authorizer(test_authorizer(bus.path()));

        // Arm for 5s, then inject once inside the window -> delivered.
        persist
            .write(ARM_TOPIC, Priority::Default, None, Some(&arm_body(5_000)))
            .expect("write arm");
        let body = signed_move_body("node-a", "ttl-1");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("write injection 1");
        worker.tick_once(&mut persist);
        assert_eq!(worker.status.injected, 1);
        assert!(worker.arm.is_some());

        // Advance past the TTL, inject again -> auto-disarmed, dropped.
        clk.store(1_000 + 5_000 + 1, Ordering::SeqCst);
        let body = signed_move_body("node-a", "ttl-2");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("write injection 2");
        worker.tick_once(&mut persist);

        assert_eq!(
            worker.status.injected, 1,
            "no injection after the TTL lapses"
        );
        assert_eq!(worker.status.dropped, 1);
        assert_eq!(injector.calls.lock().unwrap().len(), 1);
        assert!(worker.arm.is_none(), "grant auto-disarmed");
        assert!(!latest_indicator(&persist, "node-a").armed);
    }

    #[test]
    fn indicator_published_on_arm_and_cleared_on_disarm() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 1_000));

        // Baseline: worker publishes a disarmed indicator up front.
        worker.refresh_indicator(&persist);
        assert!(!latest_indicator(&persist, "node-a").armed);

        // Arm -> indicator reports armed + the controlling source.
        persist
            .write(ARM_TOPIC, Priority::Default, None, Some(&arm_body(60_000)))
            .expect("write arm");
        worker.tick_once(&mut persist);
        let armed = latest_indicator(&persist, "node-a");
        assert!(armed.armed);
        assert_eq!(armed.source.as_deref(), Some("shell:seat-user"));
        assert_eq!(armed.armed_until_ms, Some(1_000 + 60_000));

        // Disarm -> indicator cleared.
        persist
            .write(
                ARM_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::json!({"op":"disarm","source":"shell:seat-user"}).to_string()),
            )
            .expect("write disarm");
        worker.tick_once(&mut persist);
        let cleared = latest_indicator(&persist, "node-a");
        assert!(!cleared.armed);
        assert_eq!(cleared.source, None);
    }

    #[test]
    fn tick_reopens_recreated_bus_index_before_publishing_status() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let first = persist.index_inode();
        let db = bus.path().join("index.sqlite");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
        let live_index = Persist::open(bus.path().to_path_buf()).expect("live index");
        assert_ne!(
            live_index.index_inode(),
            first,
            "test must recreate the index inode"
        );
        drop(live_index);

        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 1_000));

        worker.tick_once(&mut persist);

        assert!(
            Persist::open(bus.path().to_path_buf())
                .expect("reader")
                .detect_divergence()
                .expect("divergence")
                .is_clean(),
            "status publish must land in the live index, not a deleted inode"
        );
    }

    #[test]
    fn idle_tick_publishes_status_once_not_every_poll() {
        let bus = tempfile::tempdir().expect("bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 1_000));

        worker.tick_once(&mut persist);
        worker.tick_once(&mut persist);

        assert_eq!(
            persist
                .list_since("state/seat-remote-input/node-a", None)
                .expect("status")
                .len(),
            1,
            "idle polling should not flood retained status"
        );
    }

    #[test]
    fn command_injector_runs_configured_helper() {
        let injector = CommandSeatInputInjector::with_helper(PathBuf::from("/bin/true"));
        assert!(injector
            .inject(&SeatRemoteInputEvent::Button {
                button: "primary".into(),
                clicks: 1,
            })
            .is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn command_injector_maps_helper_unavailable_exit_to_unavailable() {
        use std::os::unix::process::ExitStatusExt;

        let err = classify_helper_status(
            std::path::Path::new("/usr/libexec/mackesd/seat-remote-input"),
            ExitStatus::from_raw(69 << 8),
        )
        .expect_err("exit 69 is unavailable");
        assert!(matches!(err, InputInjectError::Unavailable(_)));
    }
}
