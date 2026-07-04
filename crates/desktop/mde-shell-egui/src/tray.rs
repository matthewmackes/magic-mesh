//! The taskbar **tray** (NAVBAR-W10-2, locks W2/W6..W11) — the right-justified
//! icon strip on the shell's ONE bar: the `^` overflow chevron, then Sessions
//! (only while a VDI session is live), Chat (unread count badge), Bluetooth,
//! Volume (muted variant when muted), Battery (charge-fill glyph + a bolt
//! overlay while charging), Status (worst-of mesh health), and the stacked
//! HH:MM-over-date clock at the right edge. Signal + Peers sit in the chevron's
//! anchored flyout (lock W10) — the Win10 hidden-icons well.
//!
//! Every icon renders **real state** (§7) folded from the same sources the
//! retired top chrome strip read: the world-readable mesh-status snapshot
//! ([`MeshSummary`], the tray's Peers/Status/Signal dots), the ONE `mde-seat`
//! [`SeatSnapshot`] (Bluetooth / Volume / Battery), and the Chat unread tally.
//! State rides a **tiny corner dot** in the OK/WARN/DANGER tokens while the
//! glyph keeps one tint (lock W9); a click **routes to the owning surface**
//! (lock W7) — except **Chat, Volume and Bluetooth**, whose clicks open the
//! W10-4 **micro-flyouts** (lock W7's exceptions), and there are **no tooltips
//! and no labels anywhere** on the bar (lock W6; the flyouts carry the names).
//!
//! The micro-flyouts (NAVBAR-W10-4) are Win10-register anchored popups, ONE
//! open at a time (opening any closes the rest, chevron included; click-away
//! dismisses): **Volume** = the master sink's name line + a slider + the
//! speaker-glyph mute toggle, driving the real mixer; **Bluetooth** = the
//! adapter radio power toggle + the connected-device count + an "Open
//! settings" row; **Chat** = the real unread tally + an "Open Chat" row (only
//! the folded tally reaches the tray — per-room counts / recent message lines
//! live on the Chat surface, honestly not mocked here). The volume/mute/radio
//! writes go through a narrow three-verb seam ([`TrayVerbs`]) over the same
//! host backends (`wpctl` / system-bus `BlueZ`) the seat snapshot is folded
//! from — the `Seat` handle itself lives in the System state and doesn't
//! reach the tray. A confirmed write **echoes** ([`Echoes`]) over the cached
//! snapshot until the System state's ~5s poll catches up, so the slider and
//! the strip icons read the new state back instantly; a refused write
//! surfaces its typed error in the flyout and echoes nothing (§7).
//!
//! The folds (glyph pick, dot tone, battery fill ladder, hidden-set
//! membership, Sessions transience, clock text, click routing, flyout copy,
//! echo reconcile/overlay, the verb drives) are pure — no egui `Context`, no
//! IO beyond the injected verb seam — so they're unit-tested directly; the
//! only egui here is the strip layout and the anchored flyout panels.

use std::time::{Duration, Instant};

use mde_egui::egui::{self, FontId, RichText};
use mde_egui::{muted_note, Style};

use mde_cosmic_applet::LighthouseHealth;
use mde_seat::{
    Battery, BatteryState, BluezClient, BtStatus, MixerClient, MixerStrip, Probe, PwGraph,
    SeatError, SeatSnapshot, ZbusBluez,
};
use mde_theme::brand::icons::IconId;

use crate::chrome::MeshSummary;
use crate::dock::{icon_texture, Surface};

/// The tray glyph edge in logical points — the 16px Win10 tray raster (lock
/// W3). `icon_texture` rasterizes it DPI-crisp at the physical size.
const TRAY_ICON: f32 = Style::SP_M;

/// One tray icon cell's width — the 16px glyph plus breathing room on the 8px
/// grid. The cell fills the bar's height so the whole column is clickable.
const TRAY_CELL_W: f32 = Style::SP_L + Style::SP_XS;

/// The corner status dot's radius (lock W9) — tiny, token-derived (§4).
const DOT_R: f32 = Style::SP_XS / 2.0;

/// Charge (%) below which a **draining** system pack's dot reads amber "low",
/// and at or below which it reads red "critical" (lock W8). A charging or full
/// pack is never amber/red (it's improving); these bite only while the pack is
/// actually discharging. Moved verbatim from the retired chrome strip.
const BATTERY_LOW: f64 = 20.0;
const BATTERY_CRITICAL: f64 = 5.0;

/// One micro-flyout row's height — compact, zebra-free rows on the 8px grid.
const ROW_H: f32 = Style::SP_L;

/// The micro-flyout rows' fixed width — token math (6 × `SP_XL` = 192pt),
/// wide enough for the volume slider + its value readout in one line.
const FLYOUT_W: f32 = Style::SP_XL * 6.0;

/// How long a confirmed write's echo may outlive the snapshot: one ~5s System
/// seat poll plus slack. Past this, the poll has had its chance and the
/// snapshot is the truth again (a concurrent change made from the System
/// surface must not be overridden forever by a stale echo).
const ECHO_TTL: Duration = Duration::from_secs(8);

// ── the flyout design copy (`Surface::label()`/`hint()` were deleted in W10-2,
//    so the rows carry their own const strings from the design doc) ──────────

/// The Chat flyout's routing row.
const OPEN_CHAT: &str = "Open Chat";
/// The Bluetooth flyout's routing row (lock W7's "Open settings" — the System
/// surface owns the full Bluetooth panel).
const OPEN_SETTINGS: &str = "Open settings";
/// The Volume flyout's fallback routing row when the mixer probe is absent.
const OPEN_SYSTEM: &str = "Open System";
/// The Bluetooth power row's label.
const BT_POWER: &str = "Bluetooth";
/// The Bluetooth power row's trailing state while the radio is on.
const BT_ON: &str = "On";
/// The Bluetooth power row's trailing state while the radio is off.
const BT_OFF: &str = "Off";
/// The honest absent-mixer line (§7) — no fake slider over a missing backend.
const MIXER_ABSENT: &str = "Mixer: not available on this seat.";
/// The honest absent-`BlueZ` line (§7) — no fake toggle over a missing radio.
const BT_ABSENT: &str = "Bluetooth: not available on this seat.";

// ─────────────────────────────── the tray model ──────────────────────────────

/// One tray icon slot. The strip/hidden partition ([`strip_items`] /
/// [`HIDDEN`]) and the click routing ([`route`]) are keyed off this — pure and
/// unit-tested. (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate`
/// form for crate-visible items in this private module, like `dock::TASKBAR_H`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayItem {
    /// The live VDI session marker — visible ONLY while a session is
    /// connected/pending (lock W10's transient rule).
    Sessions,
    /// The unified Chat surface's unread tally (count badge, lock W7).
    Chat,
    /// Bluetooth adapter power (from the seat snapshot).
    Bluetooth,
    /// Master output volume / mute (from the seat snapshot).
    Volume,
    /// The system battery pack — charge-fill glyph + bolt + tone dot (lock W8).
    Battery,
    /// Worst-of mesh lighthouse health (the fleet Status verdict).
    Status,
    /// Mesh reachability — hidden in the chevron flyout by default (lock W10).
    Signal,
    /// Peer directory presence — hidden in the chevron flyout by default.
    Peers,
}

/// The visible strip between the chevron and the clock, left→right, with the
/// transient Sessions slot leading while a VDI session is live (lock W10).
const STRIP: [TrayItem; 5] = [
    TrayItem::Chat,
    TrayItem::Bluetooth,
    TrayItem::Volume,
    TrayItem::Battery,
    TrayItem::Status,
];
const STRIP_WITH_SESSION: [TrayItem; 6] = [
    TrayItem::Sessions,
    TrayItem::Chat,
    TrayItem::Bluetooth,
    TrayItem::Volume,
    TrayItem::Battery,
    TrayItem::Status,
];

/// The chevron flyout's hidden set (lock W10): Signal + Peers by default (the
/// compact-mode fold-in is W10-5).
const HIDDEN: [TrayItem; 2] = [TrayItem::Signal, TrayItem::Peers];

/// The visible strip for this frame — Sessions leads only while a VDI session
/// is active; the fixed Chat · BT · Volume · Battery · Status run follows.
const fn strip_items(session_active: bool) -> &'static [TrayItem] {
    if session_active {
        &STRIP_WITH_SESSION
    } else {
        &STRIP
    }
}

/// Where a tray icon's routing lands (lock W7): the surface that owns its
/// state — seat hardware to System, mesh telemetry to the Mesh Map, Chat to
/// Chat, the live session to Desktop. Battery/Status/Signal/Peers/Sessions
/// route on a plain click; Chat/Volume/Bluetooth route through their
/// micro-flyouts' "Open …" rows (W10-4). The clock routes to System in
/// [`clock_cell`]'s caller.
const fn route(item: TrayItem) -> Surface {
    match item {
        TrayItem::Sessions => Surface::Desktop,
        TrayItem::Chat => Surface::Chat,
        TrayItem::Bluetooth | TrayItem::Volume | TrayItem::Battery => Surface::System,
        TrayItem::Status | TrayItem::Signal | TrayItem::Peers => Surface::MeshView,
    }
}

