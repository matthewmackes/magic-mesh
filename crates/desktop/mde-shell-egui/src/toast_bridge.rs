//! `toast_bridge` — the shell side of the **KIRON** alert/OSD lanes (KIRON-2;
//! `docs/design/kiron-toast-pattern.md`, locks 7/8/10).
//!
//! KIRON-1 built the pure `mde_egui::toast::ToastHost` (queue + dwell + OSD
//! render). This module is the shell's owner of that one host: it
//!
//! * subscribes the typed Bus lane [`TOAST_TOPIC`] (`event/toast/show`) so any
//!   node / worker — `mackesd`, a remote peer — can raise an alert fleet-wide
//!   (lock 7), decoding each body into an alert [`Toast`];
//! * drives the host once per frame ([`ToastBridge::drive`]): `tick` the real
//!   frame delta, drain the lane, then paint the top-center alert banner and
//!   centered OSD;
//! * fires **one** severity-scaled notification sound on a new alert (lock 8),
//!   the single sound authority — no double-beeps;
//! * applies **suppression** (lock 10): DND / a per-VM-session focus mute silence
//!   an Info/Warning ambient alert *and* its sound, audio-mute silences a non-critical's
//!   sound, but a **Critical always breaks through**; and
//! * keeps the action-verb grammar centralized for banner and Chat inline
//!   notification actions ([`resolve_action`]).
//!
//! The wire body is a JSON boundary (local serde structs, not a `mackesd`
//! dependency — §6 mesh/desktop boundary), the same pattern the Fleet plane and
//! the Chat surface use for their topics.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mackes_mesh_types::health::{HealthKironAlert, HealthKironAttention, HealthKironDwell};
use mde_bus::persist::Persist;
use mde_egui::egui;
use mde_egui::{Dwell, OsdLevel, Severity, Style, Tier, Toast, ToastHost};
use serde::Deserialize;

use crate::notification_center::NotificationRing;
use crate::surfaces::Surface;
use crate::timers::{
    clock_banner_projection, request_clock_banner_action, ClockBannerKind, ClockBannerProjection,
};
use crate::workbench::Plane;
use crate::workers_catalog::WorkersDestination;

/// The typed Bus lane any node / worker raises an alert on (lock 7). Flat — the
/// originating host rides the body's `source_host`, never the topic.
pub(crate) const TOAST_TOPIC: &str = "event/toast/show";

/// Poll cadence for the alert lane. Shorter than the 5s status cadence the info
/// surfaces use — an alert is time-sensitive, and the read is a cheap incremental
/// cursor scan. (The OSD tier is a direct call, never this lane — lock 7.)
const REFRESH: Duration = Duration::from_secs(1);

/// The wire body of an `event/toast/show` message — a JSON boundary mirrored with
/// a local serde struct so the shell never depends on the emitter's crate (§6).
///
/// `{ "severity": "info|warning|critical", "source_host": "nyc3", "flag":
/// "SECURITY", "headline": "…", "action_label": "Open", "action_verb":
/// "shell/goto/chat" }` — `action_*` optional (both or neither).
#[derive(Debug, Clone, Deserialize)]
struct ToastMsg {
    /// The alert severity (drives color + dwell + preempt).
    severity: WireSeverity,
    /// The originating hostname (mesh identity). Empty for an anonymous raise.
    #[serde(default)]
    source_host: String,
    /// The category flag chip — `SECURITY` / `BUILD` / `CHAT` / …
    #[serde(default)]
    flag: String,
    /// The single-line headline shown in the band.
    headline: String,
    /// The optional click-through button caption.
    #[serde(default)]
    action_label: Option<String>,
    /// The optional opaque action verb ([`resolve_action`] runs it).
    #[serde(default)]
    action_verb: Option<String>,
}

/// The wire severity — a stable lowercase string contract, mapped onto the shared
/// [`Severity`] so the wire format never leaks the enum's discriminants.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireSeverity {
    /// Informational.
    Info,
    /// Worth noticing.
    Warning,
    /// Needs attention now — preempts + breaks through suppression.
    Critical,
}

impl WireSeverity {
    const fn severity(self) -> Severity {
        match self {
            Self::Info => Severity::Info,
            Self::Warning => Severity::Warning,
            Self::Critical => Severity::Critical,
        }
    }
}

impl ToastMsg {
    /// Fold the decoded wire body into an alert [`Toast`] (severity default dwell,
    /// plus the click-through action when both `action_*` fields are present).
    fn into_toast(self) -> Toast {
        let toast = Toast::alert(
            self.severity.severity(),
            self.source_host,
            self.flag,
            self.headline,
        );
        match (self.action_label, self.action_verb) {
            (Some(label), Some(verb)) => toast.with_action(label, verb),
            _ => toast,
        }
    }
}

/// Map one already-validated health-authority record into KIRON presentation.
/// Grade interpretation, duration derivation, and dwell policy stay in the
/// shared health contract; this boundary only selects ToastHost primitives.
fn health_kiron_toast(alert: HealthKironAlert) -> Toast {
    let severity = match alert.attention() {
        HealthKironAttention::Informational => Severity::Info,
        HealthKironAttention::Warning => Severity::Warning,
        HealthKironAttention::Critical => Severity::Critical,
    };
    let dwell = match alert.dwell() {
        HealthKironDwell::TimedMs(milliseconds) => Dwell::For(Duration::from_millis(milliseconds)),
        HealthKironDwell::UntilAcknowledged => Dwell::UntilAck,
    };
    let mut flag = format!(
        "HEALTH · GRADE {} · {}",
        alert.grade.as_str(),
        alert.duration_label()
    );
    if let Some(device) = &alert.device {
        flag.push_str(" · ");
        flag.push_str(device);
    }
    let condition_id = alert.condition_id;
    let snapshot_generation = alert.snapshot_generation;
    Toast::alert(severity, alert.node, flag, alert.headline)
        .with_dwell(dwell)
        .with_action("Open Health", "shell/goto/health")
        .with_health_authority(condition_id, snapshot_generation)
}

/// Decode a raw `event/toast/show` body into an alert [`Toast`]. `None` on a
/// malformed body — a bad emitter never crashes the shell (it's silently dropped,
/// same as the Clipboard / Notifications tails).
fn decode(body: &str) -> Option<Toast> {
    decode_at(body, current_unix_ms())
}

/// Decode at an explicit clock value so the health lower-third admission rule
/// can be tested without depending on wall-clock timing.
fn decode_at(body: &str, now_ms: u64) -> Option<Toast> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("health_kiron") {
        let alert: HealthKironAlert = serde_json::from_value(value).ok()?;
        alert.validate().ok()?;
        // UX-014 must not animate an alert from the future: doing so would
        // display a false lower third and make its duration/timeline depend on
        // a publisher clock that is ahead of the seat. UX-013 remains the sole
        // authority for the timestamps; this is presentation admission only.
        if alert.active_since_ms > now_ms || alert.observed_at_ms > now_ms {
            return None;
        }
        return Some(health_kiron_toast(alert));
    }
    serde_json::from_value::<ToastMsg>(value)
        .ok()
        .map(ToastMsg::into_toast)
}

fn current_unix_ms() -> u64 {
    u64::try_from(crate::timers::now_unix())
        .unwrap_or_default()
        .saturating_mul(1_000)
}

/// The alert severity a [`Toast`] carries, or `None` for the OSD tier (which never
/// rides the alert lane / never rings).
const fn alert_severity(toast: &Toast) -> Option<Severity> {
    match toast.tier {
        Tier::Alert(s) => Some(s),
        Tier::Osd(_) => None,
    }
}

/// The live suppression posture (lock 10), refreshed by the shell each frame.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Suppress {
    /// Do-Not-Disturb — silences an Info/Warning ambient alert + its sound.
    dnd: bool,
    /// A per-VM-session focus / gaming mute (a fullscreen guest is in front) —
    /// silences an Info/Warning ambient alert + its sound.
    focus_mute: bool,
    /// The seat's audio output is muted — additionally silences a non-critical's
    /// notification sound.
    muted: bool,
}

impl Suppress {
    /// Whether an alert of this severity's ambient push is suppressed. A Critical
    /// always breaks through (safety over immersion — lock 10).
    const fn hides_ambient_push(self, severity: Severity) -> bool {
        !matches!(severity, Severity::Critical) && (self.dnd || self.focus_mute)
    }

