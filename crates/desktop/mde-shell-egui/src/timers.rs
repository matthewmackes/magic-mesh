//! `Surface::Timers` — the **Timers & Alarms** surface (VDOCK-5, design
//! `docs/design/vertical-dock.md` locks #5/#16/#20).
//!
//! The vertical dock removed the taskbar clock; its replacement is a clock-glyph
//! status cell (`dock::clock_cell` — the live HH:MM *is* the glyph, lock #20)
//! that opens this surface: create + run **countdown timers** and set daily
//! **alarms**. The store lives on the [`Shell`](crate) and is ticked from the
//! shell's per-frame loop ([`TimersState::tick`], the POWER-5 `power_honor`
//! idiom), so a due timer/alarm fires **even while the surface is closed** — a
//! shell-side timer that survives surface switches (the design's "Timers
//! reliability" risk).
//!
//! **How a fired timer reaches the operator (glue §6, no new lane).** Firing
//! emits an alert-shaped JSON body on the **`event/notify/timer`** Bus lane —
//! one more source suffix under the CHAT-FIX-2 producer's `event/notify/`
//! prefix, which the `mackesd` chat worker already folds (its
//! `ALERT_LANE_PREFIXES` matches by prefix) into this host's `alert:<self>`
//! conversation: the Chat feed shows the card, and the Warning severity bumps
//! the dock's Chat unread badge / raises a chyron. No second lane, no new
//! render path. The lane is primed once per shell run with a benign Info
//! message (the `workers::notify` `prime_lanes` idiom) so the chat worker's
//! first-sight cursor skip absorbs the prime, never a real alarm.
//!
//! **One clock (§6).** All times fold through the crate's ONE calendar — the
//! same `unix → HH:MM` + [`crate::chat::civil_from_days`] fold the curtain's
//! giant clock face renders — so the dock glyph, this panel's clock, and the
//! alarm schedule can never disagree.
//!
//! **Honest degrade (§7).** No client data dir → no persistence and no Bus
//! emission (the solo/headless state), never a fake. Timers still run and
//! finish in-memory; the panel renders their real state.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

/// The Bus lane a fired timer/alarm rides — a `timer` source suffix under the
/// CHAT-FIX-2 producer's `event/notify/` prefix, so the existing chat-worker
/// fold (prefix-matched) carries it to the Chat feed + unread badge with no
/// daemon-side change.
const NOTIFY_TOPIC: &str = "event/notify/timer";

/// The `source` field the folded Chat card shows (the lane's suffix).
const NOTIFY_SOURCE: &str = "timer";

/// The persisted store file under the client data dir (the
/// `settings-appearance.json` / `power-honor.json` sibling).
const STORE_FILE: &str = "timers-alarms.json";
const START_TIMER_DISABLED_TIP: &str = "Set a duration first";
const TIMERS_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

/// Seconds per day — the alarm schedule's civil-day modulus.
const DAY_SECS: i64 = 86_400;

// ──────────────────────────── the one clock (§6) ────────────────────────────

/// Seconds since the Unix epoch (0 on a pre-epoch clock — the curtain's guard).
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for
/// crate-visible items in a private module — likewise the siblings below.)
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The wall-clock `HH:MM` for a Unix timestamp — the SAME fold the curtain's
/// giant clock face runs, restated tiny so the dock's clock glyph and this
/// panel read one clock (§6).
pub fn hhmm(unix_secs: i64) -> String {
    let tod = unix_secs.rem_euclid(DAY_SECS);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

/// Seconds until the NEXT minute rollover — the dock's clock glyph schedules
/// its repaint on this so the painted minute is never stale.
pub fn secs_to_next_minute(unix_secs: i64) -> u64 {
    let into = unix_secs.rem_euclid(60);
    u64::try_from(60 - into).unwrap_or(60)
}

/// A countdown rendered `H:MM:SS` (or `MM:SS` under an hour) — the kitchen-timer
/// reading.
fn fmt_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// The Unix timestamp of `HH:MM` on the civil day `day` (days since the epoch).
fn alarm_fire_ts(day: i64, hour: u8, minute: u8) -> i64 {
    day * DAY_SECS + i64::from(hour) * 3600 + i64::from(minute) * 60
}

/// The local hostname the notification's `host` field carries (routes the folded
/// card into this node's `alert:<self>` conversation) — the controller plane's
/// fallback ladder, restated rather than reached across surface modules.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ──────────────────────────── the persisted model ────────────────────────────

/// One countdown timer: its label, the preset duration, and the run state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TimerEntry {
    /// The operator's label (the notification headline names it).
    label: String,
    /// The preset countdown length in whole seconds.
    duration_secs: u64,
    /// Idle / Running / Paused / Finished (see [`TimerRun`]).
    #[serde(default)]
    run: TimerRun,
}