/// Which anchored flyout is showing — at most **one** (lock W7: opening any
/// closes the rest; the `^` chevron's hidden-icon well shares the same
/// exclusivity). The tray's only cross-frame UI latch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OpenFlyout {
    /// Nothing open — the quiet bar.
    #[default]
    None,
    /// The `^` chevron's hidden-icon well (Signal + Peers, lock W10).
    Chevron,
    /// The Chat micro-flyout (unread tally + the Open Chat row).
    Chat,
    /// The Volume micro-flyout (name line + slider + mute toggle).
    Volume,
    /// The Bluetooth micro-flyout (radio toggle + count + Open settings).
    Bluetooth,
}

/// The micro-flyout a strip icon owns (lock W7's exceptions): Chat, Volume
/// and Bluetooth open anchored flyouts; every other icon plain-routes.
const fn micro_flyout(item: TrayItem) -> Option<OpenFlyout> {
    match item {
        TrayItem::Chat => Some(OpenFlyout::Chat),
        TrayItem::Volume => Some(OpenFlyout::Volume),
        TrayItem::Bluetooth => Some(OpenFlyout::Bluetooth),
        TrayItem::Sessions
        | TrayItem::Battery
        | TrayItem::Status
        | TrayItem::Signal
        | TrayItem::Peers => None,
    }
}

// ─────────────────────────────── the pure folds ──────────────────────────────

/// Everything the tray folds from, bundled so the `main → dock::taskbar → tray`
/// call chain hands one immutable view of the frame's live state.
pub struct TrayInputs<'a> {
    /// The mesh-status snapshot fold — the same [`crate::chrome::ChromeState`]
    /// poll product the retired strip read (one poll, no second reader).
    pub mesh: &'a MeshSummary,
    /// The ONE `mde-seat` snapshot (Bluetooth / Volume / Battery), `None`
    /// before the System state's first poll lands.
    pub seat: Option<&'a SeatSnapshot>,
    /// The whole-mesh Chat unread tally (folded alerts + clips + messages).
    pub unread: usize,
    /// `true` while a VDI session is connected/pending — the Sessions slot's
    /// transient-visibility signal (lock W10).
    pub session_active: bool,
}

/// One resolved tray icon: the glyph to draw, its corner-dot tone, whether the
/// charging bolt overlays it, and the Chat count badge (which replaces the
/// dot). Pure data — the fold is unit-tested without egui.
struct IconView {
    /// The brand glyph (16px tray set, or the reused Node/MeshView/Chat).
    glyph: IconId,
    /// The corner status dot's tone — an OK/WARN/DANGER/dim `Style` token.
    dot: egui::Color32,
    /// Overlay [`IconId::BatteryBolt`] in the same rect (charging, lock W8).
    bolt: bool,
    /// The unread count badge ("99+"-capped); drawn instead of the dot.
    badge: Option<String>,
}

/// Fold one tray item's live view from the frame's inputs — the single
/// glyph/dot/bolt/badge authority the strip AND the flyout render through.
fn icon_view(item: TrayItem, inputs: &TrayInputs<'_>) -> IconView {
    let plain = |glyph: IconId, dot: egui::Color32| IconView {
        glyph,
        dot,
        bolt: false,
        badge: None,
    };
    match item {
        // Visible only while a session is live, so its dot is honestly OK.
        TrayItem::Sessions => plain(IconId::Sessions, Style::OK),
        TrayItem::Chat => IconView {
            glyph: IconId::Chat,
            dot: chat_dot(inputs.unread),
            bolt: false,
            badge: chat_badge(inputs.unread),
        },
        TrayItem::Bluetooth => plain(IconId::BluetoothSmall, bluetooth_dot(inputs.seat)),
        TrayItem::Volume => {
            let (glyph, dot) = volume_view(inputs.seat);
            plain(glyph, dot)
        }
        TrayItem::Battery => {
            let (glyph, bolt, dot) = battery_indicator(inputs.seat);
            IconView {
                glyph,
                dot,
                bolt,
                badge: None,
            }
        }
        TrayItem::Status => plain(IconId::Node, status_dot(inputs.mesh)),
        TrayItem::Signal => plain(IconId::Signal, signal_dot(inputs.mesh)),
        TrayItem::Peers => plain(IconId::MeshView, peers_dot(inputs.mesh)),
    }
}

/// The Chat dot when no badge shows: accent while something waits, dim quiet.
const fn chat_dot(unread: usize) -> egui::Color32 {
    if unread > 0 {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    }
}

/// The Chat unread count badge — `None` when quiet, capped at "99+" so a
/// firehose can't stretch the cell (the same cap the retired strip used).
fn chat_badge(unread: usize) -> Option<String> {
    match unread {
        0 => None,
        1..=99 => Some(unread.to_string()),
        _ => Some("99+".to_string()),
    }
}

/// The Bluetooth dot: OK while any adapter is powered; dim when powered off,
/// absent (no `BlueZ` / no bus — the build-host case), or not yet polled.
/// Never a fabricated radio state (§7).
fn bluetooth_dot(seat: Option<&SeatSnapshot>) -> egui::Color32 {
    match seat.map(|s| &s.bluetooth) {
        Some(Probe::Present(bt)) if bt.any_adapter_powered() => Style::OK,
        _ => Style::TEXT_DIM,
    }
}

/// The Volume glyph + dot: the muted variant with a WARN dot while the master
/// output is muted (the at-a-glance "you're silent" state), the plain speaker
/// with an OK dot while live, and dim when the mixer is absent / not yet
/// polled. Never a fake level (§7).
fn volume_view(seat: Option<&SeatSnapshot>) -> (IconId, egui::Color32) {
    match seat.map(|s| &s.mixer) {
        Some(Probe::Present(m)) if m.master.muted => (IconId::VolumeMuted, Style::WARN),
        Some(Probe::Present(_)) => (IconId::Volume, Style::OK),
        _ => (IconId::Volume, Style::TEXT_DIM),
    }
}

/// The Battery slot: `(fill glyph, charging bolt, dot tone)` for the system
/// pack (lock W8). An absent backend / empty snapshot / pre-poll state reads
/// the dim empty outline — honest, never a fabricated level (§7).
fn battery_indicator(seat: Option<&SeatSnapshot>) -> (IconId, bool, egui::Color32) {
    match seat.map(|s| &s.batteries) {
        Some(Probe::Present(cells)) => {
            system_pack(cells).map_or((IconId::BatteryEmpty, false, Style::TEXT_DIM), |b| {
                (
                    battery_fill_icon(b.percentage),
                    charging(b.state),
                    battery_tone(b),
                )
            })
        }
        _ => (IconId::BatteryEmpty, false, Style::TEXT_DIM),
    }
}

/// Map a charge percentage onto the five-step Win10 fill ladder (lock W8):
/// each glyph owns the band centred on its step, so the icon reads the nearest
/// quarter — `Empty` < 12.5 ≤ `Quarter` < 37.5 ≤ `Half` < 62.5 ≤
/// `ThreeQuarter` < 87.5 ≤ `Full`.
fn battery_fill_icon(percentage: f64) -> IconId {
    if percentage < 12.5 {
        IconId::BatteryEmpty
    } else if percentage < 37.5 {
        IconId::BatteryQuarter
    } else if percentage < 62.5 {
        IconId::BatteryHalf
    } else if percentage < 87.5 {
        IconId::BatteryThreeQuarter
    } else {
        IconId::BatteryFull
    }
}

/// Whether the pack is taking charge — the bolt-overlay signal (lock W8). A
/// pending-charge pack is on AC too, so it carries the bolt like the retired
/// strip's `⚡` suffix did.
const fn charging(state: BatteryState) -> bool {
    matches!(state, BatteryState::Charging | BatteryState::PendingCharge)
}

/// The battery dot's tone for the chosen system pack — moved verbatim from the
/// retired chrome strip's `battery_tone` (the value-colour half is gone with
/// the text). A charging or full pack reads OK; a draining pack reads red at or
/// under ~5% (or when `UPower` reports it empty) and amber under ~20%; anything
/// else (a healthily draining pack, a pending state) reads the neutral dim dot.
fn battery_tone(b: &Battery) -> egui::Color32 {
    match b.state {
        BatteryState::Charging | BatteryState::FullyCharged => Style::OK,
        BatteryState::Empty => Style::DANGER,
        BatteryState::Discharging | BatteryState::PendingDischarge => {
            if b.percentage <= BATTERY_CRITICAL {
                Style::DANGER
            } else if b.percentage < BATTERY_LOW {
                Style::WARN
            } else {
                Style::TEXT_DIM
            }
        }
        BatteryState::PendingCharge | BatteryState::Unknown => Style::TEXT_DIM,
    }
}

/// Pick the system pack to summarise from a multi-battery snapshot — moved
/// verbatim from the retired chrome strip: the `PowerSupply` pack that actually
/// powers the host, else — when none is flagged (an all-peripheral snapshot) —
/// the fullest cell, so the slot never invents a reading. `None` only for an
/// empty list (the caller renders the dim empty outline).
fn system_pack(cells: &[Battery]) -> Option<&Battery> {
    cells.iter().find(|b| b.power_supply).or_else(|| {
        cells.iter().max_by(|a, b| {
            a.percentage
                .partial_cmp(&b.percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    })
}

/// The Status dot: the worst-of lighthouse verdict — OK all-healthy, DANGER
/// degraded, dim with no lighthouses in view or before the first snapshot.
const fn status_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen {
        return Style::TEXT_DIM;
    }
    match s.health {
        LighthouseHealth::AllHealthy => Style::OK,
        LighthouseHealth::Degraded => Style::DANGER,
        LighthouseHealth::None => Style::TEXT_DIM,
    }
}

/// The Signal dot: mesh reachability — OK while any peer answers, WARN when
/// the directory is populated but nobody does (isolated), dim empty/unseen.
const fn signal_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen || s.peers_total == 0 {
        Style::TEXT_DIM
    } else if s.peers_online == 0 {
        Style::WARN
    } else {
        Style::OK
    }
}

/// The Peers dot: OK all-online, WARN some-away, dim empty/unseen.
const fn peers_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen || s.peers_total == 0 {
        Style::TEXT_DIM
    } else if s.peers_online == s.peers_total {
        Style::OK
    } else {
        Style::WARN
    }
}