    /// Whether an alert of this severity's **sound** is suppressed. A Critical
    /// always rings; a non-critical is silenced by DND / focus-mute / audio-mute.
    const fn hushes_sound(self, severity: Severity) -> bool {
        !matches!(severity, Severity::Critical) && (self.dnd || self.focus_mute || self.muted)
    }
}

/// The single notification-sound seam (lock 8 — the `ToastHost` is the one sound
/// authority). Production spawns the freedesktop event sound; tests record.
pub(crate) trait Chime {
    /// Fire one notification sound scaled to the alert severity.
    fn ring(&self, severity: Severity);
}

/// The production chime — plays the freedesktop event sound, detached. An absent
/// or broken sound theme falls through to a short built-in WAV over the seat's
/// PipeWire/Pulse players. Every child is bounded so a wedged audio client cannot
/// strand the detached notification thread.
struct SystemChime;

/// Maximum wall time for one notification player. A failed/timed-out backend
/// advances to the next one; all three attempts therefore remain bounded.
const CHIME_PLAYER_TIMEOUT: Duration = Duration::from_millis(1_500);
const CHIME_PLAYER_POLL: Duration = Duration::from_millis(10);
const CHIME_SAMPLE_RATE: usize = 24_000;
const CHIME_SAMPLE_RATE_WAV: u32 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChimeBackend {
    Canberra,
    PipeWire,
    PulseAudio,
}

const CHIME_BACKENDS: [ChimeBackend; 3] = [
    ChimeBackend::Canberra,
    ChimeBackend::PipeWire,
    ChimeBackend::PulseAudio,
];

#[derive(Clone, Copy)]
struct ChimeTone {
    start_ms: usize,
    duration_ms: usize,
    frequency_hz: f32,
    mix: f32,
}

#[derive(Clone, Copy)]
struct ChimeSpec {
    duration_ms: usize,
    gain: f32,
    tones: &'static [ChimeTone],
}

const INFO_CHIME: [ChimeTone; 2] = [
    ChimeTone {
        start_ms: 0,
        duration_ms: 170,
        frequency_hz: 783.99,
        mix: 0.72,
    },
    ChimeTone {
        start_ms: 65,
        duration_ms: 185,
        frequency_hz: 1_046.50,
        mix: 0.55,
    },
];
const WARNING_CHIME: [ChimeTone; 2] = [
    ChimeTone {
        start_ms: 0,
        duration_ms: 190,
        frequency_hz: 659.25,
        mix: 0.78,
    },
    ChimeTone {
        start_ms: 105,
        duration_ms: 205,
        frequency_hz: 880.00,
        mix: 0.66,
    },
];
const CRITICAL_CHIME: [ChimeTone; 3] = [
    ChimeTone {
        start_ms: 0,
        duration_ms: 160,
        frequency_hz: 523.25,
        mix: 0.82,
    },
    ChimeTone {
        start_ms: 95,
        duration_ms: 170,
        frequency_hz: 783.99,
        mix: 0.76,
    },
    ChimeTone {
        start_ms: 190,
        duration_ms: 180,
        frequency_hz: 1_046.50,
        mix: 0.70,
    },
];

const fn chime_spec(severity: Severity) -> ChimeSpec {
    match severity {
        Severity::Info => ChimeSpec {
            duration_ms: 260,
            gain: 0.20,
            tones: &INFO_CHIME,
        },
        Severity::Warning => ChimeSpec {
            duration_ms: 320,
            gain: 0.27,
            tones: &WARNING_CHIME,
        },
        Severity::Critical => ChimeSpec {
            duration_ms: 380,
            gain: 0.34,
            tones: &CRITICAL_CHIME,
        },
    }
}

/// Build a self-contained mono PCM WAV. The ascending consonant intervals,
/// short attack, and quadratic decay keep the fallback audible without the
/// harsh square-wave character of a terminal bell.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn built_in_chime_wav(severity: Severity) -> Vec<u8> {
    let spec = chime_spec(severity);
    let sample_count = CHIME_SAMPLE_RATE * spec.duration_ms / 1_000;
    let pcm_bytes = sample_count * std::mem::size_of::<i16>();
    let Ok(pcm_bytes_u32) = u32::try_from(pcm_bytes) else {
        return Vec::new();
    };

    let mut wav = Vec::with_capacity(44 + pcm_bytes);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + pcm_bytes_u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&CHIME_SAMPLE_RATE_WAV.to_le_bytes());
    wav.extend_from_slice(&(CHIME_SAMPLE_RATE_WAV * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes()); // block alignment
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&pcm_bytes_u32.to_le_bytes());

    for sample_index in 0..sample_count {
        let mut mixed = 0.0_f32;
        for tone in spec.tones {
            let start = CHIME_SAMPLE_RATE * tone.start_ms / 1_000;
            let length = CHIME_SAMPLE_RATE * tone.duration_ms / 1_000;
            if sample_index < start || sample_index >= start + length {
                continue;
            }
            let local = sample_index - start;
            let attack = (CHIME_SAMPLE_RATE / 125).min(length / 4).max(1);
            let envelope = if local < attack {
                local as f32 / attack as f32
            } else {
                let decay = (length - local) as f32 / (length - attack) as f32;
                decay * decay
            };
            let phase =
                std::f32::consts::TAU * tone.frequency_hz * local as f32 / CHIME_SAMPLE_RATE as f32;
            // A quiet second harmonic adds a bell-like edge while the
            // fundamental remains dominant and easy on laptop speakers.
            let wave = phase.sin().mul_add(0.86, (phase * 2.0).sin() * 0.14);
            mixed += wave * envelope * tone.mix;
        }
        let sample = (mixed * spec.gain).clamp(-0.95, 0.95) * f32::from(i16::MAX);
        wav.extend_from_slice(&(sample.round() as i16).to_le_bytes());
    }
    wav
}

/// Run the ordered fallback policy through an injectable attempt seam. The WAV
/// is generated lazily only when Canberra fails, then reused for both PCM players.
fn try_chime_backends(
    severity: Severity,
    mut attempt: impl FnMut(ChimeBackend, Severity, Option<&[u8]>) -> bool,
) -> bool {
    let mut fallback_wav = None;
    for backend in CHIME_BACKENDS {
        if !matches!(backend, ChimeBackend::Canberra) && fallback_wav.is_none() {
            fallback_wav = Some(built_in_chime_wav(severity));
        }
        let wav = if matches!(backend, ChimeBackend::Canberra) {
            None
        } else {
            fallback_wav.as_deref()
        };
        if attempt(backend, severity, wav) {
            return true;
        }
    }
    false
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_chime_child(child: &mut Child) -> bool {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < CHIME_PLAYER_TIMEOUT => {
                thread::sleep(CHIME_PLAYER_POLL);
            }
            Ok(None) | Err(_) => {
                terminate_child(child);
                return false;
            }
        }
    }
}

fn run_chime_command(mut command: Command, wav: Option<&[u8]>) -> bool {
    command
        .stdin(if wav.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };

    let writer = if let Some(bytes) = wav {
        let Some(mut stdin) = child.stdin.take() else {
            terminate_child(&mut child);
            return false;
        };
        let bytes = bytes.to_vec();
        let Ok(writer) = thread::Builder::new()
            .name("mde-toast-chime-input".to_owned())
            .spawn(move || stdin.write_all(&bytes).is_ok())
        else {
            terminate_child(&mut child);
            return false;
        };
        Some(writer)
    } else {
        None
    };

    let child_ok = wait_for_chime_child(&mut child);
    let input_ok = writer.is_none_or(|writer| writer.join().is_ok_and(|ok| ok));
    child_ok && input_ok
}

/// Derive the Pulse compatibility socket from a trustworthy XDG runtime path.
/// Accept exactly `/run/user/<canonical u32>` (with harmless repeated/trailing
/// separators normalized by [`Path::components`]); reject traversal, names, and
/// alternate roots before this value reaches a subprocess environment.
fn pulse_server_from_runtime_dir(runtime_dir: &OsStr) -> Option<String> {
    let mut components = Path::new(runtime_dir).components();
    if components.next() != Some(Component::RootDir)
        || components.next() != Some(Component::Normal(OsStr::new("run")))
        || components.next() != Some(Component::Normal(OsStr::new("user")))
    {
        return None;
    }
    let Component::Normal(uid) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    let uid = uid.to_str()?;
    let numeric_uid = uid.parse::<u32>().ok()?;
    if numeric_uid.to_string() != uid {
        return None;
    }
    Some(format!("unix:/run/user/{numeric_uid}/pulse/native"))
}