/// A timer's run state. `Running` persists the **absolute** deadline, so a
/// timer set before a shell restart stays honest across it — a deadline that
/// elapsed while the shell was down still rings (late, once) on the next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
enum TimerRun {
    /// Not started — the stored preset, restartable.
    #[default]
    Idle,
    /// Counting down toward the absolute Unix deadline.
    Running {
        /// When the countdown elapses (Unix seconds).
        deadline_unix: i64,
    },
    /// Paused with this much of the countdown left.
    Paused {
        /// The remaining countdown at pause time, in whole seconds.
        remaining_secs: u64,
    },
    /// The countdown elapsed and its notification was emitted.
    Finished {
        /// When it rang (Unix seconds) — the panel shows the honest time.
        rang_unix: i64,
    },
}

/// One daily alarm: fires at `HH:MM` (the shell's displayed wall clock) every
/// day it is enabled, edge-triggered once per civil day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AlarmEntry {
    /// The operator's label (the notification headline names it).
    label: String,
    /// Alarm hour, `0..=23` (the shell's displayed wall clock).
    hour: u8,
    /// Alarm minute, `0..=59`.
    minute: u8,
    /// Whether the alarm is armed — a disabled alarm keeps its slot silently.
    enabled: bool,
    /// The last civil day (days since the epoch) it fired — the once-per-day
    /// edge trigger, persisted so a shell restart can't re-ring today's alarm.
    #[serde(default)]
    last_fired_day: Option<i64>,
}

/// The persisted store — timers + alarms, one JSON file under the client data
/// dir (atomic temp + rename, the `power_honor` idiom).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct TimersFile {
    /// The countdown timers, in creation order.
    #[serde(default)]
    timers: Vec<TimerEntry>,
    /// The daily alarms, in creation order.
    #[serde(default)]
    alarms: Vec<AlarmEntry>,
}

impl TimersFile {
    /// Load from `path`, honestly folding a missing / half-written / malformed
    /// file to the empty store (never a fatal, never a fabricated entry).
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write to `path` (atomic temp + rename, like the mesh peer/prefs records).
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Baseline-arm one alarm at `now`: if its `HH:MM` already passed **today**, mark
/// today fired so it arms for tomorrow instead of ringing instantly — the notify
/// worker's "first sight seeds the baseline silently" rule, applied at load, at
/// creation, and at re-enable (setting a 07:00 alarm at 09:00 means *tomorrow*).
fn baseline_arm(alarm: &mut AlarmEntry, now: i64) {
    let today = now.div_euclid(DAY_SECS);
    if now >= alarm_fire_ts(today, alarm.hour, alarm.minute)
        && alarm.last_fired_day.is_none_or(|d| d < today)
    {
        alarm.last_fired_day = Some(today);
    }
}

// ──────────────────────────── the shell-side state ────────────────────────────

/// The Timers & Alarms store + its Bus/persistence seams. Owned by the `Shell`
/// (NOT the panel) and ticked every frame from `render`, so firing never
/// depends on the surface being open.
pub struct TimersState {
    /// The live store (persisted on every mutation + firing).
    file: TimersFile,
    /// The store path (`<client-data-dir>/timers-alarms.json`); `None` headless.
    store: Option<PathBuf>,
    /// The client Bus root the notify lane is written to; `None` headless.
    bus_root: Option<PathBuf>,
    /// The `host` field the notification carries (routes to `alert:<self>`).
    self_host: String,
    /// Whether the notify lane was primed this shell run (once).
    primed: bool,
    /// Draft: the new-timer label field.
    draft_timer_label: String,
    /// Draft: the new-timer hours / minutes / seconds spinners.
    draft_timer_hms: (u32, u32, u32),
    /// Draft: the new-alarm label field.
    draft_alarm_label: String,
    /// Draft: the new-alarm hour / minute spinners.
    draft_alarm_hm: (u32, u32),
}

impl Default for TimersState {
    fn default() -> Self {
        let root = mde_bus::client_data_dir();
        let store = root.clone().map(|d| d.join(STORE_FILE));
        Self::with_roots(root, store, local_hostname())
    }
}

impl TimersState {
    /// Build over explicit roots (the testable constructor `Default` folds to):
    /// load the store, then baseline-arm every alarm so nothing past-due rings
    /// at boot (silently seeded, the notify-worker idiom).
    fn with_roots(bus_root: Option<PathBuf>, store: Option<PathBuf>, self_host: String) -> Self {
        let mut file = store
            .as_deref()
            .map_or_else(TimersFile::default, TimersFile::load_from);
        let now = now_unix();
        for alarm in &mut file.alarms {
            baseline_arm(alarm, now);
        }
        Self {
            file,
            store,
            bus_root,
            self_host,
            primed: false,
            draft_timer_label: String::new(),
            draft_timer_hms: (0, 5, 0),
            draft_alarm_label: String::new(),
            draft_alarm_hm: (7, 0),
        }
    }