/// The stacked clock's two lines (lock W11): wall-clock `HH:MM` over the civil
/// `YYYY-MM-DD` date, UTC — the same no-time-crate calendar math the Chat
/// timeline uses ([`crate::chat::civil_from_days`]), so the shell never claims
/// a local zone it can't know.
fn clock_lines(unix_secs: i64) -> (String, String) {
    let tod = unix_secs.rem_euclid(86_400);
    let (year, month, day) = crate::chat::civil_from_days(unix_secs.div_euclid(86_400));
    (
        format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60),
        format!("{year:04}-{month:02}-{day:02}"),
    )
}

/// The mixer's master strip, when the probe answered — the Volume flyout's
/// one subject (the tray drives master only; the full strip board is the
/// System surface's).
fn master_of(seat: Option<&SeatSnapshot>) -> Option<&MixerStrip> {
    seat?.mixer.present().map(|m| &m.master)
}

/// The Bluetooth status, when the probe answered — the Bluetooth flyout's
/// subject.
fn bt_of(seat: Option<&SeatSnapshot>) -> Option<&BtStatus> {
    seat?.bluetooth.present()
}

/// The Chat flyout's header — the SAME whole-mesh unread tally the badge
/// counts, spelled out. (Per-room counts and recent message lines don't reach
/// the tray — only the folded tally rides [`TrayInputs`] — so the flyout
/// honestly shows the count and routes to the Chat surface for the rooms.)
fn chat_flyout_header(unread: usize) -> String {
    match unread {
        0 => "No unread messages".to_owned(),
        1 => "1 unread message".to_owned(),
        n => format!("{n} unread messages"),
    }
}

/// The Bluetooth flyout's connected-device line — the real count from the
/// `BlueZ` enumeration, never a fabricated presence.
fn connected_line(connected: usize) -> String {
    match connected {
        0 => "No devices connected".to_owned(),
        1 => "1 device connected".to_owned(),
        n => format!("{n} devices connected"),
    }
}

// ───────────────────────── the verb seam + the echoes ────────────────────────

/// The three host-control writes lock W7 grants the tray's micro-flyouts —
/// master volume, master mute, adapter radio power — behind one narrow seam,
/// so the flyouts can't reach wider than the design allows and the tests can
/// inject a recording fake. The System surface owns every OTHER host verb
/// through the ONE `mde-seat` `Seat` (lock 1); that handle lives in the
/// System state and doesn't reach the tray, so these three write through
/// their own lazy clients to the SAME backends (`wpctl` / system-bus `BlueZ`)
/// the seat snapshot — the state the tray renders — is folded from.
trait TrayVerbs: Send {
    /// Set the master output strip's volume (0–100).
    ///
    /// # Errors
    /// The mixer client's typed errors (absent `PipeWire` → `Unavailable`,
    /// a control failure → `Backend`).
    fn set_master_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError>;

    /// Set the master output strip's mute.
    ///
    /// # Errors
    /// As [`Self::set_master_volume`].
    fn set_master_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError>;

    /// Power a Bluetooth adapter's radio on/off (`Adapter1.Powered`).
    ///
    /// # Errors
    /// The `BlueZ` client's typed errors (absent adapter / dead bus →
    /// `Unavailable`, a refused write → `Backend`).
    fn set_bt_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError>;
}

/// The production verbs: `PipeWire` via `wpctl` + the system-bus `BlueZ` —
/// the same backends the seat snapshot reads. Both clients are lazy: no I/O
/// until a flyout control is actually flipped, so a headless tray never
/// touches the host.
struct HostVerbs {
    /// The `PipeWire` graph client (volume/mute via `wpctl`).
    mixer: PwGraph,
    /// The system-bus `BlueZ` client (radio power).
    bluez: ZbusBluez,
}

impl HostVerbs {
    /// Wire the live host clients (no I/O here).
    fn new() -> Self {
        Self {
            mixer: PwGraph::new(),
            bluez: ZbusBluez::new(),
        }
    }
}

impl TrayVerbs for HostVerbs {
    fn set_master_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError> {
        self.mixer.set_volume(strip_id, volume)
    }

    fn set_master_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError> {
        self.mixer.set_muted(strip_id, muted)
    }

    fn set_bt_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.bluez.set_adapter_powered(adapter, on)
    }
}

/// A confirmed write's echo — the value a verb just wrote, keyed to the exact
/// strip/adapter it wrote, held until the System state's ~5s seat poll
/// catches up (or [`ECHO_TTL`] passes). The tray reads its snapshot
/// immutably, so this is its equivalent of the System state's
/// accumulate-in-place cache patch: set ONLY on a verb's `Ok` (§7 — a refused
/// write never pretends), it lets the slider, the toggles and the strip icons
/// read the just-written state back instantly instead of snapping to the
/// stale snapshot between polls.
struct Echo<T> {
    /// The written object — the master strip id / the adapter path.
    key: String,
    /// The confirmed written value.
    value: T,
    /// When the write was confirmed (the TTL clock).
    at: Instant,
}

/// The tray's in-flight write echoes, one slot per flyout verb.
#[derive(Default)]
struct Echoes {
    /// Master volume, from the Volume flyout's slider.
    volume: Option<Echo<u8>>,
    /// Master mute, from the Volume flyout's speaker toggle.
    muted: Option<Echo<bool>>,
    /// Adapter radio power, from the Bluetooth flyout's toggle.
    bt_powered: Option<Echo<bool>>,
}

impl Echoes {
    /// Whether any echo is in flight (the cheap gate before [`Self::overlay`]
    /// clones anything).
    const fn is_empty(&self) -> bool {
        self.volume.is_none() && self.muted.is_none() && self.bt_powered.is_none()
    }

    /// Drop every echo the snapshot has caught up with, plus any that
    /// outlived [`ECHO_TTL`] — past that, the poll has had its chance and the
    /// real state wins again.
    fn reconcile(&mut self, seat: Option<&SeatSnapshot>, now: Instant) {
        let master = master_of(seat);
        if let Some(e) = &self.volume {
            if now.duration_since(e.at) >= ECHO_TTL
                || master.is_some_and(|m| m.id == e.key && m.volume == e.value)
            {
                self.volume = None;
            }
        }
        if let Some(e) = &self.muted {
            if now.duration_since(e.at) >= ECHO_TTL
                || master.is_some_and(|m| m.id == e.key && m.muted == e.value)
            {
                self.muted = None;
            }
        }
        if let Some(e) = &self.bt_powered {
            let caught_up = bt_of(seat).is_some_and(|bt| {
                bt.adapters
                    .iter()
                    .any(|a| a.path == e.key && a.powered == e.value)
            });
            if now.duration_since(e.at) >= ECHO_TTL || caught_up {
                self.bt_powered = None;
            }
        }
    }

    /// Fold the in-flight echoes over this frame's snapshot: a patched clone
    /// while any write is echoing (rare — the ≤[`ECHO_TTL`] after a flyout
    /// verb), `None` otherwise so the ordinary frame renders the snapshot
    /// borrow untouched. Patches key-matched objects only; an echo whose
    /// strip/adapter re-enumerated away stays inert until its TTL clears it.
    fn overlay(&self, seat: Option<&SeatSnapshot>) -> Option<SeatSnapshot> {
        if self.is_empty() {
            return None;
        }
        let mut snap = seat?.clone();
        if let Probe::Present(m) = &mut snap.mixer {
            if let Some(e) = &self.volume {
                if m.master.id == e.key {
                    m.master.volume = e.value;
                }
            }
            if let Some(e) = &self.muted {
                if m.master.id == e.key {
                    m.master.muted = e.value;
                }
            }
        }
        if let Some(e) = &self.bt_powered {
            if let Probe::Present(bt) = &mut snap.bluetooth {
                if let Some(a) = bt.adapters.iter_mut().find(|a| a.path == e.key) {
                    a.powered = e.value;
                }
            }
        }
        Some(snap)
    }
}

/// Drive the master volume through the verb seam: on `Ok`, echo the confirmed
/// level so the slider + icon read it back instantly; on a refusal, surface
/// the typed error and echo NOTHING (§7 — the stale snapshot stays the truth).
fn drive_master_volume(
    verbs: &dyn TrayVerbs,
    echoes: &mut Echoes,
    error: &mut Option<String>,
    strip_id: &str,
    volume: u8,
    now: Instant,
) {
    match verbs.set_master_volume(strip_id, volume) {
        Ok(()) => {
            *error = None;
            echoes.volume = Some(Echo {
                key: strip_id.to_owned(),
                value: volume,
                at: now,
            });
        }
        Err(e) => *error = Some(format!("volume: {e}")),
    }
}