fn run_chime_backend(backend: ChimeBackend, severity: Severity, wav: Option<&[u8]>) -> bool {
    match backend {
        ChimeBackend::Canberra => {
            let event = match severity {
                Severity::Critical | Severity::Warning => "dialog-warning",
                Severity::Info => "message-new-instant",
            };
            let mut command = Command::new("canberra-gtk-play");
            command.args(["-i", event]);
            run_chime_command(command, None)
        }
        ChimeBackend::PipeWire => {
            let Some(wav) = wav else {
                return false;
            };
            let mut command = Command::new("pw-play");
            command.arg("-");
            run_chime_command(command, Some(wav))
        }
        ChimeBackend::PulseAudio => {
            let Some(wav) = wav else {
                return false;
            };
            let mut command = Command::new("paplay");
            if let Some(server) = std::env::var_os("XDG_RUNTIME_DIR")
                .as_deref()
                .and_then(pulse_server_from_runtime_dir)
            {
                command.env("PULSE_SERVER", server);
            }
            run_chime_command(command, Some(wav))
        }
    }
}

impl Chime for SystemChime {
    fn ring(&self, severity: Severity) {
        let _ = thread::Builder::new()
            .name("mde-toast-chime".to_owned())
            .spawn(move || {
                let _ = try_chime_backends(severity, run_chime_backend);
            });
    }
}

/// What an alert action resolves to — Chat inline notification cards use this
/// single grammar when they execute a typed action verb.
pub(crate) enum Navigate {
    /// Switch the shell to this dock surface.
    Surface(Surface),
    /// Open one leaf owned by the unified Workers workspace.
    Workers(WorkersDestination),
    /// Open the Workbench on this plane.
    Plane(Plane),
    /// Open the centered System and Mesh Health modal.
    Health,
}

/// Resolve an opaque alert action `verb` to shell navigation. The verb grammar is
/// `shell/goto/<surface>` or `shell/plane/<plane>`; an unknown verb is a no-op
/// (`None`) — a forward-compatible emitter never breaks the shell.
///
/// `pub(crate)` so the Chat surface (NOTIFY-CHAT-4) reuses this ONE resolver to
/// decide whether a folded alert's inline action verb names a reachable target
/// before it offers the button — the shell has a single navigation grammar, not a
/// second copy in `chat.rs`.
pub(crate) fn resolve_action(verb: &str) -> Option<Navigate> {
    let rest = verb.strip_prefix("shell/")?;
    if let Some(name) = rest.strip_prefix("goto/") {
        // WL-ARCH-006 — the retired Cloud plane's `instances`/`cloud` deep-links
        // land on the unified Workloads surface (Infra as Code) now, so a forward
        // emitter's old cloud verb still reaches a live cloud surface.
        if matches!(name.to_ascii_lowercase().as_str(), "instances" | "cloud") {
            return Some(Navigate::Surface(Surface::InfraCode));
        }
        if name.eq_ignore_ascii_case("mesh-map") {
            return Some(Navigate::Workers(WorkersDestination::MeshMap));
        }
        if name.eq_ignore_ascii_case("discovery") {
            return Some(Navigate::Workers(WorkersDestination::Discovery));
        }
        if name.eq_ignore_ascii_case("health") {
            return Some(Navigate::Health);
        }
        return surface_by_name(name).map(Navigate::Surface);
    }
    if let Some(name) = rest.strip_prefix("plane/") {
        return plane_by_name(name).map(Navigate::Plane);
    }
    None
}

/// Map a `shell/goto/<name>` target to a dock [`Surface`] (case-insensitive).
fn surface_by_name(name: &str) -> Option<Surface> {
    match name.to_ascii_lowercase().as_str() {
        "workers" | "worker" | "control-panel" | "controlpanel" => Some(Surface::Workers),
        "fleet-mesh" | "fleetmesh" | "fleet" => Some(Surface::FleetMesh),
        "workbench" => Some(Surface::Workbench),
        "desktop" => Some(Surface::Desktop),
        // The Infra as Code (IaC) cloud control plane (IAC-2).
        "iac" | "infra-code" | "infracode" | "infra" => Some(Surface::InfraCode),
        "music" => Some(Surface::Music),
        "files" => Some(Surface::Files),
        "browser" => Some(Surface::Browser),
        // Car boots into the governed AutoHome dashboard; keep both the
        // operator-facing profile name and the concrete surface name usable by
        // proof/deep-link callers.
        "car" | "auto-home" | "autohome" => Some(Surface::AutoHome),
        "maps" | "location" | "maps-location" | "mapslocation" => Some(Surface::MapsLocation),
        // Notifications and clipboard are content modes in the unified hub. The
        // retired Chat / Voice / Editor product routes deliberately do not alias
        // here: stale collaboration publishers must fail closed instead of keeping
        // a second navigation contract reachable.
        "notifications" | "clipboard" => Some(Surface::Communications),
        "this-node" | "thisnode" | "node" => Some(Surface::ThisNode),
        "system" => Some(Surface::System),
        "storage" => Some(Surface::Storage),
        // The Timers & Alarms surface — the clock's replacement; the
        // `clock` alias keeps a "where did the clock go?" verb landing somewhere
        // honest (lock #5: the clock is now Timers & Alarms).
        "timers" | "alarms" | "clock" => Some(Surface::Clock),
        "media" | "video" => Some(Surface::Media),
        "terminal" | "term" => Some(Surface::Terminal),
        "phones" | "phone" => Some(Surface::Phones),
        "about" => Some(Surface::About),
        // The Communications hub (WL-FUNC-011) — an alert/chyron `shell/goto`
        // targeting it now resolves like every other dock surface.
        "collaboration" | "collab" | "communications" | "comms" => Some(Surface::Communications),
        _ => None,
    }
}

/// Map a `shell/plane/<name>` target to a Workbench [`Plane`] (case-insensitive).
fn plane_by_name(name: &str) -> Option<Plane> {
    match name.to_ascii_lowercase().as_str() {
        "thisnode" => Some(Plane::ThisNode),
        // WL-ARCH-006 — `cloud`/`controller` are no longer Workbench planes (the
        // mesh cloud is the standalone Workloads surface); a forward emitter's old
        // cloud verb reaches it through `shell/goto/cloud` instead (see
        // `resolve_action`), not a plane deep-link.
        "network" => Some(Plane::Network),
        "fleet" => Some(Plane::Fleet),
        "provisioning" => Some(Plane::Provisioning),
        _ => None,
    }
}

/// The shell's one [`ToastHost`] plus its Bus subscription, suppression posture,
/// and sound seam — the KIRON-2 bridge the shell drives once per frame.
pub(crate) struct ToastBridge {
    bus_root: Option<PathBuf>,
    /// Bus ULID cursor for `list_since` — advances on each drain.
    cursor: Option<String>,
    /// When the lane was last drained (drives [`REFRESH`]).
    last_poll: Option<Instant>,
    /// The previous frame instant — the injected `tick` delta is `now - this`.
    last_tick: Option<Instant>,
    /// The one host every surface paints into (lock 1).
    host: ToastHost,
    /// The live suppression posture (refreshed by the shell each frame).
    suppress: Suppress,
    /// The notification-sound seam (production spawns the event sound; tests
    /// record).
    chime: Box<dyn Chime>,
    /// test-obs-3 — latch so a persistent alert-lane read failure logs ONCE per
    /// error streak rather than every `REFRESH` tick (the poll cadence would else
    /// spam journald). Reset on the next successful read.
    read_error_logged: bool,
    /// WL-UX-006/U13 (PLATFORM-INTERFACES Q14) — the Notification Center's
    /// grouped-history ring: every alert this bridge decodes/raises is recorded
    /// here, INCLUDING ones DND/focus-mute hid from the ambient push
    /// (suppression governs the push + sound, never the history — the iOS
    /// Notification Center semantic). A pure presentation data tap; the
    /// queue/sound policy in [`Self::admit`] is untouched.
    history: NotificationRing,
    /// Alerts admitted since Notification Center last exposed its visible
    /// rows. This is process-local presentation state, bounded by the retained
    /// ring; it is not a second acknowledgement or persistence authority.
    unread: usize,
    /// Current daemon-projected Clock banners.  They preempt generic chyrons but
    /// do not own schedules, deadlines, or command publication.
    clock_banners: Vec<ClockBannerProjection>,
    /// Bounded deduplication of occurrence/schedule generations already folded
    /// into history and sounded by this shell process.
    seen_clock_banners: VecDeque<ClockBannerProjection>,
}