    /// The shell's per-frame hook (the POWER-5 two-line idiom): evaluate every
    /// running timer + armed alarm against the wall clock, emit each firing on
    /// the notify lane, and self-schedule the next wakeup — so a due alarm
    /// fires without input, on the idle DRM seat, with the surface closed.
    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        let now = now_unix();
        if self.tick_at(now) > 0 {
            ctx.request_repaint();
        }
        if let Some(secs) = self.secs_to_next_event(now) {
            ctx.request_repaint_after(Duration::from_secs(secs.max(1)));
        }
    }

    /// One evaluation pass at an explicit `now` (the unit-testable core).
    /// Returns how many timers/alarms fired; each firing emitted its
    /// notification and the mutated store was persisted.
    fn tick_at(&mut self, now: i64) -> usize {
        self.prime_lane_once(now);
        let mut fired: Vec<String> = Vec::new();
        for timer in &mut self.file.timers {
            if let TimerRun::Running { deadline_unix } = timer.run {
                if now >= deadline_unix {
                    timer.run = TimerRun::Finished { rang_unix: now };
                    fired.push(format!(
                        "Timer \u{201c}{}\u{201d} finished ({})",
                        timer.label,
                        fmt_duration(timer.duration_secs)
                    ));
                }
            }
        }
        let today = now.div_euclid(DAY_SECS);
        for alarm in &mut self.file.alarms {
            if alarm.enabled
                && alarm.last_fired_day.is_none_or(|d| d < today)
                && now >= alarm_fire_ts(today, alarm.hour, alarm.minute)
            {
                alarm.last_fired_day = Some(today);
                fired.push(format!(
                    "Alarm \u{201c}{}\u{201d} \u{2014} {:02}:{:02}",
                    alarm.label, alarm.hour, alarm.minute
                ));
            }
        }
        for summary in &fired {
            // Warning severity: the chat fold bumps the unread badge / raises a
            // chyron for Warning+ — a ringing timer must be noticed.
            self.write_lane("warning", summary, now);
        }
        if !fired.is_empty() {
            self.persist();
        }
        fired.len()
    }

    /// Seconds until the earliest upcoming deadline/alarm (`None` when nothing
    /// is armed) — the tick's self-scheduled wakeup.
    fn secs_to_next_event(&self, now: i64) -> Option<u64> {
        let today = now.div_euclid(DAY_SECS);
        let timers = self.file.timers.iter().filter_map(|t| match t.run {
            TimerRun::Running { deadline_unix } => Some(deadline_unix),
            _ => None,
        });
        let alarms = self.file.alarms.iter().filter(|a| a.enabled).map(|a| {
            if a.last_fired_day.is_some_and(|d| d >= today) {
                alarm_fire_ts(today + 1, a.hour, a.minute)
            } else {
                alarm_fire_ts(today, a.hour, a.minute)
            }
        });
        timers
            .chain(alarms)
            .min()
            .map(|ts| u64::try_from(ts.saturating_sub(now)).unwrap_or(0))
    }

    /// Prime the notify lane once per shell run with a benign Info message —
    /// the `workers::notify::prime_lanes` rule: the chat worker seeds a
    /// first-seen topic's cursor at head and skips that first message, so the
    /// prime absorbs the skip and every real firing thereafter is folded.
    fn prime_lane_once(&mut self, now: i64) {
        if self.primed || self.bus_root.is_none() {
            return;
        }
        self.primed = true;
        self.write_lane("info", "timers online", now);
    }

    /// Serialize + publish one alert-shaped body on the notify lane — the exact
    /// field set `workers::notify` emits (`fold_alert` classifies `severity`,
    /// `host` routes to `alert:<self>`). Best-effort: a missing Bus dir is the
    /// honest solo-host no-op (§7), never a panic.
    fn write_lane(&self, severity: &str, summary: &str, now: i64) {
        let Some(root) = self.bus_root.as_ref() else {
            return;
        };
        // arch-11: writer (publishes an alert) — kept on Persist::open; the shared
        // BusReader seam is read-only.
        let Ok(persist) = Persist::open(root.clone()) else {
            return;
        };
        let body = serde_json::json!({
            "severity": severity,
            "source": NOTIFY_SOURCE,
            "summary": summary,
            "host": self.self_host,
            "ts_unix_ms": now.saturating_mul(1000),
        })
        .to_string();
        let _ = persist.write(NOTIFY_TOPIC, Priority::Default, None, Some(&body));
    }