/// Drive the master mute — the same confirm-then-echo contract as
/// [`drive_master_volume`]; the echo is what flips the strip's `VolumeMuted`
/// glyph live.
fn drive_master_mute(
    verbs: &dyn TrayVerbs,
    echoes: &mut Echoes,
    error: &mut Option<String>,
    strip_id: &str,
    muted: bool,
    now: Instant,
) {
    match verbs.set_master_muted(strip_id, muted) {
        Ok(()) => {
            *error = None;
            echoes.muted = Some(Echo {
                key: strip_id.to_owned(),
                value: muted,
                at: now,
            });
        }
        Err(e) => *error = Some(format!("mute: {e}")),
    }
}

/// Drive an adapter's radio power — the same confirm-then-echo contract; the
/// echo is what flips the strip's Bluetooth dot live.
fn drive_bt_power(
    verbs: &dyn TrayVerbs,
    echoes: &mut Echoes,
    error: &mut Option<String>,
    adapter: &str,
    on: bool,
    now: Instant,
) {
    match verbs.set_bt_powered(adapter, on) {
        Ok(()) => {
            *error = None;
            echoes.bt_powered = Some(Echo {
                key: adapter.to_owned(),
                value: on,
                at: now,
            });
        }
        Err(e) => *error = Some(format!("Bluetooth: {e}")),
    }
}

// ─────────────────────────────── the tray strip ──────────────────────────────

/// The tray's cross-frame state: which flyout is open, the in-flight write
/// echoes, the verb seam, and the last refused verb's message. Everything
/// rendered is still folded fresh from [`TrayInputs`] each frame.
pub struct TrayState {
    /// Which anchored flyout is showing — at most ONE (lock W7).
    open: OpenFlyout,
    /// Confirmed-write echoes bridging the ~5s seat-poll gap.
    echoes: Echoes,
    /// The narrow three-verb seam the micro-flyouts drive (real `wpctl` /
    /// `BlueZ` in production, a recording fake in tests).
    verbs: Box<dyn TrayVerbs>,
    /// The last refused verb's typed message, shown inside the owning flyout
    /// (§7 — a failed write surfaces honestly, never a silent no-op). Cleared
    /// by the next successful verb and whenever a flyout opens.
    error: Option<String>,
}

impl Default for TrayState {
    /// The production tray: verbs over the live host backends. Constructing
    /// them is free — no I/O happens until a flyout control is actually
    /// flipped — so a default tray in a headless test never touches the host.
    fn default() -> Self {
        Self {
            open: OpenFlyout::None,
            echoes: Echoes::default(),
            verbs: Box::new(HostVerbs::new()),
            error: None,
        }
    }
}

impl TrayState {
    /// Toggle flyout `f`: a second click on the owner closes it; anything
    /// else opens it, displacing whatever was open — ONE flyout at a time
    /// (lock W7), the chevron included. Opening clears the stale verb error.
    /// Returns `true` when `f` is now open (the caller's same-frame
    /// click-away guard).
    fn toggle(&mut self, f: OpenFlyout) -> bool {
        self.open = if self.open == f { OpenFlyout::None } else { f };
        let opened = self.open == f;
        if opened {
            self.error = None;
        }
        opened
    }
}

/// The per-frame anchor rects of the three micro-flyout owner cells, captured
/// while the strip paints (their x shifts with the transient Sessions slot),
/// so the open flyout hangs off its live cell.
#[derive(Default, Clone, Copy)]
struct FlyoutAnchors {
    /// The Chat cell's rect.
    chat: Option<egui::Rect>,
    /// The Volume cell's rect.
    volume: Option<egui::Rect>,
    /// The Bluetooth cell's rect.
    bluetooth: Option<egui::Rect>,
}

impl FlyoutAnchors {
    /// Record a strip cell's rect under its micro-flyout, if it owns one.
    const fn record(&mut self, fly: Option<OpenFlyout>, rect: egui::Rect) {
        match fly {
            Some(OpenFlyout::Chat) => self.chat = Some(rect),
            Some(OpenFlyout::Volume) => self.volume = Some(rect),
            Some(OpenFlyout::Bluetooth) => self.bluetooth = Some(rect),
            Some(OpenFlyout::None | OpenFlyout::Chevron) | None => {}
        }
    }
}

/// Render the right-justified tray into a right-to-left `ui` (the taskbar's
/// trailing layout): clock · Status · Battery · Volume · Bluetooth · Chat ·
/// [Sessions] · the `^` chevron, plus whichever anchored flyout is open.
/// Battery/Status/Sessions clicks route `active` to the owning surface (lock
/// W7); Chat/Volume/Bluetooth clicks toggle their micro-flyouts (W10-4),
/// whose "Open …" rows route instead. Returns `true` when any click routed
/// this frame so the shell can surface the body behind a session.
pub fn tray(
    ui: &mut egui::Ui,
    state: &mut TrayState,
    active: &mut Surface,
    inputs: &TrayInputs<'_>,
) -> bool {
    // Echoes first: drop any the seat poll has caught up with (or that
    // expired), then fold the survivors over this frame's snapshot so a
    // just-driven mute/volume/radio flip reads back instantly everywhere —
    // the strip icons and the open flyout render one truth.
    let now = Instant::now();
    state.echoes.reconcile(inputs.seat, now);
    let patched = state.echoes.overlay(inputs.seat);
    let inputs = &TrayInputs {
        mesh: inputs.mesh,
        seat: patched.as_ref().or(inputs.seat),
        unread: inputs.unread,
        session_active: inputs.session_active,
    };

    // The stacked clock at the right edge (lock W11); a click opens System.
    let clock_clicked = clock_cell(ui);
    if clock_clicked {
        *active = Surface::System;
        state.open = OpenFlyout::None;
    }
    let mut routed = clock_clicked;

    // The visible strip — painted right→left, so iterate the left→right order
    // reversed; Status lands beside the clock, Chat (or Sessions) leftmost.
    // `opened` marks the click that just opened the now-open flyout, so the
    // flyout's same-frame click-away check doesn't read its own opening click
    // (which lands outside the popup) as a dismissal.
    let mut anchors = FlyoutAnchors::default();
    let mut opened = false;
    for item in strip_items(inputs.session_active).iter().rev() {
        let view = icon_view(*item, inputs);
        let response = icon_cell(ui, &view, egui::vec2(TRAY_CELL_W, ui.available_height()));
        let fly = micro_flyout(*item);
        if response.clicked() {
            if let Some(f) = fly {
                opened = state.toggle(f);
            } else {
                *active = route(*item);
                state.open = OpenFlyout::None;
                routed = true;
            }
        }
        anchors.record(fly, response.rect);
    }

    // The `^` overflow chevron heads the tray (lock W10); its anchored well
    // holds the hidden Signal + Peers icons and shares the flyout exclusivity.
    let chevron = chevron_cell(ui);
    if chevron.clicked() {
        opened = state.toggle(OpenFlyout::Chevron);
    }

    // ONE flyout at a time: render whichever is open, anchored to its owning
    // cell; a routing row inside switches the surface and closes it.
    let target = match state.open {
        OpenFlyout::None => None,
        OpenFlyout::Chevron => chevron_flyout(ui.ctx(), chevron.rect, inputs, opened, state),
        OpenFlyout::Chat => anchors
            .chat
            .and_then(|a| chat_flyout(ui.ctx(), a, inputs, opened, state)),
        OpenFlyout::Volume => anchors
            .volume
            .and_then(|a| volume_flyout(ui.ctx(), a, inputs, opened, state, now)),
        OpenFlyout::Bluetooth => anchors
            .bluetooth
            .and_then(|a| bluetooth_flyout(ui.ctx(), a, inputs, opened, state, now)),
    };
    if let Some(t) = target {
        *active = t;
        state.open = OpenFlyout::None;
        routed = true;
    }
    routed
}

/// One tray icon cell: hover fill only — NO tooltip (lock W6) — the 16px glyph
/// at one tint (lock W9), the bolt overlay, and the corner dot (or the Chat
/// badge in its place). Returns the cell's response (click + the anchor rect
/// the micro-flyouts hang from).
fn icon_cell(ui: &mut egui::Ui, view: &IconView, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(TRAY_ICON, TRAY_ICON));
    if let Some(tex) = icon_texture(ui.ctx(), view.glyph, TRAY_ICON, Style::TEXT_DIM) {
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    // The charging bolt overlays the fill glyph in the same rect (lock W8),
    // brighter than the outline so it reads at 16px.
    if view.bolt {
        if let Some(tex) = icon_texture(ui.ctx(), IconId::BatteryBolt, TRAY_ICON, Style::TEXT) {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
                .paint_at(ui, icon_rect);
        }
    }
    if let Some(text) = &view.badge {
        // The Chat unread badge — an accent pill on the glyph's top-right
        // corner, replacing the dot (the count IS the state).
        let font = FontId::proportional(Style::SMALL * 0.75);
        let galley = ui.fonts(|f| f.layout_no_wrap(text.clone(), font, Style::BG));
        let text_size = galley.size();
        let badge_rect = egui::Rect::from_center_size(
            icon_rect.right_top(),
            egui::vec2(text_size.x + Style::SP_XS, text_size.y),
        );
        painter.rect_filled(badge_rect, Style::RADIUS, Style::ACCENT);
        painter.galley(badge_rect.center() - text_size / 2.0, galley, Style::BG);
    } else {
        // The tiny corner status dot (lock W9) on the glyph's bottom-right.
        painter.circle_filled(icon_rect.right_bottom(), DOT_R, view.dot);
    }
    response
}