impl Default for ToastBridge {
    fn default() -> Self {
        let bus_root = mde_bus::client_data_dir();
        Self {
            cursor: initial_toast_cursor(bus_root.as_deref()),
            bus_root,
            last_poll: None,
            last_tick: None,
            host: ToastHost::new(),
            suppress: Suppress::default(),
            chime: Box::new(SystemChime),
            read_error_logged: false,
            history: NotificationRing::default(),
            unread: 0,
            clock_banners: Vec::new(),
            seen_clock_banners: VecDeque::new(),
        }
    }
}

/// Seed a restarting shell at the current alert-lane tail.
///
/// The toast topic is a notification transport, not a durable unresolved-alert
/// ledger. Replaying its full retained history on every shell restart resurrects
/// already-resolved health incidents and expired deployment countdowns. The Bus
/// index exposes a cheap tail probe specifically for restartable consumers; new
/// messages published after this cursor are still drained normally.
fn initial_toast_cursor(bus_root: Option<&Path>) -> Option<String> {
    let root = bus_root?;
    Persist::open(root.to_path_buf())
        .ok()?
        .latest_ulid(TOAST_TOPIC)
        .ok()?
}

impl ToastBridge {
    /// Refresh the suppression posture (lock 10) — the shell folds its live DND
    /// toggle, the per-session focus mute (a fullscreen guest is in front), and the
    /// seat's audio-mute in each frame before [`drive`](Self::drive).
    pub(crate) const fn set_suppression(&mut self, dnd: bool, focus_mute: bool, muted: bool) {
        self.suppress = Suppress {
            dnd,
            focus_mute,
            muted,
        };
    }

    /// Raise a locally-generated alert directly, applying the SAME suppression +
    /// single-sound policy (locks 8/10) as a Bus-borne alert. The one local-raise
    /// seam so a surface (e.g. the System panel's refused-Bluetooth-write error)
    /// never opens a second toast channel — it hands its [`Toast`] here so the one
    /// notification sound/suppression policy handles it. Chat is the visual alert
    /// home; this bridge does not mount notification popups.
    pub(crate) fn raise(&mut self, toast: Toast) {
        self.admit(toast);
    }

    /// Flash the centered OSD pill (volume / brightness), replacing any
    /// current one in place. This is the emitter KIRON-2 left waiting on the OSD tier
    /// (KIRON-3): the seat's volume/brightness hotkeys (E12-19) call it directly —
    /// the OSD is an instant hardware-feedback channel, never the Bus alert lane
    /// (lock 7). Because it is a direct in-shell call, DND / focus-mute suppression
    /// never applies (that governs *alert* chyrons, not a level flash).
    pub(crate) fn flash_osd(&mut self, level: OsdLevel) {
        self.host.flash_osd(level);
    }

    /// The per-frame drive: advance the countdowns by the real frame delta, drain
    /// any new `event/toast/show`, then paint the alert banner and OSD tiers.
    /// Alerts remain folded into Notification history as well as being surfaced;
    /// suppression decides whether an ambient banner is admitted.
    pub(crate) fn drive(&mut self, ctx: &egui::Context) -> Option<Navigate> {
        self.tick(ctx);
        self.drain();
        self.sync_clock_banners(ctx);
        if notification_center_visible(ctx) {
            self.unread = 0;
        }
        publish_unread(ctx, self.unread);
        let action = if self.clock_banners.is_empty() {
            self.host.chyron(ctx).action
        } else {
            paint_clock_banners(ctx, &self.clock_banners);
            None
        };
        // The centered OSD tier is a separate, instant channel; painting it here
        // keeps hardware feedback live without notification clutter.
        self.host.osd(ctx);
        action.as_deref().and_then(resolve_action)
    }

    fn sync_clock_banners(&mut self, ctx: &egui::Context) {
        let mut projected = clock_banner_projection(ctx);
        let now_ms = crate::timers::now_unix().saturating_mul(1_000);

        // Preserve the original bounded action deadline while the same daemon
        // identity remains live; periodic projections must not keep extending a
        // retained row's authority forever.
        for banner in &mut projected {
            if let Some(existing) = self
                .seen_clock_banners
                .iter()
                .find(|existing| existing.identity == banner.identity)
            {
                banner.actions = existing.actions.clone();
            }
        }
        self.history.refresh_clock_actionability(&projected, now_ms);

        for banner in &projected {
            if self
                .seen_clock_banners
                .iter()
                .any(|existing| existing.identity == banner.identity)
            {
                continue;
            }
            self.seen_clock_banners.push_back(banner.clone());
            while self.seen_clock_banners.len() > crate::notification_center::RING_CAP {
                self.seen_clock_banners.pop_front();
            }
            self.history.record_clock(banner, crate::timers::now_unix());
            self.unread = self.unread.saturating_add(1).min(self.history.len());
            let severity = clock_banner_severity(banner);
            if !self.suppress.hushes_sound(severity) {
                self.chime.ring(severity);
            }
        }

        projected.retain(|banner| {
            banner
                .actions
                .iter()
                .all(|action| now_ms <= action.valid_until_utc_ms)
                && !self
                    .suppress
                    .hides_ambient_push(clock_banner_severity(banner))
        });
        self.clock_banners = projected;
        if !self.clock_banners.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Advance the host's countdowns by the elapsed frame delta and keep the
    /// repaint heartbeat alive while anything is showing (the dwell must tick down
    /// even with no other input).
    fn tick(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
        self.last_tick = Some(now);
        self.host.tick(dt);
        if !self.host.is_idle() {
            ctx.request_repaint();
        }
    }

    /// Drain the alert lane on the [`REFRESH`] cadence: read new messages after the
    /// cursor, decode each, and admit it (suppression + enqueue + sound).
    fn drain(&mut self) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if !due {
            return;
        }
        self.last_poll = Some(Instant::now());
        // No configured bus root is the honest "no bus on this seat" case (§7),
        // not an error — stay silent.
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        // A set-but-unopenable spool, or a failed read, means the alert lane is
        // DOWN: the operator's Critical alerts silently won't arrive. Surface it
        // (once per streak — this runs every REFRESH) instead of swallowing it.
        // arch-11: NOT the fail-soft BusReader seam — this reader needs the open
        // error to log the down alert-lane, so it keeps its own Persist::open.
        let persist = match Persist::open(root.clone()) {
            Ok(p) => p,
            Err(e) => {
                self.log_read_error("open the alert-lane spool", &root, &e);
                return;
            }
        };
        let msgs = match persist.list_since(TOAST_TOPIC, self.cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                self.log_read_error("read the alert lane", &root, &e);
                return;
            }
        };
        // A clean read clears the latch so the next failure logs afresh.
        self.read_error_logged = false;
        for msg in msgs {
            self.cursor = Some(msg.ulid.clone());
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            if let Some(toast) = decode(body) {
                self.admit(toast);
            }
        }
    }

    /// test-obs-3 — log an alert-lane read failure ONCE per contiguous error
    /// streak (the drain runs every `REFRESH`, so an unconditional log would spam
    /// journald). The latch is cleared by the next successful read in
    /// [`drain`](Self::drain). `error` level because a down alert lane means the
    /// operator's Critical notifications silently won't arrive.
    fn log_read_error(&mut self, op: &str, root: &std::path::Path, err: &impl std::fmt::Display) {
        if self.read_error_logged {
            return;
        }
        self.read_error_logged = true;
        tracing::error!(
            target: "shell::toast",
            bus_root = %root.display(),
            error = %err,
            "could not {op}; alert lane down — Critical alerts may be dropped",
        );
    }

    /// Apply suppression (lock 10), enqueue the visible banner, then ring (lock 8).
    /// A suppressed Info/Warning is retained in history but neither shown nor
    /// rung; a Critical always breaks through. Split from the Bus read so the
    /// whole policy is unit-tested without a spool.
    fn admit(&mut self, toast: Toast) {
        let Some(severity) = alert_severity(&toast) else {
            // An OSD-tier toast on the alert lane would just flash — but the lane
            // only ever carries alerts; route it through the host regardless.
            self.host.enqueue(toast);
            return;
        };
        // U13 (Q14) — record BEFORE the suppression fold: DND hides the push,
        // never the history (the Notification Center is where a muted alert is
        // found later). Pure data tap; nothing below changes.
        self.history
            .record(severity, &toast.source_host, &toast.flag, &toast.headline);
        self.unread = self.unread.saturating_add(1).min(self.history.len());
        // Deployment notices tagged AI-GENERATED-ALERT are an explicit operator
        // safety channel: they remain visible through DND/focused-VDI posture so
        // a seat is warned before its shell or services are updated. Audio still
        // follows the normal mute policy.
        if !toast.is_ai_generated_alert() && self.suppress.hides_ambient_push(severity) {
            return;
        }
        self.host.enqueue(toast);
        if !self.suppress.hushes_sound(severity) {
            self.chime.ring(severity);
        }
    }

    /// U13 — the Notification Center's retained alert history (read side).
    pub(crate) const fn history(&self) -> &NotificationRing {
        &self.history
    }

    /// U13 — the Notification Center's clear-all / per-group clear seam. Clears
    /// only this shell-local ring — no new ack semantics ride the Bus.
    pub(crate) const fn history_mut(&mut self) -> &mut NotificationRing {
        &mut self.history
    }

    /// Mark the rows currently exposed by Notification Center as read.
    pub(crate) fn mark_visible_read(&mut self, ctx: &egui::Context) {
        self.unread = 0;
        publish_unread(ctx, 0);
    }

    #[cfg(test)]
    pub(crate) const fn unread(&self) -> usize {
        self.unread
    }
}