    /// Persist the store (a silent no-op headless — no data dir, §7).
    fn persist(&self) {
        if let Some(path) = self.store.as_deref() {
            let _ = self.file.save_to(path);
        }
    }

    // ── panel-driven mutations (each persists) ──────────────────────────────

    /// Create a timer from the draft row and start it counting immediately.
    /// A zero draft duration is refused by the panel (the button disables).
    fn start_draft_timer(&mut self, now: i64) {
        let (h, m, s) = self.draft_timer_hms;
        let duration_secs = u64::from(h) * 3600 + u64::from(m) * 60 + u64::from(s);
        if duration_secs == 0 {
            return;
        }
        let trimmed = self.draft_timer_label.trim();
        let label = if trimmed.is_empty() {
            "Timer".to_string()
        } else {
            trimmed.to_string()
        };
        self.file.timers.push(TimerEntry {
            label,
            duration_secs,
            run: TimerRun::Running {
                deadline_unix: now + i64::try_from(duration_secs).unwrap_or(i64::MAX),
            },
        });
        self.draft_timer_label.clear();
        self.persist();
    }

    /// Create an alarm from the draft row, baseline-armed (a time already past
    /// today arms for tomorrow — never an instant ring).
    fn add_draft_alarm(&mut self, now: i64) {
        let (h, m) = self.draft_alarm_hm;
        let trimmed = self.draft_alarm_label.trim();
        let label = if trimmed.is_empty() {
            "Alarm".to_string()
        } else {
            trimmed.to_string()
        };
        let mut alarm = AlarmEntry {
            label,
            hour: u8::try_from(h.min(23)).unwrap_or(23),
            minute: u8::try_from(m.min(59)).unwrap_or(59),
            enabled: true,
            last_fired_day: None,
        };
        baseline_arm(&mut alarm, now);
        self.file.alarms.push(alarm);
        self.draft_alarm_label.clear();
        self.persist();
    }

    /// Apply one row action from the panel (collected, then applied — the
    /// borrow-friendly render idiom), then persist.
    fn apply(&mut self, action: RowAction, now: i64) {
        match action {
            RowAction::TimerStart(i) => {
                if let Some(t) = self.file.timers.get_mut(i) {
                    t.run = TimerRun::Running {
                        deadline_unix: now + i64::try_from(t.duration_secs).unwrap_or(i64::MAX),
                    };
                }
            }
            RowAction::TimerPause(i) => {
                if let Some(t) = self.file.timers.get_mut(i) {
                    if let TimerRun::Running { deadline_unix } = t.run {
                        t.run = TimerRun::Paused {
                            remaining_secs: u64::try_from(deadline_unix.saturating_sub(now))
                                .unwrap_or(0),
                        };
                    }
                }
            }
            RowAction::TimerResume(i) => {
                if let Some(t) = self.file.timers.get_mut(i) {
                    if let TimerRun::Paused { remaining_secs } = t.run {
                        t.run = TimerRun::Running {
                            deadline_unix: now + i64::try_from(remaining_secs).unwrap_or(i64::MAX),
                        };
                    }
                }
            }
            RowAction::TimerReset(i) => {
                if let Some(t) = self.file.timers.get_mut(i) {
                    t.run = TimerRun::Idle;
                }
            }
            RowAction::TimerRemove(i) => {
                if i < self.file.timers.len() {
                    self.file.timers.remove(i);
                }
            }
            RowAction::AlarmToggle(i, on) => {
                if let Some(a) = self.file.alarms.get_mut(i) {
                    a.enabled = on;
                    if on {
                        // Re-enabling mid-day must not ring instantly for a time
                        // already past — the same baseline rule as creation.
                        baseline_arm(a, now);
                    }
                }
            }
            RowAction::AlarmRemove(i) => {
                if i < self.file.alarms.len() {
                    self.file.alarms.remove(i);
                }
            }
        }
        self.persist();
    }
}

/// One deferred row action the render loop collects and applies after the
/// iteration borrow ends.
#[derive(Debug, Clone, Copy)]
enum RowAction {
    /// (Re)start timer `i` from its full preset.
    TimerStart(usize),
    /// Pause running timer `i`, keeping its remaining time.
    TimerPause(usize),
    /// Resume paused timer `i` from its remaining time.
    TimerResume(usize),
    /// Reset timer `i` back to its idle preset.
    TimerReset(usize),
    /// Delete timer `i`.
    TimerRemove(usize),
    /// Enable/disable alarm `i`.
    AlarmToggle(usize, bool),
    /// Delete alarm `i`.
    AlarmRemove(usize),
}

// ──────────────────────────── the panel ────────────────────────────