/// The `^` chevron cell — glyph only, hover fill, no dot (it carries no state;
/// the hidden icons behind it carry their own).
fn chevron_cell(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(TRAY_CELL_W, ui.available_height()),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if let Some(tex) = icon_texture(ui.ctx(), IconId::ChevronUp, TRAY_ICON, Style::TEXT_DIM) {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(TRAY_ICON, TRAY_ICON));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    response
}

/// The stacked clock (lock W11): `HH:MM` over `YYYY-MM-DD` in small token
/// text, right edge, hover fill, no tooltip. Returns `true` on a click (the
/// caller routes to System). The repaint heartbeat rides the chrome poll's
/// shared cadence, so the minute flip surfaces without input.
fn clock_cell(ui: &mut egui::Ui) -> bool {
    // A small margin keeps the clock off the screen's right edge (RTL: the
    // first space paints rightmost).
    ui.add_space(Style::SP_S);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let (time, date) = clock_lines(now);
    let font = FontId::proportional(Style::SMALL);
    let time_galley = ui.fonts(|f| f.layout_no_wrap(time, font.clone(), Style::TEXT));
    let date_galley = ui.fonts(|f| f.layout_no_wrap(date, font, Style::TEXT_DIM));
    let (time_size, date_size) = (time_galley.size(), date_galley.size());

    let w = time_size.x.max(date_size.x) + Style::SP_S;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(w, ui.available_height()), egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let gap = Style::SP_XS / 2.0;
    let top = rect.center().y - (time_size.y + gap + date_size.y) / 2.0;
    painter.galley(
        egui::pos2(rect.center().x - time_size.x / 2.0, top),
        time_galley,
        Style::TEXT,
    );
    painter.galley(
        egui::pos2(rect.center().x - date_size.x / 2.0, top + time_size.y + gap),
        date_galley,
        Style::TEXT_DIM,
    );
    response.clicked()
}

// ─────────────────────────────── the flyouts ─────────────────────────────────

/// One anchored flyout panel — the Foreground-order popup idiom every tray
/// flyout paints through: pivoted so its padded bottom-right corner hangs
/// just above the anchor cell (the same geometry the chevron well always
/// had), a SURFACE fill + hairline border expanded `SP_S` beyond the content
/// (the keyboard.rs overlay idiom), sized by the content itself. egui's
/// default screen constraint keeps it on a narrow display. Returns the area's
/// response for the caller's click-away dismissal.
fn flyout_panel(
    ctx: &egui::Context,
    id: &str,
    anchor: egui::Rect,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(egui::pos2(
            anchor.right() - Style::SP_S,
            anchor.top() - Style::SP_XS - Style::SP_S,
        ))
        .show(ctx, |ui| {
            // Reserve a slot so the panel background paints BEHIND the content
            // (the keyboard.rs overlay idiom).
            let bg = ui.painter().add(egui::Shape::Noop);
            let inner = ui.scope(content).response.rect.expand(Style::SP_S);
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(inner, Style::RADIUS, Style::SURFACE),
            );
            ui.painter().rect_stroke(
                inner,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
        })
        .response
}

/// Close the open flyout on a click-away — unless a row already routed (the
/// caller closes on route) or this frame's click is the one that just opened
/// it (that click lands outside the popup and would otherwise dismiss it in
/// the same frame).
fn close_on_click_away(
    state: &mut TrayState,
    response: &egui::Response,
    routed: bool,
    opened: bool,
) {
    if !routed && !opened && response.clicked_elsewhere() {
        state.open = OpenFlyout::None;
    }
}

/// The chevron's hidden-icon well (lock W10): a small grid of the hidden
/// Signal + Peers icons floated above the chevron. A click on a hidden icon
/// returns its routed surface; a click anywhere else closes the flyout.
fn chevron_flyout(
    ctx: &egui::Context,
    anchor: egui::Rect,
    inputs: &TrayInputs<'_>,
    opened: bool,
    state: &mut TrayState,
) -> Option<Surface> {
    let mut target = None;
    let response = flyout_panel(ctx, "w10-tray-flyout", anchor, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        ui.horizontal(|ui| {
            for item in HIDDEN {
                let view = icon_view(item, inputs);
                if icon_cell(ui, &view, egui::vec2(Style::SP_XL, Style::SP_XL)).clicked() {
                    target = Some(route(item));
                }
            }
        });
    });
    close_on_click_away(state, &response, target.is_some(), opened);
    target
}

/// The Chat micro-flyout (lock W7's exception): the real whole-mesh unread
/// tally — the same fold the badge counts — spelled out over an "Open Chat"
/// routing row. Recent message lines are honestly absent: only the folded
/// tally reaches the tray, the Chat surface carries the rooms.
fn chat_flyout(
    ctx: &egui::Context,
    anchor: egui::Rect,
    inputs: &TrayInputs<'_>,
    opened: bool,
    state: &mut TrayState,
) -> Option<Surface> {
    let mut target = None;
    let response = flyout_panel(ctx, "w10-tray-flyout-chat", anchor, |ui| {
        let header = chat_flyout_header(inputs.unread);
        if inputs.unread > 0 {
            ui.colored_label(Style::TEXT, RichText::new(header).size(Style::SMALL));
        } else {
            muted_note(ui, header);
        }
        ui.add_space(Style::SP_XS);
        if flyout_row(ui, Some(IconId::Chat), OPEN_CHAT, None).clicked() {
            target = Some(route(TrayItem::Chat));
        }
    });
    close_on_click_away(state, &response, target.is_some(), opened);
    target
}

/// The Volume micro-flyout (lock W7's exception): the master sink's name
/// line, then the Win10 volume row — the speaker glyph (the mute toggle,
/// showing the live muted/unmuted variant) beside a slider driving the REAL
/// master strip through the verb seam. An absent mixer renders the honest
/// not-available line + an "Open System" routing row instead of a dead
/// slider (§7); a refused write surfaces its typed error underneath.
fn volume_flyout(
    ctx: &egui::Context,
    anchor: egui::Rect,
    inputs: &TrayInputs<'_>,
    opened: bool,
    state: &mut TrayState,
    now: Instant,
) -> Option<Surface> {
    let mut target = None;
    let response = flyout_panel(ctx, "w10-tray-flyout-volume", anchor, |ui| {
        if let Some(master) = master_of(inputs.seat) {
            // The device/stream name line — the master sink's real name.
            muted_note(ui, master.name.as_str());
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                // The Win10 mute affordance: the speaker glyph at the
                // slider's left IS the toggle; its variant is the live
                // (echo-folded) mute state, so a confirmed flip shows here
                // and on the strip icon in the same frame.
                let glyph = if master.muted {
                    IconId::VolumeMuted
                } else {
                    IconId::Volume
                };
                if glyph_button(ui, glyph) {
                    drive_master_mute(
                        state.verbs.as_ref(),
                        &mut state.echoes,
                        &mut state.error,
                        &master.id,
                        !master.muted,
                        now,
                    );
                }
                // The slider renders the effective (echo-folded) level and
                // writes each change through the seam — the System mixer
                // faders' drive-on-change idiom.
                ui.spacing_mut().slider_width = Style::SP_XL.mul_add(-2.0, FLYOUT_W);
                let mut level = master.volume;
                if ui.add(egui::Slider::new(&mut level, 0..=100)).changed() {
                    drive_master_volume(
                        state.verbs.as_ref(),
                        &mut state.echoes,
                        &mut state.error,
                        &master.id,
                        level,
                        now,
                    );
                }
            });
            error_note(ui, state.error.as_deref());
        } else {
            muted_note(ui, MIXER_ABSENT);
            ui.add_space(Style::SP_XS);
            if flyout_row(ui, Some(IconId::Volume), OPEN_SYSTEM, None).clicked() {
                target = Some(route(TrayItem::Volume));
            }
        }
    });
    close_on_click_away(state, &response, target.is_some(), opened);
    target
}

/// The Bluetooth micro-flyout (lock W7's exception): the adapter radio power
/// row (click flips `Adapter1.Powered` on the first adapter — the same pick
/// the System panel's toggle makes), the real connected-device count, and an
/// "Open settings" row routing where the plain click used to (System owns
/// the full panel). An absent `BlueZ` renders the honest not-available line
/// instead of a dead toggle (§7).
fn bluetooth_flyout(
    ctx: &egui::Context,
    anchor: egui::Rect,
    inputs: &TrayInputs<'_>,
    opened: bool,
    state: &mut TrayState,
    now: Instant,
) -> Option<Surface> {
    let mut target = None;
    let response = flyout_panel(ctx, "w10-tray-flyout-bt", anchor, |ui| {
        let adapter = bt_of(inputs.seat).and_then(|bt| bt.adapters.first().map(|a| (bt, a)));
        if let Some((bt, adapter)) = adapter {
            let trailing = if adapter.powered { BT_ON } else { BT_OFF };
            if flyout_row(ui, Some(IconId::BluetoothSmall), BT_POWER, Some(trailing)).clicked() {
                drive_bt_power(
                    state.verbs.as_ref(),
                    &mut state.echoes,
                    &mut state.error,
                    &adapter.path,
                    !adapter.powered,
                    now,
                );
            }
            muted_note(ui, connected_line(bt.connected_devices()));
            error_note(ui, state.error.as_deref());
        } else {
            muted_note(ui, BT_ABSENT);
        }
        ui.add_space(Style::SP_XS);
        if flyout_row(ui, None, OPEN_SETTINGS, None).clicked() {
            target = Some(route(TrayItem::Bluetooth));
        }
    });
    close_on_click_away(state, &response, target.is_some(), opened);
    target
}