const CLOCK_BANNER_VISIBLE_CAP: usize = 4;

const fn clock_banner_severity(banner: &ClockBannerProjection) -> Severity {
    match banner.actions[0].kind {
        ClockBannerKind::Alarm => Severity::Critical,
        ClockBannerKind::Timer => Severity::Warning,
    }
}

fn paint_clock_banners(ctx: &egui::Context, banners: &[ClockBannerProjection]) {
    egui::Area::new(egui::Id::new("shell-clock-action-banners"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, Style::SP_M))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.vertical(|ui| {
                for banner in banners.iter().take(CLOCK_BANNER_VISIBLE_CAP) {
                    egui::Frame::popup(ui.style())
                        .fill(Style::SURFACE)
                        .stroke(Style::hairline())
                        .corner_radius(Style::RADIUS_M)
                        .inner_margin(Style::SP_M)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&banner.headline)
                                    .color(Style::TEXT_STRONG)
                                    .strong(),
                            );
                            ui.horizontal(|ui| {
                                for action in &banner.actions {
                                    if ui.button(action.label).clicked() {
                                        request_clock_banner_action(ui.ctx(), action.clone());
                                    }
                                }
                            });
                        });
                }
                if banners.len() > CLOCK_BANNER_VISIBLE_CAP {
                    ui.label(format!(
                        "+{} more Clock alerts in Notification Center",
                        banners.len() - CLOCK_BANNER_VISIBLE_CAP
                    ));
                }
            });
        });
}

fn unread_context_id() -> egui::Id {
    egui::Id::new("construct-notification-unread")
}

fn notification_center_visible_id() -> egui::Id {
    egui::Id::new("construct-notification-center-visible")
}

pub(crate) fn set_notification_center_visible(ctx: &egui::Context, visible: bool) {
    ctx.data_mut(|data| data.insert_temp(notification_center_visible_id(), visible));
}

fn notification_center_visible(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp(notification_center_visible_id())
            .unwrap_or(false)
    })
}

fn publish_unread(ctx: &egui::Context, unread: usize) {
    ctx.data_mut(|data| data.insert_temp(unread_context_id(), unread));
}