/// Render the Timers & Alarms surface into `ui`. A pure renderer over the
/// shell-owned [`TimersState`] — the *shell* ticks the store every frame, so
/// nothing here is load-bearing for firing (§7: closing the surface never
/// silences an alarm).
pub fn timers_panel(ui: &mut egui::Ui, state: &mut TimersState) {
    let now = now_unix();
    clock_header(ui, now);

    let mut action: Option<RowAction> = None;
    let any_running = timers_section(ui, state, now, &mut action);
    ui.add_space(Style::SP_M);
    ui.separator();
    alarms_section(ui, state, now, &mut action);

    ui.add_space(Style::SP_M);
    ui.label(
        RichText::new(
            "Firings post to the Chat feed (the event/notify lane) and ring even while \
             this surface is closed — the shell keeps the countdown.",
        )
        .size(Style::SMALL)
        .color(Style::TEXT_DIM),
    );

    if let Some(act) = action {
        state.apply(act, now);
    }
    // A visible running countdown re-renders each second (the shell's tick
    // schedules the *firing* wakeups; this is only display smoothness).
    if any_running {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
}

/// The live clock header — the same one-clock `HH:MM` + civil-date fold as the
/// curtain face (§6), display-sized.
fn clock_header(ui: &mut egui::Ui, now: i64) {
    let (year, month, day) = crate::chat::civil_from_days(now.div_euclid(DAY_SECS));
    ui.add_space(Style::SP_M);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(hhmm(now))
                .size(Style::DISPLAY)
                .color(Style::TEXT_STRONG),
        );
        ui.label(
            RichText::new(format!("{year:04}-{month:02}-{day:02}"))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
    ui.add_space(Style::SP_M);
    ui.separator();
}

/// The **Timers** section: the draft row (label + H/M/S + Start) and one row
/// per timer with its state-true controls. Returns whether any timer is
/// visibly counting (the caller keeps a 1 Hz repaint for display smoothness).
fn timers_section(
    ui: &mut egui::Ui,
    state: &mut TimersState,
    now: i64,
    action: &mut Option<RowAction>,
) -> bool {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Timers")
            .size(Style::TITLE)
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.draft_timer_label)
                .hint_text("Timer label")
                .desired_width(160.0),
        );
        let (h, m, s) = &mut state.draft_timer_hms;
        ui.add(egui::DragValue::new(h).range(0..=99).suffix("h"));
        ui.add(egui::DragValue::new(m).range(0..=59).suffix("m"));
        ui.add(egui::DragValue::new(s).range(0..=59).suffix("s"));
        let zero = *h == 0 && *m == 0 && *s == 0;
        let start_response = ui.add_enabled(!zero, egui::Button::new("Start"));
        if timers_disabled_hover_text(start_response, START_TIMER_DISABLED_TIP).clicked() {
            state.start_draft_timer(now);
        }
    });
    ui.add_space(Style::SP_S);
    if state.file.timers.is_empty() {
        ui.label(
            RichText::new("No timers yet — set a duration and press Start.")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
    let mut any_running = false;
    for (i, timer) in state.file.timers.iter().enumerate() {
        ui.push_id(("timer-row", i), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&timer.label).color(Style::TEXT));
                if matches!(timer.run, TimerRun::Running { .. }) {
                    any_running = true;
                }
                timer_row_controls(ui, i, timer, now, action);
            });
        });
    }
    any_running
}

fn timers_tooltip(ui: &mut egui::Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let surface = Style::resolve_color(&ctx, Style::SURFACE);
    let border = Style::resolve_color(&ctx, Style::BORDER);
    let text_color = Style::resolve_color(&ctx, Style::TEXT);
    egui::Frame::NONE
        .fill(surface)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(TIMERS_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(text_color)).wrap(),
            );
        });
}

fn timers_disabled_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_disabled_hover_ui(move |ui| timers_tooltip(ui, text.as_str()))
}