/// One compact flyout row (zebra-free): an optional 16px glyph, the SMALL
/// label, an optional dim trailing state ("On"/"Off"), hover fill only — no
/// tooltip (lock W6). Fixed [`FLYOUT_W`] so the panel reads as one column.
fn flyout_row(
    ui: &mut egui::Ui,
    glyph: Option<IconId>,
    label: &str,
    trailing: Option<&str>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(FLYOUT_W, ROW_H), egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let mut x = rect.left() + Style::SP_XS;
    if let Some(g) = glyph {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(x + TRAY_ICON / 2.0, rect.center().y),
            egui::vec2(TRAY_ICON, TRAY_ICON),
        );
        if let Some(tex) = icon_texture(ui.ctx(), g, TRAY_ICON, Style::TEXT_DIM) {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
                .paint_at(ui, icon_rect);
        }
        x += TRAY_ICON + Style::SP_S;
    }
    let font = FontId::proportional(Style::SMALL);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), Style::TEXT));
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        Style::TEXT,
    );
    if let Some(t) = trailing {
        let galley = ui.fonts(|f| f.layout_no_wrap(t.to_owned(), font, Style::TEXT_DIM));
        painter.galley(
            egui::pos2(
                rect.right() - Style::SP_XS - galley.size().x,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            Style::TEXT_DIM,
        );
    }
    response
}

/// A 16px glyph as a compact click target (the Volume flyout's mute
/// affordance): hover fill, no tooltip (lock W6), the glyph at the tray tint.
/// Returns `true` on a click.
fn glyph_button(ui: &mut egui::Ui, glyph: IconId) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(Style::SP_L, Style::SP_L), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if let Some(tex) = icon_texture(ui.ctx(), glyph, TRAY_ICON, Style::TEXT_DIM) {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(TRAY_ICON, TRAY_ICON));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    response.clicked()
}