/// Read-only chrome projection of the bridge's bounded unread count.
pub(crate) fn unread_count(ctx: &egui::Context) -> usize {
    ctx.data(|data| data.get_temp(unread_context_id()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::workers_catalog::WorkersDestination;
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::{Style, Tier, Toast, ToastHost};

    use super::{
        alert_severity, built_in_chime_wav, decode, initial_toast_cursor, plane_by_name,
        pulse_server_from_runtime_dir, resolve_action, surface_by_name, try_chime_backends,
        unread_count, Chime, ChimeBackend, Navigate, Severity, Suppress, ToastBridge, TOAST_TOPIC,
    };
    use crate::surfaces::Surface;
    use crate::workbench::Plane;

    /// A recording chime — counts each ring so a test can assert "fires once /
    /// suppressed" without a sound backend.
    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<Severity>>>);

    impl Chime for Recorder {
        fn ring(&self, severity: Severity) {
            self.0.borrow_mut().push(severity);
        }
    }

    /// A bridge with no Bus (so `drain` is inert) and a recording chime.
    fn bridge_with(rec: &Recorder) -> ToastBridge {
        ToastBridge {
            bus_root: None,
            chime: Box::new(rec.clone()),
            ..ToastBridge::default()
        }
    }

    fn body(severity: &str, host: &str, headline: &str) -> String {
        format!(
            r#"{{"severity":"{severity}","source_host":"{host}","flag":"SECURITY","headline":"{headline}"}}"#
        )
    }

    #[test]
    fn notification_unread_is_bounded_and_marked_read_only_when_exposed() {
        let recorder = Recorder::default();
        let mut bridge = bridge_with(&recorder);
        bridge.raise(Toast::alert(
            Severity::Warning,
            "seat-9",
            "CLOCK",
            "Timer complete",
        ));
        assert_eq!(bridge.unread(), 1);
        assert_eq!(bridge.history().len(), 1);

        let ctx = egui::Context::default();
        bridge.mark_visible_read(&ctx);
        assert_eq!(bridge.unread(), 0);
        assert_eq!(unread_count(&ctx), 0);
        assert_eq!(
            bridge.history().len(),
            1,
            "mark-read must retain the notification row"
        );
    }

    #[test]
    fn clock_projection_is_folded_once_and_stale_rows_lose_actionability() {
        use crate::notification_center::grouped;
        use crate::timers::{
            ClockBannerAction, ClockBannerKind, ClockBannerProjection, ClockBannerVerb,
        };

        let now_ms = crate::timers::now_unix().saturating_mul(1_000);
        let action = |label, verb| ClockBannerAction {
            label,
            verb,
            kind: ClockBannerKind::Alarm,
            occurrence_id: "occurrence-1".into(),
            occurrence_revision: 5,
            schedule_id: "alarm-1".into(),
            schedule_revision: 6,
            admitted_snapshot_revision: 7,
            valid_until_utc_ms: now_ms + 60_000,
        };
        let banner = ClockBannerProjection {
            headline: "Alarm · Wake up".into(),
            identity: "occurrence-1:5:alarm-1:6".into(),
            actions: [
                action("Snooze", ClockBannerVerb::Snooze),
                action("Stop", ClockBannerVerb::Stop),
            ],
        };
        let ctx = egui::Context::default();
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("shell-clock-banner-projection"), vec![banner]);
        });
        let recorder = Recorder::default();
        let mut bridge = bridge_with(&recorder);
        bridge.sync_clock_banners(&ctx);
        bridge.sync_clock_banners(&ctx);
        assert_eq!(bridge.history().len(), 1, "projection deduplicated");
        assert_eq!(recorder.0.borrow().len(), 1, "one Clock chime");
        assert!(
            grouped(bridge.history())[0].entries[0]
                .clock_actions
                .as_ref()
                .expect("retained Clock metadata")
                .actionable
        );

        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("shell-clock-banner-projection"),
                Vec::<ClockBannerProjection>::new(),
            );
        });
        bridge.sync_clock_banners(&ctx);
        let retained = grouped(bridge.history())[0].entries[0]
            .clock_actions
            .as_ref()
            .expect("metadata remains retained");
        assert!(!retained.actionable, "stale row must fail closed");
    }

    #[test]
    fn restart_cursor_skips_retained_alerts_but_drains_new_ones() {
        let dir = tempfile::tempdir().expect("temp bus");
        let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
        let old = persist
            .write(
                TOAST_TOPIC,
                Priority::Urgent,
                None,
                Some(&body("critical", "old-node", "resolved incident")),
            )
            .expect("write retained alert");
        let cursor = initial_toast_cursor(Some(dir.path()));
        assert_eq!(cursor.as_deref(), Some(old.ulid.as_str()));

        let recorder = Recorder::default();
        let mut bridge = ToastBridge {
            bus_root: Some(dir.path().to_path_buf()),
            cursor,
            chime: Box::new(recorder.clone()),
            ..ToastBridge::default()
        };
        persist
            .write(
                TOAST_TOPIC,
                Priority::Default,
                None,
                Some(&body("info", "new-node", "new incident")),
            )
            .expect("write new alert");

        bridge.drain();
        assert_eq!(
            bridge.history().len(),
            1,
            "only the post-start alert is read"
        );
        assert_eq!(*recorder.0.borrow(), vec![Severity::Info]);
    }

    fn wav_u16(wav: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([wav[offset], wav[offset + 1]])
    }

    fn wav_u32(wav: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            wav[offset],
            wav[offset + 1],
            wav[offset + 2],
            wav[offset + 3],
        ])
    }

    fn wav_peak(wav: &[u8]) -> i32 {
        wav[44..]
            .chunks_exact(2)
            .map(|sample| i32::from(i16::from_le_bytes([sample[0], sample[1]])).abs())
            .max()
            .unwrap_or(0)
    }

    // ── production chime fallback ────────────────────────────────────────────

    #[test]
    fn built_in_chimes_are_deterministic_valid_mono_pcm_wav() {
        for severity in [Severity::Info, Severity::Warning, Severity::Critical] {
            let wav = built_in_chime_wav(severity);
            assert_eq!(wav, built_in_chime_wav(severity));
            assert_eq!(&wav[0..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            assert_eq!(&wav[12..16], b"fmt ");
            assert_eq!(wav_u16(&wav, 20), 1, "fallback is linear PCM");
            assert_eq!(wav_u16(&wav, 22), 1, "fallback is mono");
            assert_eq!(wav_u32(&wav, 24), 24_000);
            assert_eq!(wav_u16(&wav, 34), 16);
            assert_eq!(&wav[36..40], b"data");
            assert_eq!(wav.len(), 44 + wav_u32(&wav, 40) as usize);
            assert!(wav_peak(&wav) > 1_000, "fallback signal was silent");
        }
    }

    #[test]
    fn built_in_chime_duration_and_peak_scale_with_severity() {
        let info = built_in_chime_wav(Severity::Info);
        let warning = built_in_chime_wav(Severity::Warning);
        let critical = built_in_chime_wav(Severity::Critical);

        assert!(info.len() < warning.len() && warning.len() < critical.len());
        assert!(
            wav_peak(&info) < wav_peak(&warning) && wav_peak(&warning) < wav_peak(&critical),
            "higher severities should be more audible without clipping"
        );
        assert!(wav_peak(&critical) < i32::from(i16::MAX));
    }

    #[test]
    fn chime_fallback_tries_canberra_then_pipewire_and_stops_on_success() {
        let mut attempts = Vec::new();
        let played = try_chime_backends(Severity::Warning, |backend, severity, wav| {
            attempts.push(backend);
            assert_eq!(severity, Severity::Warning);
            match backend {
                ChimeBackend::Canberra => {
                    assert!(wav.is_none(), "Canberra should use the sound theme");
                    false
                }
                ChimeBackend::PipeWire => {
                    assert!(wav.is_some_and(|bytes| bytes.starts_with(b"RIFF")));
                    true
                }
                ChimeBackend::PulseAudio => false,
            }
        });

        assert!(played);
        assert_eq!(
            attempts,
            vec![ChimeBackend::Canberra, ChimeBackend::PipeWire]
        );
    }

    #[test]
    fn chime_fallback_reaches_paplay_and_reuses_the_same_wav() {
        let mut attempts = Vec::new();
        let mut payload_fingerprints = Vec::new();
        let played = try_chime_backends(Severity::Critical, |backend, _, wav| {
            attempts.push(backend);
            payload_fingerprints.push(wav.map(|bytes| {
                (
                    bytes.len(),
                    bytes
                        .iter()
                        .fold(0_u64, |sum, byte| sum.wrapping_add(u64::from(*byte))),
                )
            }));
            matches!(backend, ChimeBackend::PulseAudio)
        });

        assert!(played);
        assert_eq!(attempts, super::CHIME_BACKENDS);
        assert_eq!(payload_fingerprints[0], None);
        assert_eq!(payload_fingerprints[1], payload_fingerprints[2]);
    }

    #[test]
    fn pulse_server_derivation_accepts_only_safe_numeric_runtime_dirs() {
        use std::ffi::OsStr;

        assert_eq!(
            pulse_server_from_runtime_dir(OsStr::new("/run/user/1000")),
            Some("unix:/run/user/1000/pulse/native".to_owned())
        );
        assert_eq!(
            pulse_server_from_runtime_dir(OsStr::new("/run//user/0/")),
            Some("unix:/run/user/0/pulse/native".to_owned())
        );

        for rejected in [
            "run/user/1000",
            "/tmp/run/user/1000",
            "/run/user/root",
            "/run/user/01000",
            "/run/user/4294967296",
            "/run/user/1000/pulse",
            "/run/user/1000/../0",
            "unix:/run/user/1000",
        ] {
            assert_eq!(
                pulse_server_from_runtime_dir(OsStr::new(rejected)),
                None,
                "accepted unsafe XDG_RUNTIME_DIR {rejected:?}"
            );
        }
    }

    // ── decode (the wire boundary) ────────────────────────────────────────────

    #[test]
    fn decode_folds_a_wire_body_into_an_alert_toast() {
        let toast = decode(&body("warning", "nyc3", "disk 90%")).expect("decodes");
        assert_eq!(alert_severity(&toast), Some(Severity::Warning));
        assert_eq!(toast.source_host, "nyc3");
        assert_eq!(toast.flag, "SECURITY");
        assert_eq!(toast.headline, "disk 90%");
        assert!(toast.action.is_none());
    }

    #[test]
    fn decode_carries_an_optional_action_when_both_fields_present() {
        let raw = r#"{"severity":"info","source_host":"lh1","flag":"CHAT","headline":"new message","action_label":"Open","action_verb":"shell/goto/chat"}"#;
        let toast = decode(raw).expect("decodes");
        let action = toast.action.expect("action set");
        assert_eq!(action.label, "Open");
        assert_eq!(action.verb, "shell/goto/chat");
    }

    fn health_kiron_body(grade: mackes_mesh_types::health::GradeLetter) -> String {
        let now_ms = super::current_unix_ms();
        health_kiron_body_at(grade, now_ms.saturating_sub(70_000), now_ms)
    }

    fn health_kiron_body_at(
        grade: mackes_mesh_types::health::GradeLetter,
        active_since_ms: u64,
        observed_at_ms: u64,
    ) -> String {
        let alert = mackes_mesh_types::health::HealthKironAlert {
            kind: mackes_mesh_types::health::HealthKironKind::HealthKiron,
            schema_version: mackes_mesh_types::health::HEALTH_KIRON_SCHEMA_VERSION,
            snapshot_generation: 42,
            condition_id: "disk-pressure".into(),
            node: "node-9".into(),
            device: Some("nvme0n1".into()),
            grade,
            headline: "Storage pressure remains active".into(),
            active_since_ms,
            observed_at_ms,
        };
        serde_json::to_string(&alert).expect("serialize shared health contract")
    }

    #[test]
    fn shared_health_contract_maps_into_toast_host_without_regrading() {
        use mackes_mesh_types::health::GradeLetter;

        let grade_a = decode(&health_kiron_body(GradeLetter::A)).expect("grade A admitted");
        assert_eq!(grade_a.tier, Tier::Alert(Severity::Info));
        assert_eq!(
            grade_a.dwell,
            mde_egui::Dwell::For(std::time::Duration::from_secs(3))
        );

        let grade_b = decode(&health_kiron_body(GradeLetter::B)).expect("grade B admitted");
        assert_eq!(grade_b.tier, Tier::Alert(Severity::Info));
        assert_eq!(
            grade_b.dwell,
            mde_egui::Dwell::For(std::time::Duration::from_secs(5))
        );

        let grade_c = decode(&health_kiron_body(GradeLetter::C)).expect("grade C admitted");
        assert_eq!(grade_c.tier, Tier::Alert(Severity::Warning));
        assert_eq!(
            grade_c.dwell,
            mde_egui::Dwell::For(std::time::Duration::from_secs(6))
        );

        let grade_d = decode(&health_kiron_body(GradeLetter::D)).expect("grade D admitted");
        assert_eq!(grade_d.tier, Tier::Alert(Severity::Warning));
        assert_eq!(
            grade_d.dwell,
            mde_egui::Dwell::For(std::time::Duration::from_secs(10))
        );
        assert_eq!(grade_d.source_host, "node-9");
        assert_eq!(grade_d.flag, "HEALTH · GRADE D · 1m 10s · nvme0n1");
        assert_eq!(
            grade_d.action.as_ref().map(|action| action.verb.as_str()),
            Some("shell/goto/health")
        );
        assert_eq!(
            grade_d.action.as_ref().map(|action| action.label.as_str()),
            Some("Open Health")
        );

        let grade_e = decode(&health_kiron_body(GradeLetter::E)).expect("grade E admitted");
        assert_eq!(grade_e.tier, Tier::Alert(Severity::Critical));
        assert_eq!(
            grade_e.dwell,
            mde_egui::Dwell::For(std::time::Duration::from_secs(15))
        );
        assert_eq!(grade_e.flag, "HEALTH · GRADE E · 1m 10s · nvme0n1");

        let grade_f = decode(&health_kiron_body(GradeLetter::F)).expect("grade F admitted");
        assert_eq!(grade_f.tier, Tier::Alert(Severity::Critical));
        assert_eq!(grade_f.dwell, mde_egui::Dwell::UntilAck);
    }

    #[test]
    fn governed_health_generation_survives_decode_and_blocks_grade_f_rollback() {
        use mackes_mesh_types::health::GradeLetter;

        let current_f = decode(&health_kiron_body(GradeLetter::F)).expect("grade F admitted");

        let mut stale_recovery: serde_json::Value =
            serde_json::from_str(&health_kiron_body(GradeLetter::A))
                .expect("valid stale recovery body");
        stale_recovery["snapshot_generation"] = serde_json::json!(41);
        stale_recovery["headline"] = serde_json::json!("Host recovered in stale snapshot");
        let stale_recovery =
            decode(&stale_recovery.to_string()).expect("stale body is well-formed");

        let mut conflicting_replay: serde_json::Value =
            serde_json::from_str(&health_kiron_body(GradeLetter::E))
                .expect("valid conflicting replay body");
        conflicting_replay["headline"] =
            serde_json::json!("Same generation claims a different grade");
        let conflicting_replay =
            decode(&conflicting_replay.to_string()).expect("conflicting body is well-formed");

        let mut host = ToastHost::new();
        host.enqueue(current_f);
        host.enqueue(stale_recovery);
        host.enqueue(conflicting_replay);

        let current = host.current().expect("current grade F must remain visible");
        assert_eq!(current.flag, "HEALTH · GRADE F · 1m 10s · nvme0n1");
        assert_eq!(current.headline, "Storage pressure remains active");
        assert_eq!(current.dwell, mde_egui::Dwell::UntilAck);
        assert_eq!(host.backlog(), 0, "rollback projections must not queue");
    }

    #[test]
    fn typed_health_marker_fails_closed_for_unknown_grade() {
        let unsupported = health_kiron_body(mackes_mesh_types::health::GradeLetter::E)
            .replace(r#""grade":"E""#, r#""grade":"G""#);
        assert!(
            decode(&unsupported).is_none(),
            "typed health must not fall back to generic severity decoding"
        );
    }

    #[test]
    fn typed_health_marker_rejects_future_dated_lower_thirds() {
        let now_ms = 1_000_000;
        let mut value: serde_json::Value = serde_json::from_str(&health_kiron_body_at(
            mackes_mesh_types::health::GradeLetter::D,
            now_ms - 10_000,
            now_ms - 1_000,
        ))
        .expect("valid health body");
        value["observed_at_ms"] = serde_json::json!(now_ms + 1);
        assert!(
            super::decode_at(&value.to_string(), now_ms).is_none(),
            "a future observation must not reach the cinematic lower third"
        );

        value["observed_at_ms"] = serde_json::json!(now_ms);
        value["active_since_ms"] = serde_json::json!(now_ms + 1);
        assert!(
            super::decode_at(&value.to_string(), now_ms).is_none(),
            "a future lifecycle start must not reach the cinematic lower third"
        );
    }

    #[test]
    fn decode_rejects_a_malformed_body() {
        assert!(decode("not json").is_none());
        // A partial action (label without verb) drops the action, not the toast.
        let raw = r#"{"severity":"info","headline":"hi","action_label":"Open"}"#;
        let toast = decode(raw).expect("still a valid toast");
        assert!(toast.action.is_none());
    }

    // ── suppression policy (lock 10) ──────────────────────────────────────────

    #[test]
    fn dnd_suppresses_info_and_warning_but_a_critical_breaks_through() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(true, false, false);

        b.admit(decode(&body("info", "a", "fyi")).unwrap());
        b.admit(decode(&body("warning", "b", "careful")).unwrap());
        // Nothing shown, nothing rang.
        assert!(b.host.is_idle());
        assert!(rec.0.borrow().is_empty());

        b.admit(decode(&body("critical", "lh1", "intrusion")).unwrap());
        assert!(b.host.has_critical(), "Critical opened a visible alert");
        assert_eq!(*rec.0.borrow(), vec![Severity::Critical], "and still rings");
    }

    #[test]
    fn focus_mute_suppresses_like_dnd() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(false, true, false);
        b.admit(decode(&body("info", "a", "fyi")).unwrap());
        assert!(b.host.is_idle());
        assert!(rec.0.borrow().is_empty());
    }

    #[test]
    fn audio_mute_hushes_the_sound_but_keeps_the_visible_alert() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(false, false, true);
        b.admit(decode(&body("warning", "a", "build failed")).unwrap());
        assert!(
            !b.host.is_idle(),
            "audio mute must not hide the visual alert"
        );
        assert!(rec.0.borrow().is_empty(), "but no sound fired");
    }

    #[test]
    fn every_alert_lands_in_the_notification_history_even_under_dnd() {
        // U13 (Q14): suppression hides the ambient push + sound, never the
        // history — a DND'd alert is found in the Notification Center later.
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(true, false, false);
        b.admit(decode(&body("info", "a", "fyi")).unwrap());
        b.admit(decode(&body("critical", "lh1", "intrusion")).unwrap());
        assert_eq!(b.history().len(), 2, "both alerts retained");
        assert!(rec.0.borrow().len() <= 1, "suppression policy unchanged");
        b.history_mut().clear();
        assert!(b.history().is_empty(), "clear-all empties the shell ring");
    }

    #[test]
    fn a_plain_alert_rings_exactly_once_and_opens_one_popup() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.admit(decode(&body("info", "nyc3", "hi")).unwrap());
        assert!(!b.host.is_idle(), "the shell bridge enqueued the popup");
        assert_eq!(*rec.0.borrow(), vec![Severity::Info], "one beep, no double");
    }

    #[test]
    fn an_ai_generated_alert_breaks_through_dnd_visually() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(true, true, true);
        b.admit(
            decode(
                r#"{"severity":"warning","source_host":"controller","flag":"AI-GENERATED-ALERT","headline":"Update begins in 5 seconds"}"#,
            )
            .unwrap(),
        );
        assert!(!b.host.is_idle(), "deployment warning was hidden by DND");
        assert!(
            rec.0.borrow().is_empty(),
            "mute posture still hushes its sound"
        );
    }

    // ── action verb resolution (KIRON-2 executes it) ──────────────────────────

    #[test]
    fn resolve_action_maps_goto_and_plane_verbs() {
        // WL-FUNC-011 — retired product routes are not aliases for the native
        // Collaboration surface. Keeping them reachable would preserve a legacy
        // publisher contract indefinitely.
        for retired in ["chat", "voice", "editor", "code", "teams", "mesh-teams"] {
            assert!(
                resolve_action(&format!("shell/goto/{retired}")).is_none(),
                "retired collaboration route {retired:?} remained reachable"
            );
        }
        assert!(matches!(
            resolve_action("shell/goto/collaboration"),
            Some(Navigate::Surface(Surface::Communications))
        ));
        // Content-mode aliases continue to reach their native Collaboration modes.
        assert!(matches!(
            resolve_action("shell/goto/notifications"),
            Some(Navigate::Surface(Surface::Communications))
        ));
        assert!(matches!(
            resolve_action("shell/goto/clipboard"),
            Some(Navigate::Surface(Surface::Communications))
        ));
        assert!(matches!(
            resolve_action("shell/goto/browser"),
            Some(Navigate::Surface(Surface::Browser))
        ));
        assert!(resolve_action("shell/goto/bookmarks").is_none());
        // WL-ARCH-006 — the retired Cloud plane's `instances`/`cloud` verbs now
        // land on the unified Workloads surface (Infra as Code).
        assert!(matches!(
            resolve_action("shell/goto/instances"),
            Some(Navigate::Surface(Surface::InfraCode))
        ));
        assert!(matches!(
            resolve_action("shell/goto/cloud"),
            Some(Navigate::Surface(Surface::InfraCode))
        ));
        assert!(matches!(
            resolve_action("shell/plane/fleet"),
            Some(Navigate::Plane(Plane::Fleet))
        ));
        assert!(matches!(
            resolve_action("shell/goto/mesh-map"),
            Some(Navigate::Workers(WorkersDestination::MeshMap))
        ));
        assert!(matches!(
            resolve_action("shell/goto/health"),
            Some(Navigate::Health)
        ));
        for retired in ["meshview", "mesh"] {
            assert!(
                resolve_action(&format!("shell/goto/{retired}")).is_none(),
                "retired Mesh View alias {retired:?} bypassed Control Panel routing"
            );
        }
        // Unknown verbs are a no-op, not a panic.
        assert!(resolve_action("shell/goto/nope").is_none());
        assert!(resolve_action("chat/open/peer").is_none());
        assert!(resolve_action("").is_none());
    }

    #[test]
    fn discovery_route_uses_workers_and_retired_explorer_aliases_fail_closed() {
        assert!(matches!(
            resolve_action("shell/goto/discovery"),
            Some(Navigate::Workers(WorkersDestination::Discovery))
        ));
        for retired in ["explorer", "Explorer", "fleet-explorer"] {
            assert!(
                resolve_action(&format!("shell/goto/{retired}")).is_none(),
                "retired Explorer route {retired:?} bypassed Control Panel authority"
            );
        }
    }

    #[test]
    fn name_maps_are_case_insensitive() {
        assert_eq!(surface_by_name("SYSTEM"), Some(Surface::System));
        assert_eq!(surface_by_name("This-Node"), Some(Surface::ThisNode));
        assert_eq!(
            surface_by_name("Collaboration"),
            Some(Surface::Communications)
        );
        assert_eq!(surface_by_name("car"), Some(Surface::AutoHome));
        assert_eq!(surface_by_name("AUTO-HOME"), Some(Surface::AutoHome));
        assert_eq!(surface_by_name("CONTROL-PANEL"), Some(Surface::Workers));
        assert_eq!(surface_by_name("controlpanel"), Some(Surface::Workers));
        assert_eq!(plane_by_name("ThisNode"), Some(Plane::ThisNode));
    }

    #[test]
    fn reach1_every_surface_has_a_goto_verb() {
        // REACH-1 — the six surfaces that used to silently no-op a shell/goto now resolve.
        assert_eq!(surface_by_name("workers"), Some(Surface::Workers));
        assert_eq!(surface_by_name("explorer"), None);
        assert_eq!(surface_by_name("media"), Some(Surface::Media));
        assert_eq!(surface_by_name("terminal"), Some(Surface::Terminal));
        assert_eq!(surface_by_name("phones"), Some(Surface::Phones));
        assert_eq!(surface_by_name("about"), Some(Surface::About));
        assert_eq!(surface_by_name("this-node"), Some(Surface::ThisNode));
        assert_eq!(
            surface_by_name("collaboration"),
            Some(Surface::Communications)
        );
        // Retired sibling surfaces are intentionally absent from the external
        // grammar; the remaining surfaces resolve from their lowercased name.
        for s in Surface::ALL
            .into_iter()
            .filter(|surface| *surface != Surface::Explorer)
        {
            let verb = format!("{s:?}").to_ascii_lowercase();
            assert_eq!(
                surface_by_name(&verb),
                Some(s),
                "surface {s:?} has no shell/goto verb for '{verb}'",
            );
        }
    }

    #[test]
    fn the_retired_cloud_plane_verb_reaches_the_workloads_surface() {
        // WL-ARCH-006 — the Cloud plane retired into the Workloads surface; a
        // forward emitter's old `cloud`/`instances` goto verb lands there, and
        // `cloud`/`controller` are no longer Workbench planes.
        assert!(matches!(
            resolve_action("shell/goto/cloud"),
            Some(Navigate::Surface(Surface::InfraCode))
        ));
        assert!(matches!(
            resolve_action("shell/goto/instances"),
            Some(Navigate::Surface(Surface::InfraCode))
        ));
        assert_eq!(plane_by_name("cloud"), None);
        assert_eq!(plane_by_name("controller"), None);
    }

    // ── suppress policy is pure ────────────────────────────────────────────────

    #[test]
    fn suppress_policy_matrix() {
        let dnd = Suppress {
            dnd: true,
            focus_mute: false,
            muted: false,
        };
        assert!(dnd.hides_ambient_push(Severity::Info));
        assert!(!dnd.hides_ambient_push(Severity::Critical));
        assert!(dnd.hushes_sound(Severity::Warning));
        assert!(!dnd.hushes_sound(Severity::Critical));
    }

    // ── the OSD emitter (KIRON-3 — the seat hotkeys flash it) ─────────────────

    #[test]
    fn flash_osd_lights_the_osd_channel_without_touching_the_alert_queue() {
        use mde_egui::{OsdKind, OsdLevel};
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        // A Critical alert and OSD remain independent visible channels.
        b.admit(decode(&body("critical", "lh1", "intrusion")).unwrap());
        b.flash_osd(OsdLevel::new(OsdKind::Volume, 0.4));
        assert!(b.host.osd_active(), "the volume hotkey lit the OSD tier");
        assert!(
            b.host.current().is_some(),
            "the OSD flash disturbed the active alert"
        );
        // The OSD is a direct channel — it never rings the notification chime.
        assert_eq!(*rec.0.borrow(), vec![Severity::Critical]);
    }

    // ── visible alert and OSD render mounts (§7) ──────────────────────────────

    #[test]
    fn an_alert_tessellates_a_notification_popup_through_the_bridge() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.admit(
            decode(r#"{"severity":"info","source_host":"nyc3","flag":"CHAT","headline":"a message","action_label":"Open","action_verb":"shell/goto/chat"}"#)
                .unwrap(),
        );

        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| {
            let _ = b.drive(ctx);
        });
        let out = ctx.run(input(), |ctx| {
            let nav = b.drive(ctx);
            assert!(nav.is_none(), "no alert action was clicked");
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the alert popup produced no geometry");
        assert_eq!(*rec.0.borrow(), vec![Severity::Info]);
        assert!(!b.host.is_idle());
    }

    #[test]
    fn shared_health_contract_tessellates_grade_metadata_through_the_bridge() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let rec = Recorder::default();
        let mut bridge = bridge_with(&rec);
        bridge.admit(
            decode(&health_kiron_body(
                mackes_mesh_types::health::GradeLetter::D,
            ))
            .expect("typed health alert admitted"),
        );

        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| {
            let _ = bridge.drive(ctx);
        });
        let output = ctx.run(input(), |ctx| {
            let navigation = bridge.drive(ctx);
            assert!(
                navigation.is_none(),
                "headless frame cannot click Control Panel"
            );
        });
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(
            !primitives.is_empty(),
            "the grade-bound lower third produced no geometry"
        );
        assert_eq!(*rec.0.borrow(), vec![Severity::Warning]);
        assert_eq!(
            bridge.host.current().map(|toast| toast.flag.as_str()),
            Some("HEALTH · GRADE D · 1m 10s · nvme0n1")
        );
    }

    #[test]
    fn the_osd_pill_still_tessellates_through_the_bridge() {
        use mde_egui::{OsdKind, OsdLevel};

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.flash_osd(OsdLevel::new(OsdKind::Brightness, 0.7));

        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| {
            let _ = b.drive(ctx);
        });
        let out = ctx.run(input(), |ctx| {
            let _ = b.drive(ctx);
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the OSD pill produced no geometry");
        assert!(rec.0.borrow().is_empty(), "OSD does not ring notifications");
    }
}