/// One timer row's state line + buttons — each run state shows its honest
/// reading and only the actions that are real for it.
fn timer_row_controls(
    ui: &mut egui::Ui,
    i: usize,
    timer: &TimerEntry,
    now: i64,
    action: &mut Option<RowAction>,
) {
    match timer.run {
        TimerRun::Running { deadline_unix } => {
            let left = u64::try_from(deadline_unix.saturating_sub(now)).unwrap_or(0);
            ui.label(
                RichText::new(fmt_duration(left))
                    .color(Style::ACCENT)
                    .strong(),
            );
            if ui.button("Pause").clicked() {
                *action = Some(RowAction::TimerPause(i));
            }
            if ui.button("Cancel").clicked() {
                *action = Some(RowAction::TimerReset(i));
            }
        }
        TimerRun::Paused { remaining_secs } => {
            ui.label(
                RichText::new(format!("{} (paused)", fmt_duration(remaining_secs)))
                    .color(Style::TEXT_DIM),
            );
            if ui.button("Resume").clicked() {
                *action = Some(RowAction::TimerResume(i));
            }
            if ui.button("Cancel").clicked() {
                *action = Some(RowAction::TimerReset(i));
            }
        }
        TimerRun::Idle => {
            ui.label(RichText::new(fmt_duration(timer.duration_secs)).color(Style::TEXT_DIM));
            if ui.button("Start").clicked() {
                *action = Some(RowAction::TimerStart(i));
            }
            if ui.button("Remove").clicked() {
                *action = Some(RowAction::TimerRemove(i));
            }
        }
        TimerRun::Finished { rang_unix } => {
            ui.label(RichText::new(format!("rang at {}", hhmm(rang_unix))).color(Style::WARN));
            if ui.button("Restart").clicked() {
                *action = Some(RowAction::TimerStart(i));
            }
            if ui.button("Remove").clicked() {
                *action = Some(RowAction::TimerRemove(i));
            }
        }
    }
}

