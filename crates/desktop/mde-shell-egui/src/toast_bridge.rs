//! `toast_bridge` — the shell side of the **KIRON** chyron pattern (KIRON-2;
//! `docs/design/kiron-toast-pattern.md`, locks 7/8/10).
//!
//! KIRON-1 built the pure `mde_egui::toast::ToastHost` (queue + dwell + the two
//! renders). This module is the shell's owner of that one host: it
//!
//! * subscribes the typed Bus lane [`TOAST_TOPIC`] (`event/toast/show`) so any
//!   node / worker — `mackesd`, a remote peer — can raise a chyron fleet-wide
//!   (lock 7), decoding each body into an alert [`Toast`];
//! * drives the host once per frame ([`ToastBridge::drive`]): `tick` the real
//!   frame delta, drain the lane, then paint the lower-third chyron + the
//!   center-bottom OSD;
//! * fires **one** severity-scaled notification sound on a new alert (lock 8),
//!   the single sound authority — no double-beeps;
//! * applies **suppression** (lock 10): DND / a per-VM-session focus mute silence
//!   an Info/Warning chyron *and* its sound, audio-mute silences a non-critical's
//!   sound, but a **Critical always breaks through**; and
//! * resolves a clicked action verb to shell navigation — KIRON-1 only *reported*
//!   the verb; this is where it executes ([`resolve_action`]).
//!
//! The wire body is a JSON boundary (local serde structs, not a `mackesd`
//! dependency — §6 mesh/desktop boundary), the same pattern the Fleet plane and
//! the Chat surface use for their topics.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use mde_egui::egui;
use mde_egui::{OsdLevel, Severity, Tier, Toast, ToastHost};
use serde::Deserialize;

use crate::dock::Surface;
use crate::workbench::Plane;

/// The typed Bus lane any node / worker raises a chyron on (lock 7). Flat — the
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

/// Decode a raw `event/toast/show` body into an alert [`Toast`]. `None` on a
/// malformed body — a bad emitter never crashes the shell (it's silently dropped,
/// same as the Clipboard / Notifications tails).
fn decode(body: &str) -> Option<Toast> {
    serde_json::from_str::<ToastMsg>(body)
        .ok()
        .map(ToastMsg::into_toast)
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
    /// Do-Not-Disturb — silences an Info/Warning chyron + its sound.
    dnd: bool,
    /// A per-VM-session focus / gaming mute (a fullscreen guest is in front) —
    /// silences an Info/Warning chyron + its sound.
    focus_mute: bool,
    /// The seat's audio output is muted — additionally silences a non-critical's
    /// notification sound (the chyron still shows).
    muted: bool,
}

