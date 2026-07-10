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

use super::proc::status_with_timeout;
use super::{ShutdownToken, Worker};

/// KDC-owned remote-input handoff topic.
pub const ACTION_TOPIC: &str = "action/seat/remote-input";

/// Retained-latest status topic prefix for this node.
pub const STATE_PREFIX: &str = "state/seat-remote-input/";

/// Per-event injection result topic prefix for this node.
pub const RESULT_PREFIX: &str = "event/seat-remote-input/";

/// Default poll cadence. Phone touchpad input is interactive, so stay below the
/// control-poller cadence while still using the Bus as the handoff contract.
pub const DEFAULT_TICK: Duration = Duration::from_millis(40);

const MAX_PHONE_CHARS: usize = 128;
const MAX_TEXT_CHARS: usize = 16;
const MAX_DELTA: f64 = 4096.0;
const DEFAULT_HELPER: &str = "/usr/libexec/mackesd/seat-remote-input";
const ENV_HELPER: &str = "MDE_SEAT_REMOTE_INPUT_COMMAND";
const INJECT_CMD_TIMEOUT: Duration = Duration::from_millis(500);

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
    /// Valid requests whose injector failed or was unavailable.
    pub failed: u64,
    /// Timestamp of the most recent accepted request.
    pub last_event_ms: Option<u64>,
    /// Timestamp of the most recent status publication.
    pub updated_ms: u64,
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
    status: RemoteInputStatus,
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
                failed: 0,
                last_event_ms: None,
                updated_ms,
            },
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
            match parse_request(&body, &msg.ulid) {
                Ok(request) => self.apply_request(persist, request),
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

    fn apply_request(&mut self, persist: &Persist, request: RemoteInputRequest) {
        let now = self.now_ms();
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
                self.publish_event(persist, &request, now, "injected", None);
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

    fn publish_status(&self, persist: &Persist) {
        let topic = format!("{STATE_PREFIX}{}", self.node);
        if let Ok(body) = serde_json::to_string(&self.status) {
            let _ = persist.write(&topic, Priority::Min, None, Some(&body));
        }
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
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::seat_remote_input", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.publish_status(&persist);
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_requests(&persist);
                    self.publish_status(&persist);
                }
                () = shutdown.wait() => break,
            }
        }
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
    use std::sync::Mutex;

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::*;

    #[derive(Default)]
    struct RecordingInjector {
        calls: Mutex<Vec<SeatRemoteInputEvent>>,
        error: Option<InputInjectError>,
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
            .with_now_fn(Arc::new(|| 777));
        let request = parse_request(&move_body(), "req-1").expect("request");

        worker.apply_request(&persist, request);

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
            .with_now_fn(Arc::new(|| 888));
        let request = parse_request(&move_body(), "req-1").expect("request");

        worker.apply_request(&persist, request);

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
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&move_body()))
            .expect("write good");
        let injector = Arc::new(RecordingInjector::default());
        let mut worker = SeatRemoteInputWorker::with_injector("node-a".into(), injector)
            .with_now_fn(Arc::new(|| 999));

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