/// The **Alarms** section: the draft row (label + HH:MM + Set) and one row per
/// alarm — enable toggle, time, label, the daily note, and the honest
/// "rang today" marker.
fn alarms_section(
    ui: &mut egui::Ui,
    state: &mut TimersState,
    now: i64,
    action: &mut Option<RowAction>,
) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Alarms")
            .size(Style::TITLE)
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.draft_alarm_label)
                .hint_text("Alarm label")
                .desired_width(160.0),
        );
        let (h, m) = &mut state.draft_alarm_hm;
        ui.add(egui::DragValue::new(h).range(0..=23));
        ui.label(RichText::new(":").color(Style::TEXT_DIM));
        ui.add(egui::DragValue::new(m).range(0..=59));
        if ui.button("Set alarm").clicked() {
            state.add_draft_alarm(now);
        }
    });
    ui.add_space(Style::SP_S);
    if state.file.alarms.is_empty() {
        ui.label(
            RichText::new("No alarms set.")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
    let today = now.div_euclid(DAY_SECS);
    for (i, alarm) in state.file.alarms.iter().enumerate() {
        ui.push_id(("alarm-row", i), |ui| {
            ui.horizontal(|ui| {
                let mut on = alarm.enabled;
                if ui.checkbox(&mut on, "").changed() {
                    *action = Some(RowAction::AlarmToggle(i, on));
                }
                let tone = if alarm.enabled {
                    Style::TEXT
                } else {
                    Style::TEXT_DIM
                };
                ui.label(
                    RichText::new(format!("{:02}:{:02}", alarm.hour, alarm.minute))
                        .color(tone)
                        .strong(),
                );
                ui.label(RichText::new(&alarm.label).color(tone));
                ui.label(
                    RichText::new("daily")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                if alarm.last_fired_day == Some(today) {
                    ui.label(
                        RichText::new("rang today")
                            .size(Style::SMALL)
                            .color(Style::WARN),
                    );
                }
                if ui.button("Remove").clicked() {
                    *action = Some(RowAction::AlarmRemove(i));
                }
            });
        });
    }
}

// ──────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        alarm_fire_ts, baseline_arm, fmt_duration, hhmm, secs_to_next_minute, timers_panel,
        AlarmEntry, TimerEntry, TimerRun, TimersFile, TimersState, DAY_SECS, NOTIFY_TOPIC,
    };
    use mde_bus::persist::Persist;
    use mde_egui::egui;
    use mde_egui::{Density, Style, StyleColorScheme};
    use std::path::PathBuf;

    /// A fresh per-test scratch dir (no tempdir dev-dep in this crate) —
    /// removed best-effort on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mde-vdock5-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A state over a real Bus root (notifications land in a real `Persist`)
    /// with no store file — the emission harness.
    fn state_with_bus(root: &std::path::Path) -> TimersState {
        TimersState::with_roots(Some(root.to_path_buf()), None, "testhost".to_string())
    }

    fn lane_bodies(root: &std::path::Path) -> Vec<String> {
        let persist = Persist::open(root.to_path_buf()).expect("open persist");
        persist
            .list_since(NOTIFY_TOPIC, None)
            .expect("list lane")
            .into_iter()
            .filter_map(|m| m.body)
            .collect()
    }

    fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)))
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    if rect.fill != egui::Color32::TRANSPARENT {
                        out.push(rect.fill);
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render_timers_tooltip_frame(ctx: &egui::Context) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 96.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::timers_tooltip(ui, super::START_TIMER_DISABLED_TIP);
                });
            },
        )
    }

    #[test]
    fn the_clock_folds_read_sanely() {
        // 2026-07-05 12:34:56 UTC-ish fold points, plus the epoch edge.
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(12 * 3600 + 34 * 60 + 56), "12:34");
        assert_eq!(hhmm(-1), "23:59", "pre-epoch folds through rem_euclid");
        assert_eq!(secs_to_next_minute(0), 60);
        assert_eq!(secs_to_next_minute(59), 1);
        assert_eq!(fmt_duration(0), "00:00");
        assert_eq!(fmt_duration(65), "01:05");
        assert_eq!(fmt_duration(3600 + 62), "1:01:02");
    }

    #[test]
    fn a_due_timer_fires_one_warning_notification_on_the_notify_lane() {
        // The unit's acceptance: a timer that elapses emits the alert-shaped
        // body on `event/notify/timer` (the CHAT-FIX-2 lane the chat worker
        // folds), exactly once, and flips to Finished.
        let scratch = Scratch::new("timer-fires");
        let mut s = state_with_bus(&scratch.0);
        let now = 1_800_000_000_i64;
        s.file.timers.push(TimerEntry {
            label: "tea".to_string(),
            duration_secs: 300,
            run: TimerRun::Running {
                deadline_unix: now + 300,
            },
        });

        // Before the deadline: only the benign prime is on the lane.
        assert_eq!(s.tick_at(now), 0, "nothing due yet");
        let primed = lane_bodies(&scratch.0);
        assert_eq!(primed.len(), 1, "the prime alone rides the lane pre-fire");
        assert!(
            primed[0].contains("\"severity\":\"info\""),
            "the prime is Info"
        );

        // At/past the deadline: exactly one Warning firing, alert-shaped.
        assert_eq!(s.tick_at(now + 300), 1, "the due timer fires");
        let bodies = lane_bodies(&scratch.0);
        assert_eq!(bodies.len(), 2, "prime + one firing");
        let fired: serde_json::Value =
            serde_json::from_str(&bodies[1]).expect("alert-shaped JSON body");
        assert_eq!(fired["severity"], "warning", "a ring must bump the badge");
        assert_eq!(fired["source"], "timer");
        assert_eq!(fired["host"], "testhost", "routes to alert:<self>");
        assert!(
            fired["summary"].as_str().unwrap_or("").contains("tea"),
            "the summary names the timer"
        );
        assert!(fired["ts_unix_ms"].as_i64().unwrap_or(0) > 0);
        assert!(
            matches!(s.file.timers[0].run, TimerRun::Finished { .. }),
            "the timer lands Finished"
        );

        // Edge-triggered: a later tick must NOT re-fire the finished timer.
        assert_eq!(s.tick_at(now + 400), 0, "no re-fire");
        assert_eq!(lane_bodies(&scratch.0).len(), 2, "the lane stays at two");
    }

    #[test]
    fn an_alarm_fires_once_per_day_and_rearms_tomorrow() {
        let scratch = Scratch::new("alarm-daily");
        let mut s = state_with_bus(&scratch.0);
        // Day-aligned base so the H:M arithmetic below is exact.
        let day0 = 20_000_i64;
        let base = day0 * DAY_SECS;
        s.file.alarms.push(AlarmEntry {
            label: "standup".to_string(),
            hour: 9,
            minute: 30,
            enabled: true,
            last_fired_day: None,
        });
        let ring = alarm_fire_ts(day0, 9, 30);
        assert_eq!(s.tick_at(base + 3600), 0, "09:30 not reached at 01:00");
        assert_eq!(s.tick_at(ring), 1, "fires at 09:30");
        assert_eq!(s.tick_at(ring + 60), 0, "once per day (edge-triggered)");
        assert_eq!(s.tick_at(ring + DAY_SECS), 1, "re-arms the next day");
        // A disabled alarm never fires.
        s.file.alarms[0].enabled = false;
        assert_eq!(s.tick_at(ring + 2 * DAY_SECS), 0, "disabled = silent");
    }

    #[test]
    fn boot_seeds_past_due_alarms_silently_and_the_store_round_trips() {
        // An alarm whose time already passed must NOT ring at construction
        // (the notify worker's silent first-sight baseline), and the atomic
        // store write must read back byte-identical state.
        let scratch = Scratch::new("baseline");
        let store = scratch.0.join("timers-alarms.json");
        let file = TimersFile {
            timers: vec![TimerEntry {
                label: "idle".to_string(),
                duration_secs: 60,
                run: TimerRun::Idle,
            }],
            alarms: vec![AlarmEntry {
                label: "early".to_string(),
                hour: 0,
                minute: 0,
                enabled: true,
                last_fired_day: None,
            }],
        };
        file.save_to(&store).expect("atomic save");
        assert_eq!(TimersFile::load_from(&store), file, "round trip");

        let mut s =
            TimersState::with_roots(Some(scratch.0.clone()), Some(store), "testhost".to_string());
        // The 00:00 alarm passed hours ago today — construction baselined it.
        let now = super::now_unix();
        assert_eq!(s.tick_at(now), 0, "no boot-time ring for a passed alarm");
        let bodies = lane_bodies(&scratch.0);
        assert_eq!(bodies.len(), 1, "only the prime — nothing rang");
        // A malformed store folds honestly to empty.
        let bad = scratch.0.join("garbage.json");
        std::fs::write(&bad, "{not json").expect("write garbage");
        assert_eq!(TimersFile::load_from(&bad), TimersFile::default());
    }

    #[test]
    fn disabled_start_tooltip_uses_themed_text_and_surface_in_light_mode() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::Light, Density::Mouse);
        let out = render_timers_tooltip_frame(&ctx);
        let text_color = Style::resolve_color(&ctx, Style::TEXT);
        let surface = Style::resolve_color(&ctx, Style::SURFACE);

        let texts = painted_text_colors(&out.shapes);
        assert!(
            texts.iter().any(|(text, color)| {
                text == super::START_TIMER_DISABLED_TIP && *color == text_color
            }),
            "Timers disabled tooltip should paint themed text: {texts:?}"
        );
        assert!(
            text_color != surface,
            "Timers disabled tooltip text and surface must stay distinct in light mode"
        );

        let fills = painted_fill_colors(&out.shapes);
        assert!(
            fills.contains(&surface),
            "Timers disabled tooltip should paint its themed surface: {fills:?}"
        );
    }

    #[test]
    fn baseline_arm_only_marks_times_already_past_today() {
        let day = 10_000_i64;
        let mut early = AlarmEntry {
            label: "a".into(),
            hour: 1,
            minute: 0,
            enabled: true,
            last_fired_day: None,
        };
        // Now = 02:00 — the 01:00 alarm is past: seeded fired-today.
        baseline_arm(&mut early, day * DAY_SECS + 2 * 3600);
        assert_eq!(early.last_fired_day, Some(day));
        let mut late = AlarmEntry {
            label: "b".into(),
            hour: 23,
            minute: 0,
            enabled: true,
            last_fired_day: None,
        };
        // Now = 02:00 — the 23:00 alarm is still ahead: left armed for today.
        baseline_arm(&mut late, day * DAY_SECS + 2 * 3600);
        assert_eq!(late.last_fired_day, None);
    }

    #[test]
    fn the_tick_schedules_the_earliest_upcoming_wakeup() {
        let scratch = Scratch::new("wakeup");
        let mut s = state_with_bus(&scratch.0);
        let day0 = 20_000_i64;
        let now = day0 * DAY_SECS + 8 * 3600; // 08:00
        assert_eq!(s.secs_to_next_event(now), None, "nothing armed → no wake");
        s.file.alarms.push(AlarmEntry {
            label: "nine".into(),
            hour: 9,
            minute: 0,
            enabled: true,
            last_fired_day: None,
        });
        assert_eq!(
            s.secs_to_next_event(now),
            Some(3600),
            "09:00 is an hour out"
        );
        s.file.timers.push(TimerEntry {
            label: "soon".into(),
            duration_secs: 120,
            run: TimerRun::Running {
                deadline_unix: now + 120,
            },
        });
        assert_eq!(s.secs_to_next_event(now), Some(120), "the timer is sooner");
        // Once today's alarm fired, tomorrow's slot is the alarm candidate.
        s.file.alarms[0].last_fired_day = Some(day0);
        s.file.timers.clear();
        assert_eq!(
            s.secs_to_next_event(now + 2 * 3600),
            Some(23 * 3600),
            "fired-today re-arms for tomorrow 09:00"
        );
    }

    #[test]
    fn the_panel_renders_headless_over_real_state() {
        // The same Context::run → tessellate path the other surface panels
        // prove reachability with — real state (one of each run state), no
        // demo data, non-empty primitives.
        let scratch = Scratch::new("panel");
        let mut s = state_with_bus(&scratch.0);
        let now = super::now_unix();
        s.file.timers.push(TimerEntry {
            label: "tea".into(),
            duration_secs: 300,
            run: TimerRun::Running {
                deadline_unix: now + 300,
            },
        });
        s.file.timers.push(TimerEntry {
            label: "rest".into(),
            duration_secs: 60,
            run: TimerRun::Paused { remaining_secs: 30 },
        });
        s.file.alarms.push(AlarmEntry {
            label: "standup".into(),
            hour: 9,
            minute: 30,
            enabled: true,
            last_fired_day: None,
        });
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(960.0, 640.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-timers", |ui| timers_panel(ui, &mut s));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the Timers panel painted nothing");
    }
}