impl Suppress {
    /// Whether an alert of this severity's **chyron** is suppressed. A Critical
    /// always breaks through (safety over immersion — lock 10).
    const fn hides_chyron(self, severity: Severity) -> bool {
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
/// player is honest silence (no fake success): the process just fails to spawn.
struct SystemChime;

impl Chime for SystemChime {
    fn ring(&self, severity: Severity) {
        // Severity-scaled event id from the sound theme: a sharper cue for the
        // higher tiers, a soft one for Info.
        let event = match severity {
            Severity::Critical | Severity::Warning => "dialog-warning",
            Severity::Info => "message-new-instant",
        };
        let _ = Command::new("canberra-gtk-play")
            .args(["-i", event])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// What a clicked chyron action resolves to — where KIRON-2 executes the verb the
/// KIRON-1 render only reported.
pub(crate) enum Navigate {
    /// Switch the shell to this dock surface.
    Surface(Surface),
    /// Open the Workbench on this plane.
    Plane(Plane),
}

/// Resolve an opaque chyron action `verb` to shell navigation. The verb grammar is
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
        "workbench" => Some(Surface::Workbench),
        // OW-10 — the live Mesh Map. An all-green onboard self-test auto-opens it
        // through this same grammar (accepting the `mde-mesh-view` variant name too).
        "mesh-map" | "meshview" | "mesh" => Some(Surface::MeshView),
        "desktop" => Some(Surface::Desktop),
        "instances" => Some(Surface::Instances),
        // The Infra as Code (IaC) OpenStack control plane (IAC-2).
        "iac" | "infra-code" | "infracode" | "infra" => Some(Surface::InfraCode),
        "music" => Some(Surface::Music),
        "files" => Some(Surface::Files),
        "voice" => Some(Surface::Voice),
        // The ONE notification interface (NOTIFY-CHAT-6) — the retired
        // `notifications` / `clipboard` verbs now resolve here so a forward emitter's
        // old `shell/goto/notifications` still reaches a live surface.
        "chat" | "notifications" | "clipboard" => Some(Surface::Chat),
        "system" => Some(Surface::System),
        "storage" => Some(Surface::Storage),
        // The Timers & Alarms surface (VDOCK-5) — the clock's replacement; the
        // `clock` alias keeps a "where did the clock go?" verb landing somewhere
        // honest (lock #5: the clock is now Timers & Alarms).
        "timers" | "alarms" | "clock" => Some(Surface::Timers),
        _ => None,
    }
}

/// Map a `shell/plane/<name>` target to a Workbench [`Plane`] (case-insensitive).
fn plane_by_name(name: &str) -> Option<Plane> {
    match name.to_ascii_lowercase().as_str() {
        "thisnode" => Some(Plane::ThisNode),
        // QC-12 (Q70) — the Controller plane became the Cloud plane; a forward
        // emitter's old `shell/plane/controller` still reaches the live plane
        // (the retired-verb aliasing idiom `notifications`/`clipboard` use).
        "cloud" | "controller" => Some(Plane::Cloud),
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
}

impl Default for ToastBridge {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            cursor: None,
            last_poll: None,
            last_tick: None,
            host: ToastHost::new(),
            suppress: Suppress::default(),
            chime: Box::new(SystemChime),
        }
    }
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
    /// never opens a second toast channel — it hands its [`Toast`] here and the one
    /// [`ToastHost`] renders it.
    pub(crate) fn raise(&mut self, toast: Toast) {
        self.admit(toast);
    }

    /// Flash the center-bottom OSD level bar (volume / brightness), replacing any
    /// current one in place. This is the emitter KIRON-2 left waiting on the OSD tier
    /// (KIRON-3): the seat's volume/brightness hotkeys (E12-19) call it directly —
    /// the OSD is an instant hardware-feedback channel, never the Bus alert lane
    /// (lock 7). Because it is a direct in-shell call, DND / focus-mute suppression
    /// never applies (that governs *alert* chyrons, not a level flash).
    pub(crate) fn flash_osd(&mut self, level: OsdLevel) {
        self.host.flash_osd(level);
    }

    /// The per-frame drive: advance the countdowns by the real frame delta, drain
    /// any new `event/toast/show`, then paint the OSD tier + the lower-third chyron.
    /// Returns the navigation a clicked action verb resolved to, if any — the shell
    /// applies it (this is where the verb executes).
    pub(crate) fn drive(&mut self, ctx: &egui::Context) -> Option<Navigate> {
        self.tick(ctx);
        self.drain();
        // The center-bottom OSD tier is a separate, instant channel (KIRON-3 wires
        // its first in-shell emitter — the seat volume hotkey); painting it here
        // keeps the channel live and ready.
        self.host.osd(ctx);
        let clicked = self.host.chyron(ctx).action;
        clicked.as_deref().and_then(resolve_action)
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
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(msgs) = persist.list_since(TOAST_TOPIC, self.cursor.as_deref()) else {
            return;
        };
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

    /// Apply suppression (lock 10) then enqueue + ring (lock 8). A suppressed
    /// Info/Warning never reaches the queue nor rings; a Critical always does both.
    /// Split from the Bus read so the whole policy is unit-tested without a spool.
    fn admit(&mut self, toast: Toast) {
        let Some(severity) = alert_severity(&toast) else {
            // An OSD-tier toast on the alert lane would just flash — but the lane
            // only ever carries alerts; route it through the host regardless.
            self.host.enqueue(toast);
            return;
        };
        if self.suppress.hides_chyron(severity) {
            return;
        }
        self.host.enqueue(toast);
        if !self.suppress.hushes_sound(severity) {
            self.chime.ring(severity);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    use super::{
        alert_severity, decode, plane_by_name, resolve_action, surface_by_name, Chime, Navigate,
        Severity, Suppress, ToastBridge,
    };
    use crate::dock::Surface;
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
        assert!(b.host.has_critical(), "a Critical breaks through DND");
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
    fn audio_mute_hushes_the_sound_but_still_shows_the_chyron() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.set_suppression(false, false, true);
        b.admit(decode(&body("warning", "a", "build failed")).unwrap());
        assert!(!b.host.is_idle(), "the chyron still shows under audio-mute");
        assert!(rec.0.borrow().is_empty(), "but no sound fired");
    }

    #[test]
    fn a_plain_alert_shows_and_rings_exactly_once() {
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        b.admit(decode(&body("info", "nyc3", "hi")).unwrap());
        assert_eq!(
            b.host.current().map(|t| t.source_host.clone()),
            Some("nyc3".into())
        );
        assert_eq!(*rec.0.borrow(), vec![Severity::Info], "one beep, no double");
    }

    // ── action verb resolution (KIRON-2 executes it) ──────────────────────────

    #[test]
    fn resolve_action_maps_goto_and_plane_verbs() {
        assert!(matches!(
            resolve_action("shell/goto/chat"),
            Some(Navigate::Surface(Surface::Chat))
        ));
        // The retired notify/clipboard verbs now resolve to the ONE Chat surface
        // (NOTIFY-CHAT-6) so a forward emitter's old verb still reaches a surface.
        assert!(matches!(
            resolve_action("shell/goto/notifications"),
            Some(Navigate::Surface(Surface::Chat))
        ));
        assert!(matches!(
            resolve_action("shell/goto/clipboard"),
            Some(Navigate::Surface(Surface::Chat))
        ));
        assert!(matches!(
            resolve_action("shell/plane/fleet"),
            Some(Navigate::Plane(Plane::Fleet))
        ));
        // Unknown verbs are a no-op, not a panic.
        assert!(resolve_action("shell/goto/nope").is_none());
        assert!(resolve_action("chat/open/peer").is_none());
        assert!(resolve_action("").is_none());
    }

    #[test]
    fn name_maps_are_case_insensitive() {
        assert_eq!(surface_by_name("SYSTEM"), Some(Surface::System));
        assert_eq!(plane_by_name("ThisNode"), Some(Plane::ThisNode));
    }

    #[test]
    fn the_retired_controller_plane_verb_reaches_the_cloud_plane() {
        // QC-12 (Q70) — the rename must not strand a forward emitter's old verb.
        assert_eq!(plane_by_name("cloud"), Some(Plane::Cloud));
        assert_eq!(plane_by_name("controller"), Some(Plane::Cloud));
    }

    // ── suppress policy is pure ────────────────────────────────────────────────

    #[test]
    fn suppress_policy_matrix() {
        let dnd = Suppress {
            dnd: true,
            focus_mute: false,
            muted: false,
        };
        assert!(dnd.hides_chyron(Severity::Info));
        assert!(!dnd.hides_chyron(Severity::Critical));
        assert!(dnd.hushes_sound(Severity::Warning));
        assert!(!dnd.hushes_sound(Severity::Critical));
    }

    // ── the OSD emitter (KIRON-3 — the seat hotkeys flash it) ─────────────────

    #[test]
    fn flash_osd_lights_the_osd_channel_without_touching_the_alert_queue() {
        use mde_egui::{OsdKind, OsdLevel};
        let rec = Recorder::default();
        let mut b = bridge_with(&rec);
        // A pending Critical alert must be untouched by an OSD flash (separate tier).
        b.admit(decode(&body("critical", "lh1", "intrusion")).unwrap());
        b.flash_osd(OsdLevel::new(OsdKind::Volume, 0.4));
        assert!(b.host.osd_active(), "the volume hotkey lit the OSD tier");
        assert!(
            b.host.has_critical(),
            "the OSD flash left the alert queue alone"
        );
        // The OSD is a direct channel — it never rings the notification chime.
        assert_eq!(*rec.0.borrow(), vec![Severity::Critical]);
    }

    // ── the drain wiring renders a real band (mount-tessellate, §7) ────────────

    #[test]
    fn a_queued_toast_tessellates_a_real_band_through_the_bridge() {
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
        // Warm one frame (a fresh floating Area lays out invisibly first), then
        // tessellate the steady-state paint — the same two-frame path KIRON-1 uses.
        let _ = ctx.run(input(), |ctx| {
            let _ = b.drive(ctx);
        });
        let out = ctx.run(input(), |ctx| {
            let nav = b.drive(ctx);
            assert!(nav.is_none(), "no verb was clicked in a headless frame");
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the bridged chyron produced no geometry");
    }
}