/// The last refused verb's honest error line (§7) — paints nothing when the
/// verbs are all green.
fn error_note(ui: &mut egui::Ui, error: Option<&str>) {
    if let Some(e) = error {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, RichText::new(e).size(Style::SMALL));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        battery_fill_icon, battery_tone, charging, chat_badge, chat_dot, chat_flyout_header,
        clock_lines, connected_line, drive_bt_power, drive_master_mute, drive_master_volume,
        icon_view, peers_dot, route, signal_dot, status_dot, strip_items, system_pack, tray,
        volume_view, Echo, Echoes, OpenFlyout, TrayInputs, TrayItem, TrayState, TrayVerbs,
        ECHO_TTL, HIDDEN, ROW_H,
    };
    use crate::chrome::MeshSummary;
    use crate::dock::{Surface, TASKBAR_H};
    use mde_cosmic_applet::LighthouseHealth;
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_seat::{
        Backend, Battery, BatteryKind, BatteryState, BtAdapter, BtStatus, MixerStatus, MixerStrip,
        Probe, SeatError, SeatSnapshot, StripOrigin,
    };
    use mde_theme::brand::icons::IconId;
    use std::time::Instant;

    /// A typed-absent probe of any section (the honest build-host state).
    fn absent<T>() -> Probe<T> {
        Probe::Absent {
            backend: Backend::PipeWire,
            reason: "PipeWire is not available: test".to_string(),
        }
    }

    /// An all-absent seat snapshot the per-section fixtures override.
    fn seat() -> SeatSnapshot {
        SeatSnapshot {
            bluetooth: absent(),
            batteries: absent(),
            on_ac: absent(),
            power: absent(),
            power_profile: absent(),
            charge_limit: absent(),
            lid: absent(),
            displays: absent(),
            backlights: absent(),
            mixer: absent(),
            ddc: absent(),
        }
    }

    /// A master mixer strip at a chosen mute state.
    fn mixer(muted: bool) -> MixerStatus {
        MixerStatus {
            master: MixerStrip {
                id: "master".to_string(),
                name: "Master".to_string(),
                origin: StripOrigin::HostSession,
                volume: 40,
                muted,
            },
            strips: Vec::new(),
        }
    }

    /// One internal system pack at a chosen charge/state.
    fn pack(percentage: f64, state: BatteryState, power_supply: bool) -> Battery {
        Battery {
            model: "BAT0".to_string(),
            kind: BatteryKind::Internal,
            percentage,
            state,
            power_supply,
            time_to_empty: None,
            time_to_full: None,
            energy_rate: None,
        }
    }

    /// One Bluetooth adapter at a chosen radio power.
    fn adapter(powered: bool) -> BtAdapter {
        BtAdapter {
            path: "/org/bluez/hci0".into(),
            name: "hci0".into(),
            powered,
            discovering: false,
            discoverable: false,
            pairable: false,
        }
    }

    /// A seen mesh summary at a chosen presence/health shape.
    fn mesh(online: usize, total: usize, health: LighthouseHealth) -> MeshSummary {
        MeshSummary {
            peers_total: total,
            peers_online: online,
            health,
            seen: true,
        }
    }

    /// Inputs over a chosen mesh + seat (no unread, no session).
    fn inputs<'a>(mesh: &'a MeshSummary, seat: Option<&'a SeatSnapshot>) -> TrayInputs<'a> {
        TrayInputs {
            mesh,
            seat,
            unread: 0,
            session_active: false,
        }
    }

    // ── the battery fill ladder + bolt + dot (lock W8) ───────────────────────

    #[test]
    fn battery_fill_ladder_maps_charge_to_the_five_glyphs() {
        for (pct, icon) in [
            (0.0, IconId::BatteryEmpty),
            (12.0, IconId::BatteryEmpty),
            (12.5, IconId::BatteryQuarter),
            (25.0, IconId::BatteryQuarter),
            (37.5, IconId::BatteryHalf),
            (50.0, IconId::BatteryHalf),
            (62.5, IconId::BatteryThreeQuarter),
            (75.0, IconId::BatteryThreeQuarter),
            (87.5, IconId::BatteryFull),
            (100.0, IconId::BatteryFull),
        ] {
            assert_eq!(battery_fill_icon(pct), icon, "{pct}% → wrong fill glyph");
        }
    }

    #[test]
    fn the_bolt_overlays_only_while_taking_charge() {
        assert!(charging(BatteryState::Charging));
        assert!(charging(BatteryState::PendingCharge));
        assert!(!charging(BatteryState::Discharging));
        assert!(!charging(BatteryState::FullyCharged));
        assert!(!charging(BatteryState::Empty));
    }

    #[test]
    fn battery_dot_reads_amber_low_and_red_critical_only_while_draining() {
        // Charging / full → OK, regardless of charge.
        assert_eq!(
            battery_tone(&pack(9.0, BatteryState::Charging, true)),
            Style::OK
        );
        assert_eq!(
            battery_tone(&pack(100.0, BatteryState::FullyCharged, true)),
            Style::OK
        );
        // Draining: healthy → dim, low (<20) → WARN, critical (≤5) → DANGER.
        assert_eq!(
            battery_tone(&pack(72.0, BatteryState::Discharging, true)),
            Style::TEXT_DIM
        );
        assert_eq!(
            battery_tone(&pack(12.0, BatteryState::Discharging, true)),
            Style::WARN
        );
        assert_eq!(
            battery_tone(&pack(3.0, BatteryState::Discharging, true)),
            Style::DANGER
        );
        // An empty pack → DANGER too.
        assert_eq!(
            battery_tone(&pack(0.0, BatteryState::Empty, true)),
            Style::DANGER
        );
    }

    #[test]
    fn battery_view_folds_the_system_pack_over_a_fuller_peripheral() {
        // The `PowerSupply` cell is summarised even though a peripheral is
        // fuller — a mouse must not mask the low system pack.
        let cells = vec![
            pack(95.0, BatteryState::Discharging, false), // peripheral mouse
            pack(15.0, BatteryState::Discharging, true),  // the system pack, low
        ];
        let chosen = system_pack(&cells).expect("a pack is chosen");
        assert!((chosen.percentage - 15.0).abs() < f64::EPSILON);

        let mut s = seat();
        s.batteries = Probe::Present(cells);
        let m = MeshSummary::default();
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryQuarter, "15% → quarter fill");
        assert!(!view.bolt);
        assert_eq!(view.dot, Style::WARN);
    }

    #[test]
    fn battery_view_is_honestly_dim_when_absent_or_empty() {
        let m = MeshSummary::default();
        // No snapshot yet / absent backend / no pack — the dim empty outline.
        let view = icon_view(TrayItem::Battery, &inputs(&m, None));
        assert_eq!((view.glyph, view.bolt), (IconId::BatteryEmpty, false));
        assert_eq!(view.dot, Style::TEXT_DIM);
        let s = seat(); // batteries Absent
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryEmpty);
        let mut s = seat();
        s.batteries = Probe::Present(Vec::new()); // a desktop with no pack
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(
            (view.glyph, view.dot),
            (IconId::BatteryEmpty, Style::TEXT_DIM)
        );
    }

    #[test]
    fn a_charging_pack_carries_the_bolt_over_its_fill_glyph() {
        let mut s = seat();
        s.batteries = Probe::Present(vec![pack(80.0, BatteryState::Charging, true)]);
        let m = MeshSummary::default();
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryThreeQuarter);
        assert!(view.bolt, "a charging pack overlays the bolt");
        assert_eq!(view.dot, Style::OK);
    }

    // ── volume / bluetooth (seat folds) ──────────────────────────────────────

    #[test]
    fn volume_swaps_to_the_muted_glyph_with_a_warn_dot() {
        let mut s = seat();
        s.mixer = Probe::Present(mixer(true));
        assert_eq!(volume_view(Some(&s)), (IconId::VolumeMuted, Style::WARN));
        s.mixer = Probe::Present(mixer(false));
        assert_eq!(volume_view(Some(&s)), (IconId::Volume, Style::OK));
        // Absent mixer / pre-poll — the plain glyph, dim (never a fake level).
        assert_eq!(
            volume_view(Some(&seat())),
            (IconId::Volume, Style::TEXT_DIM)
        );
        assert_eq!(volume_view(None), (IconId::Volume, Style::TEXT_DIM));
    }

    #[test]
    fn bluetooth_dot_is_ok_only_while_an_adapter_is_powered() {
        let mut s = seat();
        s.bluetooth = Probe::Present(BtStatus {
            adapters: vec![adapter(true)],
            devices: Vec::new(),
        });
        let m = MeshSummary::default();
        assert_eq!(
            icon_view(TrayItem::Bluetooth, &inputs(&m, Some(&s))).dot,
            Style::OK
        );
        s.bluetooth = Probe::Present(BtStatus {
            adapters: vec![adapter(false)],
            devices: Vec::new(),
        });
        assert_eq!(
            icon_view(TrayItem::Bluetooth, &inputs(&m, Some(&s))).dot,
            Style::TEXT_DIM
        );
        // Absent / pre-poll → dim, and always the small Bluetooth rune.
        let view = icon_view(TrayItem::Bluetooth, &inputs(&m, None));
        assert_eq!(
            (view.glyph, view.dot),
            (IconId::BluetoothSmall, Style::TEXT_DIM)
        );
    }

    // ── the mesh dots (Peers / Status / Signal) ──────────────────────────────

    #[test]
    fn mesh_dots_fold_presence_and_health() {
        // All online + healthy → three OK dots.
        let up = mesh(3, 3, LighthouseHealth::AllHealthy);
        assert_eq!(peers_dot(&up), Style::OK);
        assert_eq!(status_dot(&up), Style::OK);
        assert_eq!(signal_dot(&up), Style::OK);

        // Some away → Peers amber; any peer up keeps Signal OK.
        let some = mesh(2, 3, LighthouseHealth::AllHealthy);
        assert_eq!(peers_dot(&some), Style::WARN);
        assert_eq!(signal_dot(&some), Style::OK);

        // Nobody answers a populated directory → Signal amber "isolated".
        let isolated = mesh(0, 3, LighthouseHealth::Degraded);
        assert_eq!(signal_dot(&isolated), Style::WARN);
        assert_eq!(status_dot(&isolated), Style::DANGER);

        // No lighthouses in view → a dim Status, never a fabricated verdict.
        assert_eq!(
            status_dot(&mesh(1, 1, LighthouseHealth::None)),
            Style::TEXT_DIM
        );

        // Unseen (pre-first-snapshot) → everything dim.
        let unseen = MeshSummary::default();
        assert_eq!(peers_dot(&unseen), Style::TEXT_DIM);
        assert_eq!(status_dot(&unseen), Style::TEXT_DIM);
        assert_eq!(signal_dot(&unseen), Style::TEXT_DIM);
    }

    // ── chat badge ───────────────────────────────────────────────────────────

    #[test]
    fn chat_badge_counts_and_caps_and_the_dot_covers_quiet() {
        assert_eq!(chat_badge(0), None);
        assert_eq!(chat_badge(7), Some("7".to_string()));
        assert_eq!(chat_badge(99), Some("99".to_string()));
        assert_eq!(chat_badge(240), Some("99+".to_string()));
        assert_eq!(chat_dot(0), Style::TEXT_DIM);
        assert_eq!(chat_dot(3), Style::ACCENT);

        let m = MeshSummary::default();
        let view = icon_view(
            TrayItem::Chat,
            &TrayInputs {
                mesh: &m,
                seat: None,
                unread: 120,
                session_active: false,
            },
        );
        assert_eq!(view.glyph, IconId::Chat);
        assert_eq!(view.badge.as_deref(), Some("99+"));
    }

    // ── the strip / hidden partition + Sessions transience (lock W10) ────────

    #[test]
    fn signal_and_peers_are_hidden_behind_the_chevron() {
        assert_eq!(HIDDEN, [TrayItem::Signal, TrayItem::Peers]);
        for item in HIDDEN {
            assert!(
                !strip_items(false).contains(&item) && !strip_items(true).contains(&item),
                "{item:?} must live in the flyout, not the strip"
            );
        }
    }

    #[test]
    fn sessions_is_transient_on_a_live_vdi_session() {
        // No session → the fixed five: Chat · BT · Volume · Battery · Status.
        assert_eq!(
            strip_items(false),
            [
                TrayItem::Chat,
                TrayItem::Bluetooth,
                TrayItem::Volume,
                TrayItem::Battery,
                TrayItem::Status,
            ]
        );
        // A live session leads with the Sessions icon; the rest is unchanged.
        assert_eq!(strip_items(true)[0], TrayItem::Sessions);
        assert_eq!(&strip_items(true)[1..], strip_items(false));
        // And it wears the Sessions glyph with an honest OK dot.
        let m = MeshSummary::default();
        let view = icon_view(
            TrayItem::Sessions,
            &TrayInputs {
                mesh: &m,
                seat: None,
                unread: 0,
                session_active: true,
            },
        );
        assert_eq!((view.glyph, view.dot), (IconId::Sessions, Style::OK));
    }

    // ── click routing (lock W7) ──────────────────────────────────────────────

    #[test]
    fn every_tray_icon_routes_to_its_owning_surface() {
        for (item, surface) in [
            (TrayItem::Sessions, Surface::Desktop),
            (TrayItem::Chat, Surface::Chat),
            (TrayItem::Bluetooth, Surface::System),
            (TrayItem::Volume, Surface::System),
            (TrayItem::Battery, Surface::System),
            (TrayItem::Status, Surface::MeshView),
            (TrayItem::Signal, Surface::MeshView),
            (TrayItem::Peers, Surface::MeshView),
        ] {
            assert_eq!(route(item), surface, "{item:?} → wrong surface");
        }
    }

    // ── the micro-flyouts (NAVBAR-W10-4) ─────────────────────────────────────

    /// A recording fake of the tray's narrow verb seam — every call is logged
    /// as one line; `fail` makes each verb answer a typed refusal.
    #[derive(Default)]
    struct FakeVerbs {
        fail: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeVerbs {
        fn failing() -> Self {
            Self {
                fail: true,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn took(&self) -> Vec<String> {
            self.calls.lock().expect("calls lock").clone()
        }

        fn record(&self, call: String) -> Result<(), SeatError> {
            self.calls.lock().expect("calls lock").push(call);
            if self.fail {
                Err(SeatError::Unavailable {
                    backend: Backend::PipeWire,
                    reason: "test refusal".to_owned(),
                })
            } else {
                Ok(())
            }
        }
    }

    impl TrayVerbs for FakeVerbs {
        fn set_master_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError> {
            self.record(format!("volume {strip_id} {volume}"))
        }

        fn set_master_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError> {
            self.record(format!("mute {strip_id} {muted}"))
        }

        fn set_bt_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
            self.record(format!("bt {adapter} {on}"))
        }
    }

    #[test]
    fn opening_one_flyout_closes_the_others() {
        // ONE flyout at a time (lock W7): opening any displaces whatever was
        // open, the chevron included; a second click on the owner closes it.
        let mut state = TrayState::default();
        assert!(state.toggle(OpenFlyout::Chat), "first click opens Chat");
        assert_eq!(state.open, OpenFlyout::Chat);
        assert!(state.toggle(OpenFlyout::Volume), "Volume displaces Chat");
        assert_eq!(state.open, OpenFlyout::Volume);
        assert!(
            state.toggle(OpenFlyout::Chevron),
            "the chevron participates"
        );
        assert_eq!(state.open, OpenFlyout::Chevron);
        assert!(
            !state.toggle(OpenFlyout::Chevron),
            "a second click on the owner closes"
        );
        assert_eq!(state.open, OpenFlyout::None);
    }

    #[test]
    fn the_volume_slider_drives_the_real_mixer_seam_and_echoes_on_ok() {
        let fake = FakeVerbs::default();
        let mut echoes = Echoes::default();
        let mut error = None;
        drive_master_volume(
            &fake,
            &mut echoes,
            &mut error,
            "node-42",
            70,
            Instant::now(),
        );
        assert_eq!(fake.took(), vec!["volume node-42 70".to_owned()]);
        assert!(error.is_none());
        let e = echoes.volume.as_ref().expect("a confirmed write echoes");
        assert_eq!((e.key.as_str(), e.value), ("node-42", 70));
    }

    #[test]
    fn a_refused_write_surfaces_the_error_and_echoes_nothing() {
        // §7 — a refusal never pretends: no echo, an honest typed message.
        let fake = FakeVerbs::failing();
        let mut echoes = Echoes::default();
        let mut error = None;
        drive_master_volume(
            &fake,
            &mut echoes,
            &mut error,
            "node-42",
            70,
            Instant::now(),
        );
        assert!(echoes.volume.is_none(), "a refusal must not echo");
        assert!(error.as_deref().is_some_and(|e| e.starts_with("volume:")));
        drive_bt_power(
            &fake,
            &mut echoes,
            &mut error,
            "/org/bluez/hci0",
            true,
            Instant::now(),
        );
        assert!(echoes.bt_powered.is_none());
        assert!(error
            .as_deref()
            .is_some_and(|e| e.starts_with("Bluetooth:")));
    }

    #[test]
    fn the_mute_toggle_flips_the_tray_icon_variant_through_the_echo() {
        // A live unmuted master in the (stale) snapshot…
        let mut s = seat();
        s.mixer = Probe::Present(mixer(false));
        let fake = FakeVerbs::default();
        let mut echoes = Echoes::default();
        let mut error = None;
        // …mute it through the seam: the confirmed echo folds over the stale
        // snapshot, so the muted glyph + WARN dot read back NOW, not at the
        // next 5s poll.
        drive_master_mute(
            &fake,
            &mut echoes,
            &mut error,
            "master",
            true,
            Instant::now(),
        );
        assert_eq!(fake.took(), vec!["mute master true".to_owned()]);
        let patched = echoes.overlay(Some(&s)).expect("an in-flight echo patches");
        assert_eq!(
            volume_view(Some(&patched)),
            (IconId::VolumeMuted, Style::WARN)
        );
        // And back to live.
        drive_master_mute(
            &fake,
            &mut echoes,
            &mut error,
            "master",
            false,
            Instant::now(),
        );
        let patched = echoes.overlay(Some(&s)).expect("the unmute echoes too");
        assert_eq!(volume_view(Some(&patched)), (IconId::Volume, Style::OK));
    }

    #[test]
    fn the_bt_power_toggle_drives_the_adapter_and_reads_back() {
        let mut s = seat();
        s.bluetooth = Probe::Present(BtStatus {
            adapters: vec![adapter(false)],
            devices: Vec::new(),
        });
        let fake = FakeVerbs::default();
        let mut echoes = Echoes::default();
        let mut error = None;
        drive_bt_power(
            &fake,
            &mut echoes,
            &mut error,
            "/org/bluez/hci0",
            true,
            Instant::now(),
        );
        assert_eq!(fake.took(), vec!["bt /org/bluez/hci0 true".to_owned()]);
        // The echo powers the adapter in the effective view → the strip's
        // Bluetooth dot reads OK this frame.
        let patched = echoes.overlay(Some(&s)).expect("the radio echo patches");
        let m = MeshSummary::default();
        assert_eq!(
            icon_view(TrayItem::Bluetooth, &inputs(&m, Some(&patched))).dot,
            Style::OK
        );
    }

    #[test]
    fn echoes_clear_when_the_snapshot_catches_up_or_the_ttl_passes() {
        let at = Instant::now();
        let echo = |value| {
            Some(Echo {
                key: "master".to_owned(),
                value,
                at,
            })
        };
        // A stale snapshot (volume 40) keeps the echo alive…
        let mut s = seat();
        s.mixer = Probe::Present(mixer(false));
        let mut echoes = Echoes {
            volume: echo(70u8),
            muted: None,
            bt_powered: None,
        };
        echoes.reconcile(Some(&s), at);
        assert!(echoes.volume.is_some(), "stale snapshot → echo survives");
        // …the caught-up poll clears it…
        let mut caught = mixer(false);
        caught.master.volume = 70;
        s.mixer = Probe::Present(caught);
        echoes.reconcile(Some(&s), at);
        assert!(echoes.volume.is_none(), "caught-up snapshot → echo drops");
        // …and a never-catching snapshot falls to the TTL (a concurrent
        // change from the System surface must not be overridden forever).
        let mut echoes = Echoes {
            volume: echo(70u8),
            muted: None,
            bt_powered: None,
        };
        s.mixer = Probe::Present(mixer(false));
        echoes.reconcile(Some(&s), at + ECHO_TTL);
        assert!(echoes.volume.is_none(), "expired echo → snapshot wins");
        // No echoes in flight → no snapshot clone at all.
        assert!(Echoes::default().overlay(Some(&s)).is_none());
    }

    #[test]
    fn the_chat_flyout_reports_the_real_unread_tally() {
        // Only the folded tally reaches the tray (recent message lines are
        // honestly absent — the Chat surface carries the rooms).
        assert_eq!(chat_flyout_header(0), "No unread messages");
        assert_eq!(chat_flyout_header(1), "1 unread message");
        assert_eq!(chat_flyout_header(42), "42 unread messages");
    }

    #[test]
    fn the_bt_flyout_counts_connected_devices_honestly() {
        assert_eq!(connected_line(0), "No devices connected");
        assert_eq!(connected_line(1), "1 device connected");
        assert_eq!(connected_line(3), "3 devices connected");
    }

    // ── the flyouts, mounted headless (the dock harness idiom) ───────────────

    /// Mount the tray alone on a bottom bar (the dock tests' headless
    /// `ctx.run` harness) over a present, unmuted mixer, and run one frame.
    fn run_tray(
        ctx: &egui::Context,
        state: &mut TrayState,
        active: &mut Surface,
        unread: usize,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mesh = MeshSummary::default();
        let mut s = seat();
        s.mixer = Probe::Present(mixer(false));
        let inputs = TrayInputs {
            mesh: &mesh,
            seat: Some(&s),
            unread,
            session_active: false,
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("tray-test-bar")
                .exact_height(TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        let _ = tray(ui, state, active, &inputs);
                    });
                });
        })
    }

    /// A primary-button press/release pair at `pos` (the egui click model:
    /// press one frame, release the next).
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn a_click_away_closes_the_open_flyout() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut state = TrayState::default();
        let mut active = Surface::default();
        state.open = OpenFlyout::Chat;
        // Prime two frames so the anchored area settles, then confirm the
        // quiet frames left it open.
        let _ = run_tray(&ctx, &mut state, &mut active, 3, Vec::new());
        let _ = run_tray(&ctx, &mut state, &mut active, 3, Vec::new());
        assert_eq!(state.open, OpenFlyout::Chat, "unclicked → stays open");
        // Click the empty body far from the flyout and the bar.
        let away = egui::pos2(200.0, 200.0);
        let _ = run_tray(
            &ctx,
            &mut state,
            &mut active,
            3,
            vec![egui::Event::PointerMoved(away), press(away, true)],
        );
        let _ = run_tray(&ctx, &mut state, &mut active, 3, vec![press(away, false)]);
        assert_eq!(state.open, OpenFlyout::None, "a click-away dismisses");
        assert_eq!(active, Surface::default(), "a dismissal routes nowhere");
    }

    #[test]
    fn the_chat_flyouts_open_row_routes_to_the_chat_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut state = TrayState::default();
        let mut active = Surface::About;
        state.open = OpenFlyout::Chat;
        let _ = run_tray(&ctx, &mut state, &mut active, 3, Vec::new());
        let _ = run_tray(&ctx, &mut state, &mut active, 3, Vec::new());
        let rect = ctx
            .memory(|m| m.area_rect(egui::Id::new("w10-tray-flyout-chat")))
            .expect("the chat flyout painted");
        // The Open Chat row is the panel's bottom row.
        let click = egui::pos2(rect.center().x, rect.bottom() - ROW_H / 2.0);
        let _ = run_tray(
            &ctx,
            &mut state,
            &mut active,
            3,
            vec![egui::Event::PointerMoved(click), press(click, true)],
        );
        let _ = run_tray(&ctx, &mut state, &mut active, 3, vec![press(click, false)]);
        assert_eq!(active, Surface::Chat, "the routing row switches surface");
        assert_eq!(state.open, OpenFlyout::None, "routing closes the flyout");
    }

    #[test]
    fn an_open_volume_flyout_paints_over_a_present_mixer() {
        // The name line + mute glyph + slider lay out headless without
        // panicking, and a quiet frame leaves the flyout open.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut state = TrayState::default();
        let mut active = Surface::default();
        state.open = OpenFlyout::Volume;
        let _ = run_tray(&ctx, &mut state, &mut active, 0, Vec::new());
        let _ = run_tray(&ctx, &mut state, &mut active, 0, Vec::new());
        assert!(
            ctx.memory(|m| m.area_rect(egui::Id::new("w10-tray-flyout-volume")))
                .is_some(),
            "the volume flyout painted"
        );
        assert_eq!(state.open, OpenFlyout::Volume);
    }

    // ── the clock (lock W11) ─────────────────────────────────────────────────

    #[test]
    fn clock_stacks_hh_mm_over_the_civil_date() {
        assert_eq!(
            clock_lines(0),
            ("00:00".to_string(), "1970-01-01".to_string())
        );
        // 2020-01-01T13:05:59Z.
        assert_eq!(
            clock_lines(1_577_836_800 + 13 * 3600 + 5 * 60 + 59),
            ("13:05".to_string(), "2020-01-01".to_string())
        );
    }
}
