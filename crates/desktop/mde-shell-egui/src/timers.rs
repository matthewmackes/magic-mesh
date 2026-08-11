//! Daemon-projected Clock surface.
//!
//! The shell owns presentation state only. `mackesd` owns schedules, deadlines,
//! persistence, ringing, and replicated stopwatch state. Bus reads happen in
//! [`ClockState::pump`]; [`clock_panel`] is a pure egui projection that emits
//! typed [`ClockUiAction`] values and performs no Bus, network, persistence, or
//! scheduling I/O.

use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ed25519_dalek::SigningKey;
use jiff::{tz::TimeZone, Timestamp};
use mackes_mesh_types::clock::{
    clock_command_topic, clock_state_topic, ClockAcknowledgementV1, ClockAlarmRecurrenceV1,
    ClockAlarmV1, ClockAudioRef, ClockCivilTimeV1, ClockCommandKindV1, ClockCommandV1,
    ClockFoldPolicy, ClockGapPolicy, ClockOccurrencePhase, ClockScheduleKindV1, ClockScheduleV1,
    ClockSnapshotV1, ClockStopwatchPhase, ClockStopwatchV1, ClockTimerPhase, ClockTimerV1,
    ClockValidationContext, ClockWeekday, CLOCK_SCHEMA_VERSION, MAX_CLOCK_COMMAND_TTL_MS,
    MAX_CLOCK_LABEL_BYTES, MAX_CLOCK_TIMER_DURATION_MS,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{self, RichText};
use mde_egui::nav_chrome::AppFrame;
use mde_egui::Style;
const DAY_SECS: i64 = 86_400;
const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockZoneChoice {
    pub(crate) iana: &'static str,
    pub(crate) label: &'static str,
    pub(crate) detail: &'static str,
}

pub(crate) const CLOCK_ZONE_CHOICES: [ClockZoneChoice; 5] = [
    ClockZoneChoice {
        iana: "America/New_York",
        label: "Eastern Time",
        detail: "America/New_York",
    },
    ClockZoneChoice {
        iana: "America/Chicago",
        label: "Central Time",
        detail: "America/Chicago",
    },
    ClockZoneChoice {
        iana: "America/Denver",
        label: "Mountain Time",
        detail: "America/Denver",
    },
    ClockZoneChoice {
        iana: "America/Los_Angeles",
        label: "Pacific Time",
        detail: "America/Los_Angeles",
    },
    ClockZoneChoice {
        iana: "UTC",
        label: "Coordinated Universal Time",
        detail: "UTC",
    },
];

static CLOCK_ZONE: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("America/New_York".to_owned()));

pub(crate) fn set_clock_zone(zone: &str) {
    if TimeZone::get(zone).is_err() {
        return;
    }
    if let Ok(mut configured) = CLOCK_ZONE.write() {
        zone.clone_into(&mut configured);
    }
}
pub(crate) fn display_unix() -> Result<i64, String> {
    let now = now_unix();
    display_offset_seconds_at(now).map(|offset| now.saturating_add(offset))
}
pub(crate) fn display_offset_seconds_at(unix_secs: i64) -> Result<i64, String> {
    zone_offset_seconds_at(&configured_clock_zone()?, unix_secs)
}
pub(crate) fn display_zone_label() -> String {
    configured_clock_zone().unwrap_or_else(|_| "unavailable".to_owned())
}
fn configured_clock_zone() -> Result<String, String> {
    CLOCK_ZONE
        .read()
        .map(|zone| zone.clone())
        .map_err(|_| "display time-zone state is unavailable".to_owned())
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExactZoneTime {
    clock: String,
    offset_seconds: i32,
}

fn exact_zone_time(zone: &str, unix_secs: i64) -> Result<ExactZoneTime, String> {
    let timestamp = Timestamp::from_second(unix_secs)
        .map_err(|_| "timestamp is outside the supported range".to_owned())?;
    let time_zone = TimeZone::get(zone).map_err(|_| {
        format!("IANA zone {zone} is unavailable in the configured time-zone database")
    })?;
    let zoned = timestamp.to_zoned(time_zone);
    Ok(ExactZoneTime {
        clock: format!("{:02}:{:02}", zoned.hour(), zoned.minute()),
        offset_seconds: zoned.offset().seconds(),
    })
}

fn zone_offset_seconds_at(zone: &str, unix_secs: i64) -> Result<i64, String> {
    exact_zone_time(zone, unix_secs).map(|time| i64::from(time.offset_seconds))
}
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
pub fn hhmm(unix_secs: i64) -> String {
    let tod = unix_secs.rem_euclid(DAY_SECS);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}
pub fn secs_to_next_minute(unix_secs: i64) -> u64 {
    u64::try_from(60 - unix_secs.rem_euclid(60)).unwrap_or(60)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClockSection {
    #[default]
    WorldClock,
    Alarms,
    Timers,
    Stopwatch,
}
impl ClockSection {
    const ALL: [Self; 4] = [
        Self::WorldClock,
        Self::Alarms,
        Self::Timers,
        Self::Stopwatch,
    ];
    const fn label(self) -> &'static str {
        match self {
            Self::WorldClock => "World Clock",
            Self::Alarms => "Alarms",
            Self::Timers => "Timers",
            Self::Stopwatch => "Stopwatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimerAction {
    Pause,
    Resume,
    Restart,
    AddMinute,
    Stop,
    Remove,
}

/// Presentation-only Clock alert family.  The daemon projection remains the
/// authority for whether the referenced occurrence is still ringing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockBannerKind {
    Alarm,
    Timer,
}

/// Fixed Clock verbs exposed by banners and retained Notification Center rows.
/// No free-form topic, command, path, or URL crosses this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockBannerVerb {
    Snooze,
    AddMinute,
    Stop,
}

/// Bounded identity carried by a Clock action after the visual banner folds
/// into Notification Center.  Generation checks prevent an old row from acting
/// on a replacement occurrence or schedule that reused an identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockBannerAction {
    pub(crate) label: &'static str,
    pub(crate) verb: ClockBannerVerb,
    pub(crate) kind: ClockBannerKind,
    pub(crate) occurrence_id: String,
    pub(crate) occurrence_revision: u64,
    pub(crate) schedule_id: String,
    pub(crate) schedule_revision: u64,
    pub(crate) admitted_snapshot_revision: u64,
    pub(crate) valid_until_utc_ms: i64,
}

/// One daemon-derived actionable Clock banner.  It contains display text and
/// two fixed typed actions only; scheduling and signing remain in `ClockState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockBannerProjection {
    pub(crate) headline: String,
    pub(crate) identity: String,
    pub(crate) actions: [ClockBannerAction; 2],
}

fn clock_banner_projection_id() -> egui::Id {
    egui::Id::new("shell-clock-banner-projection")
}

fn clock_banner_request_id() -> egui::Id {
    egui::Id::new("shell-clock-banner-action-request")
}

/// Read the latest presentation projection.  Consumers never perform Clock
/// daemon I/O and cannot manufacture action authority.
pub(crate) fn clock_banner_projection(ctx: &egui::Context) -> Vec<ClockBannerProjection> {
    ctx.data(|data| {
        data.get_temp(clock_banner_projection_id())
            .unwrap_or_default()
    })
}

/// Queue one fixed action for the Clock controller to validate and sign on its
/// next pump.  At most one click is retained per frame.
pub(crate) fn request_clock_banner_action(ctx: &egui::Context, action: ClockBannerAction) {
    ctx.data_mut(|data| data.insert_temp(clock_banner_request_id(), Some(action)));
}

fn take_clock_banner_action(ctx: &egui::Context) -> Option<ClockBannerAction> {
    ctx.data_mut(|data| {
        data.remove_temp::<Option<ClockBannerAction>>(clock_banner_request_id())
            .flatten()
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopwatchAction {
    Start,
    Pause,
    Lap,
    Reset,
}

/// Every operator gesture leaving the Clock renderer is represented here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClockUiAction {
    SelectSection(ClockSection),
    AddWorldClock {
        time_zone: String,
    },
    RemoveWorldClock {
        time_zone: String,
    },
    CreateAlarm {
        label: String,
        hour: u8,
        minute: u8,
    },
    SetAlarmEnabled {
        schedule_id: String,
        enabled: bool,
    },
    RemoveAlarm {
        schedule_id: String,
    },
    CreateTimer {
        label: String,
        duration_ms: u64,
    },
    ControlTimer {
        schedule_id: String,
        action: TimerAction,
    },
    ControlStopwatch {
        stopwatch_id: Option<String>,
        action: StopwatchAction,
    },
    AcknowledgeOccurrence {
        occurrence_id: String,
        stop: bool,
    },
}

#[derive(Default)]
struct ClockDrafts {
    world_zone: String,
    alarm_label: String,
    alarm_hour: u8,
    alarm_minute: u8,
    timer_label: String,
    timer_hours: u32,
    timer_minutes: u32,
    timer_seconds: u32,
}

pub(crate) struct ClockState {
    bus_root: Option<PathBuf>,
    node_id: String,
    signer_id: Option<String>,
    snapshot: Option<ClockSnapshotV1>,
    section: ClockSection,
    drafts: ClockDrafts,
    in_flight: Option<InFlightCommand>,
    last_poll: Option<Instant>,
    projection_error: Option<String>,
    error: Option<String>,
    command_note: Option<String>,
}

struct InFlightCommand {
    request_id: String,
    expected_revision: u64,
    expected_body: ClockCommandKindV1,
    published_at: Instant,
}

impl Default for ClockState {
    fn default() -> Self {
        Self::with_bus_root(mde_bus::client_data_dir(), local_hostname())
    }
}

impl ClockState {
    fn with_bus_root(bus_root: Option<PathBuf>, node_id: String) -> Self {
        Self {
            bus_root,
            node_id,
            signer_id: std::env::var("MDE_CLOCK_SIGNER_ID")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            snapshot: None,
            section: ClockSection::default(),
            drafts: ClockDrafts {
                world_zone: "Etc/UTC".into(),
                alarm_hour: 7,
                ..Default::default()
            },
            in_flight: None,
            last_poll: None,
            projection_error: None,
            error: None,
            command_note: None,
        }
    }
    /// I/O controller hook, called before the surface is rendered.
    pub(crate) fn pump(&mut self, ctx: &egui::Context) {
        if let Some(action) = take_clock_banner_action(ctx) {
            match self.validate_banner_action(action) {
                Ok(action) => self.publish_action(action),
                Err(error) => {
                    self.command_note = None;
                    self.error = Some(format!("Clock banner action was not sent: {error}"));
                }
            }
        }
        if self.last_poll.is_some_and(|at| at.elapsed() < POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        ctx.request_repaint_after(POLL);
        if self.in_flight.as_ref().is_some_and(|pending| {
            pending.published_at.elapsed()
                > Duration::from_millis(
                    u64::try_from(MAX_CLOCK_COMMAND_TTL_MS).unwrap_or(5 * 60 * 1_000),
                )
        }) {
            self.in_flight = None;
            self.error =
                Some("Clock daemon confirmation did not arrive before the command expired.".into());
            self.command_note = None;
        }
        let Some(root) = self.bus_root.clone() else {
            self.projection_error =
                Some("Clock daemon projection unavailable: no Bus root.".into());
            return;
        };
        let result = (|| {
            let persist = Persist::open(root).map_err(|e| e.to_string())?;
            let topic = clock_state_topic(&self.node_id).map_err(|e| e.to_string())?;
            let message = persist
                .read_latest(&topic)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "waiting for the Clock daemon projection".to_string())?;
            let body = message
                .body
                .ok_or_else(|| "Clock projection had no body".to_string())?;
            let now = now_unix().saturating_mul(1000);
            let context = ClockValidationContext {
                wall_utc_ms: now,
                monotonic_ms: 1,
                zone_exists: &zone_exists,
            };
            ClockSnapshotV1::from_persisted_json_at(body.as_bytes(), &context)
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(snapshot) => {
                if self
                    .in_flight
                    .as_ref()
                    .is_some_and(|pending| snapshot.revision > pending.expected_revision)
                {
                    let pending = self.in_flight.take().expect("checked Clock command");
                    if command_effect_visible(&snapshot, &pending.expected_body) {
                        self.command_note = Some("Clock change applied by the daemon.".into());
                        self.error = None;
                    } else {
                        self.command_note = None;
                        self.error = Some(
                            "The Clock projection advanced without the requested change; the daemon refused or superseded it."
                                .into(),
                        );
                    }
                }
                publish_clock_banner_projection(ctx, &snapshot);
                self.snapshot = Some(snapshot);
                self.projection_error = None;
            }
            Err(error) => {
                ctx.data_mut(|data| {
                    data.insert_temp(
                        clock_banner_projection_id(),
                        Vec::<ClockBannerProjection>::new(),
                    );
                });
                self.projection_error = Some(error);
            }
        }
    }

    fn validate_banner_action(&self, action: ClockBannerAction) -> Result<ClockUiAction, String> {
        let now_ms = now_unix_ms()?;
        if now_ms > action.valid_until_utc_ms {
            return Err("the retained Clock action expired".to_owned());
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| "the daemon projection is unavailable".to_owned())?;
        if snapshot.revision < action.admitted_snapshot_revision {
            return Err("the Clock projection is older than the retained action".to_owned());
        }
        let occurrence = snapshot
            .occurrences
            .iter()
            .find(|item| item.occurrence_id == action.occurrence_id)
            .ok_or_else(|| "the Clock occurrence is no longer present".to_owned())?;
        if occurrence.revision != action.occurrence_revision
            || occurrence.schedule_id != action.schedule_id
            || occurrence.phase != ClockOccurrencePhase::Ringing
        {
            return Err("the retained Clock occurrence identity is stale".to_owned());
        }
        let schedule = snapshot
            .schedules
            .iter()
            .find(|item| item.schedule_id == action.schedule_id)
            .ok_or_else(|| "the Clock schedule is no longer present".to_owned())?;
        if schedule.revision != action.schedule_revision
            || !matches!(
                (action.kind, &schedule.schedule),
                (ClockBannerKind::Alarm, ClockScheduleKindV1::Alarm(_))
                    | (ClockBannerKind::Timer, ClockScheduleKindV1::Timer(_))
            )
        {
            return Err("the retained Clock schedule identity is stale".to_owned());
        }
        match (action.kind, action.verb) {
            (ClockBannerKind::Alarm, ClockBannerVerb::Snooze) => {
                Ok(ClockUiAction::AcknowledgeOccurrence {
                    occurrence_id: action.occurrence_id,
                    stop: false,
                })
            }
            (ClockBannerKind::Alarm | ClockBannerKind::Timer, ClockBannerVerb::Stop) => {
                Ok(ClockUiAction::AcknowledgeOccurrence {
                    occurrence_id: action.occurrence_id,
                    stop: true,
                })
            }
            (ClockBannerKind::Timer, ClockBannerVerb::AddMinute) => {
                Ok(ClockUiAction::ControlTimer {
                    schedule_id: action.schedule_id,
                    action: TimerAction::AddMinute,
                })
            }
            _ => Err("that action is not admitted for this Clock alert".to_owned()),
        }
    }
    fn emit(&mut self, action: ClockUiAction) {
        if let ClockUiAction::SelectSection(section) = action {
            self.section = section;
        } else {
            self.publish_action(action);
        }
    }

    fn publish_action(&mut self, action: ClockUiAction) {
        if self.in_flight.is_some() {
            self.error = Some(
                "A Clock change is still awaiting the daemon projection; try again after it applies."
                    .into(),
            );
            return;
        }
        match self.build_and_publish(action) {
            Ok(pending) => {
                self.command_note = Some(format!(
                    "Clock change {} was signed and published; awaiting daemon confirmation.",
                    pending.request_id
                ));
                self.error = None;
                self.in_flight = Some(pending);
            }
            Err(error) => {
                self.command_note = None;
                self.error = Some(format!("Clock change was not sent: {error}"));
            }
        }
    }

    fn build_and_publish(&self, action: ClockUiAction) -> Result<InFlightCommand, String> {
        let root = self
            .bus_root
            .as_ref()
            .ok_or_else(|| "the local mesh Bus is unavailable".to_owned())?;
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| "the daemon projection is unavailable".to_owned())?;
        let now_ms = now_unix_ms()?;
        let monotonic_ms = monotonic_ms();
        let body = command_body(action, snapshot, now_ms, monotonic_ms)?;
        let request_id = format!("clock-{}", uuid::Uuid::new_v4());
        let signer_id = self
            .signer_id
            .clone()
            .ok_or_else(|| "Clock signer configuration MDE_CLOCK_SIGNER_ID is absent".to_owned())?;
        let signing_key = SigningKey::from_bytes(&crate::iac::provisioned_shell_signing_seed()?);
        let context = ClockValidationContext {
            wall_utc_ms: now_ms,
            monotonic_ms,
            zone_exists: &zone_exists,
        };
        let command = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.clone(),
            origin_node_id: self.node_id.clone(),
            expected_revision: snapshot.revision,
            issued_at_utc_ms: now_ms,
            expires_at_utc_ms: now_ms.saturating_add(MAX_CLOCK_COMMAND_TTL_MS),
            body: body.clone(),
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(signer_id, &signing_key, &context)
        .map_err(|error| error.to_string())?;
        let encoded = serde_json::to_string(&command).map_err(|error| error.to_string())?;
        ClockCommandV1::from_json_at(encoded.as_bytes(), &context)
            .map_err(|error| error.to_string())?;
        let topic = clock_command_topic(&self.node_id).map_err(|error| error.to_string())?;
        Persist::open(root.clone())
            .and_then(|persist| {
                persist.write(&topic, Priority::Default, None, Some(encoded.as_str()))
            })
            .map_err(|error| error.to_string())?;
        Ok(InFlightCommand {
            request_id,
            expected_revision: snapshot.revision,
            expected_body: body,
            published_at: Instant::now(),
        })
    }
}

fn publish_clock_banner_projection(ctx: &egui::Context, snapshot: &ClockSnapshotV1) {
    let now_ms = now_unix().saturating_mul(1_000);
    let valid_until_utc_ms = now_ms.saturating_add(MAX_CLOCK_COMMAND_TTL_MS);
    let mut banners = Vec::new();
    for occurrence in snapshot
        .occurrences
        .iter()
        .filter(|item| item.phase == ClockOccurrencePhase::Ringing)
    {
        let Some(schedule) = snapshot
            .schedules
            .iter()
            .find(|item| item.schedule_id == occurrence.schedule_id)
        else {
            continue;
        };
        let (kind, first_label, first_verb, prefix) = match &schedule.schedule {
            ClockScheduleKindV1::Alarm(_) => (
                ClockBannerKind::Alarm,
                "Snooze",
                ClockBannerVerb::Snooze,
                "Alarm",
            ),
            ClockScheduleKindV1::Timer(_) => (
                ClockBannerKind::Timer,
                "Add 1 minute",
                ClockBannerVerb::AddMinute,
                "Timer",
            ),
        };
        let action = |label, verb| ClockBannerAction {
            label,
            verb,
            kind,
            occurrence_id: occurrence.occurrence_id.clone(),
            occurrence_revision: occurrence.revision,
            schedule_id: schedule.schedule_id.clone(),
            schedule_revision: schedule.revision,
            admitted_snapshot_revision: snapshot.revision,
            valid_until_utc_ms,
        };
        banners.push(ClockBannerProjection {
            headline: format!("{prefix} · {}", schedule.label),
            identity: format!(
                "{}:{}:{}:{}",
                occurrence.occurrence_id,
                occurrence.revision,
                schedule.schedule_id,
                schedule.revision
            ),
            actions: [
                action(first_label, first_verb),
                action("Stop", ClockBannerVerb::Stop),
            ],
        });
    }
    ctx.data_mut(|data| data.insert_temp(clock_banner_projection_id(), banners));
}

fn zone_exists(zone: &str) -> bool {
    !zone.starts_with('/') && !zone.contains("..") && TimeZone::get(zone).is_ok()
}

fn now_unix_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "the system clock is before the Unix epoch".to_owned())
        .and_then(|duration| {
            i64::try_from(duration.as_millis())
                .map_err(|_| "the system clock is outside the supported range".to_owned())
        })
}

fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    u64::try_from(elapsed).unwrap_or(u64::MAX).saturating_add(1)
}

fn action_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

fn action_label(value: &str, fallback: &str) -> Result<String, String> {
    let value = value.trim();
    let value = if value.is_empty() { fallback } else { value };
    if value.len() > MAX_CLOCK_LABEL_BYTES || value.chars().any(char::is_control) {
        return Err(format!(
            "Clock label must be at most {MAX_CLOCK_LABEL_BYTES} bytes and contain no control characters"
        ));
    }
    Ok(value.to_owned())
}

fn bundled_tone() -> ClockAudioRef {
    ClockAudioRef::Bundled {
        tone_id: "bright-bell".into(),
    }
}

fn find_schedule<'a>(
    snapshot: &'a ClockSnapshotV1,
    id: &str,
) -> Result<&'a ClockScheduleV1, String> {
    snapshot
        .schedules
        .iter()
        .find(|schedule| schedule.schedule_id == id)
        .ok_or_else(|| "the selected Clock schedule is no longer present".to_owned())
}

fn acknowledgement_body(
    snapshot: &ClockSnapshotV1,
    occurrence_id: &str,
    node_id: &str,
    now_ms: i64,
    stop: bool,
) -> Result<ClockCommandKindV1, String> {
    let occurrence = snapshot
        .occurrences
        .iter()
        .find(|occurrence| occurrence.occurrence_id == occurrence_id)
        .ok_or_else(|| "the ringing occurrence is no longer present".to_owned())?;
    if occurrence.phase != ClockOccurrencePhase::Ringing {
        return Err("the occurrence is no longer ringing".to_owned());
    }
    if !stop {
        let schedule = find_schedule(snapshot, &occurrence.schedule_id)?;
        if !matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_)) {
            return Err("only a ringing alarm can be snoozed".to_owned());
        }
    }
    Ok(ClockCommandKindV1::Acknowledge {
        occurrence_id: occurrence.occurrence_id.clone(),
        acknowledgement: ClockAcknowledgementV1 {
            acknowledgement_id: action_id(if stop { "stop" } else { "snooze" }),
            global_event_id: occurrence.global_event_id.clone(),
            actor_node_id: node_id.to_owned(),
            actor_clock: snapshot.revision.saturating_add(1).max(1),
            acknowledged_at_utc_ms: now_ms,
            stop,
        },
    })
}

fn stopwatch_elapsed(stopwatch: &ClockStopwatchV1, now_ms: i64) -> u64 {
    let live = stopwatch
        .started_wall_utc_ms
        .and_then(|started| u64::try_from(now_ms.saturating_sub(started)).ok())
        .unwrap_or(0);
    stopwatch.accumulated_elapsed_ms.saturating_add(live)
}

fn command_body(
    action: ClockUiAction,
    snapshot: &ClockSnapshotV1,
    now_ms: i64,
    monotonic_ms: u64,
) -> Result<ClockCommandKindV1, String> {
    match action {
        ClockUiAction::SelectSection(_) => Err("navigation is not a Clock mutation".to_owned()),
        ClockUiAction::AddWorldClock { time_zone } => {
            if !zone_exists(&time_zone) {
                return Err(format!(
                    "IANA zone {time_zone} is unavailable in the configured time-zone database"
                ));
            }
            let mut settings = snapshot.settings.clone();
            if settings.world_clock_time_zones.contains(&time_zone) {
                return Err("that world-clock zone is already present".to_owned());
            }
            settings.world_clock_time_zones.push(time_zone);
            Ok(ClockCommandKindV1::SetSettings { settings })
        }
        ClockUiAction::RemoveWorldClock { time_zone } => {
            let mut settings = snapshot.settings.clone();
            let before = settings.world_clock_time_zones.len();
            settings
                .world_clock_time_zones
                .retain(|zone| zone != &time_zone);
            if settings.world_clock_time_zones.len() == before {
                return Err("that world-clock zone is no longer present".to_owned());
            }
            Ok(ClockCommandKindV1::SetSettings { settings })
        }
        ClockUiAction::CreateAlarm {
            label,
            hour,
            minute,
        } => {
            let label = action_label(&label, "Alarm")?;
            let schedule_id = action_id("alarm");
            Ok(ClockCommandKindV1::UpsertSchedule {
                schedule: ClockScheduleV1 {
                    schedule_id,
                    origin_node_id: snapshot.node_id.clone(),
                    revision: 1,
                    label: label.clone(),
                    selected_target_ids: vec![snapshot.node_id.clone()],
                    schedule: ClockScheduleKindV1::Alarm(ClockAlarmV1 {
                        enabled: true,
                        label,
                        recurrence: ClockAlarmRecurrenceV1::Weekdays {
                            local_time: ClockCivilTimeV1 {
                                hour,
                                minute,
                                second: 0,
                                time_zone: snapshot.settings.this_node_time_zone.clone(),
                                fold: ClockFoldPolicy::Earlier,
                                gap: ClockGapPolicy::NextValid,
                            },
                            weekdays: vec![
                                ClockWeekday::Monday,
                                ClockWeekday::Tuesday,
                                ClockWeekday::Wednesday,
                                ClockWeekday::Thursday,
                                ClockWeekday::Friday,
                                ClockWeekday::Saturday,
                                ClockWeekday::Sunday,
                            ],
                        },
                        sound: bundled_tone(),
                        vibrate: false,
                    }),
                },
            })
        }
        ClockUiAction::SetAlarmEnabled {
            schedule_id,
            enabled,
        } => Ok(ClockCommandKindV1::SetScheduleEnabled {
            schedule_id,
            enabled,
        }),
        ClockUiAction::RemoveAlarm { schedule_id } => {
            Ok(ClockCommandKindV1::RemoveSchedule { schedule_id })
        }
        ClockUiAction::CreateTimer { label, duration_ms } => {
            if duration_ms == 0 || duration_ms > MAX_CLOCK_TIMER_DURATION_MS {
                return Err("timer duration is outside the admitted Clock range".to_owned());
            }
            let label = action_label(&label, "Timer")?;
            Ok(ClockCommandKindV1::UpsertSchedule {
                schedule: ClockScheduleV1 {
                    schedule_id: action_id("timer"),
                    origin_node_id: snapshot.node_id.clone(),
                    revision: 1,
                    label,
                    selected_target_ids: vec![snapshot.node_id.clone()],
                    schedule: ClockScheduleKindV1::Timer(ClockTimerV1 {
                        original_duration_ms: duration_ms,
                        phase: ClockTimerPhase::Running,
                        absolute_deadline_utc_ms: Some(now_ms.saturating_add(duration_ms as i64)),
                        paused_remaining_ms: None,
                        expired_at_utc_ms: None,
                        sound: bundled_tone(),
                        vibrate: false,
                    }),
                },
            })
        }
        ClockUiAction::ControlTimer {
            schedule_id,
            action,
        } => {
            if action == TimerAction::Remove {
                return Ok(ClockCommandKindV1::RemoveSchedule { schedule_id });
            }
            if action == TimerAction::Stop {
                let occurrence = snapshot
                    .occurrences
                    .iter()
                    .find(|occurrence| {
                        occurrence.schedule_id == schedule_id
                            && occurrence.phase == ClockOccurrencePhase::Ringing
                    })
                    .ok_or_else(|| {
                        "the expired timer has no ringing occurrence to stop".to_owned()
                    })?;
                return acknowledgement_body(
                    snapshot,
                    &occurrence.occurrence_id,
                    &snapshot.node_id,
                    now_ms,
                    true,
                );
            }
            let mut schedule = find_schedule(snapshot, &schedule_id)?.clone();
            let ClockScheduleKindV1::Timer(timer) = &mut schedule.schedule else {
                return Err("the selected schedule is not a timer".to_owned());
            };
            match action {
                TimerAction::Pause if timer.phase == ClockTimerPhase::Running => {
                    let remaining = timer
                        .absolute_deadline_utc_ms
                        .and_then(|deadline| u64::try_from(deadline.saturating_sub(now_ms)).ok())
                        .unwrap_or(0)
                        .min(timer.original_duration_ms);
                    timer.phase = ClockTimerPhase::Paused;
                    timer.absolute_deadline_utc_ms = None;
                    timer.paused_remaining_ms = Some(remaining);
                    timer.expired_at_utc_ms = None;
                }
                TimerAction::Resume if timer.phase == ClockTimerPhase::Paused => {
                    let remaining = timer.paused_remaining_ms.unwrap_or(0);
                    if remaining == 0 {
                        return Err("a zero-duration paused timer cannot resume".to_owned());
                    }
                    timer.phase = ClockTimerPhase::Running;
                    timer.absolute_deadline_utc_ms = Some(now_ms.saturating_add(remaining as i64));
                    timer.paused_remaining_ms = None;
                    timer.expired_at_utc_ms = None;
                }
                TimerAction::Restart if timer.phase == ClockTimerPhase::Expired => {
                    timer.phase = ClockTimerPhase::Running;
                    timer.absolute_deadline_utc_ms =
                        Some(now_ms.saturating_add(timer.original_duration_ms as i64));
                    timer.paused_remaining_ms = None;
                    timer.expired_at_utc_ms = None;
                }
                TimerAction::AddMinute if timer.phase == ClockTimerPhase::Expired => {
                    timer.phase = ClockTimerPhase::Running;
                    timer.absolute_deadline_utc_ms = Some(now_ms.saturating_add(60_000));
                    timer.paused_remaining_ms = None;
                    timer.expired_at_utc_ms = None;
                }
                _ => return Err("that timer action is stale for its current phase".to_owned()),
            }
            Ok(ClockCommandKindV1::UpsertSchedule { schedule })
        }
        ClockUiAction::ControlStopwatch {
            stopwatch_id,
            action,
        } => {
            let mut stopwatch = match stopwatch_id {
                Some(id) => snapshot
                    .stopwatches
                    .iter()
                    .find(|stopwatch| stopwatch.stopwatch_id == id)
                    .cloned()
                    .ok_or_else(|| "the selected stopwatch is no longer present".to_owned())?,
                None if action == StopwatchAction::Start => ClockStopwatchV1 {
                    stopwatch_id: action_id("stopwatch"),
                    origin_node_id: snapshot.node_id.clone(),
                    mirror_target_ids: vec![snapshot.node_id.clone()],
                    revision: 1,
                    phase: ClockStopwatchPhase::Reset,
                    started_wall_utc_ms: None,
                    started_monotonic_ms: None,
                    accumulated_elapsed_ms: 0,
                    laps: Vec::new(),
                },
                None => return Err("start the stopwatch before using that action".to_owned()),
            };
            if stopwatch.origin_node_id != snapshot.node_id {
                return Err("a mirrored stopwatch is read-only on this node".to_owned());
            }
            match action {
                StopwatchAction::Start if stopwatch.phase != ClockStopwatchPhase::Running => {
                    stopwatch.phase = ClockStopwatchPhase::Running;
                    stopwatch.started_wall_utc_ms = Some(now_ms);
                    stopwatch.started_monotonic_ms = Some(monotonic_ms);
                }
                StopwatchAction::Pause if stopwatch.phase == ClockStopwatchPhase::Running => {
                    stopwatch.accumulated_elapsed_ms = stopwatch_elapsed(&stopwatch, now_ms);
                    stopwatch.phase = ClockStopwatchPhase::Paused;
                    stopwatch.started_wall_utc_ms = None;
                    stopwatch.started_monotonic_ms = None;
                }
                StopwatchAction::Lap if stopwatch.phase == ClockStopwatchPhase::Running => {
                    let total = stopwatch_elapsed(&stopwatch, now_ms);
                    let previous = stopwatch.laps.last().map_or(0, |lap| lap.total_elapsed_ms);
                    let split = total.saturating_sub(previous);
                    if split == 0 {
                        return Err(
                            "wait for elapsed stopwatch time before recording a lap".to_owned()
                        );
                    }
                    stopwatch.accumulated_elapsed_ms = total;
                    stopwatch.started_wall_utc_ms = Some(now_ms);
                    stopwatch.started_monotonic_ms = Some(monotonic_ms);
                    stopwatch.laps.push(mackes_mesh_types::clock::ClockLapV1 {
                        lap_id: action_id("lap"),
                        split_elapsed_ms: split,
                        total_elapsed_ms: total,
                    });
                }
                StopwatchAction::Reset => {
                    stopwatch.phase = ClockStopwatchPhase::Reset;
                    stopwatch.started_wall_utc_ms = None;
                    stopwatch.started_monotonic_ms = None;
                    stopwatch.accumulated_elapsed_ms = 0;
                    stopwatch.laps.clear();
                }
                _ => return Err("that stopwatch action is stale for its current phase".to_owned()),
            }
            Ok(ClockCommandKindV1::UpsertStopwatch { stopwatch })
        }
        ClockUiAction::AcknowledgeOccurrence {
            occurrence_id,
            stop,
        } => acknowledgement_body(snapshot, &occurrence_id, &snapshot.node_id, now_ms, stop),
    }
}

fn command_effect_visible(snapshot: &ClockSnapshotV1, body: &ClockCommandKindV1) -> bool {
    match body {
        ClockCommandKindV1::UpsertSchedule { schedule } => snapshot
            .schedules
            .iter()
            .find(|candidate| candidate.schedule_id == schedule.schedule_id)
            .is_some_and(|candidate| {
                let mut candidate = candidate.clone();
                let mut expected = schedule.clone();
                candidate.revision = 1;
                expected.revision = 1;
                candidate == expected
            }),
        ClockCommandKindV1::RemoveSchedule { schedule_id } => !snapshot
            .schedules
            .iter()
            .any(|schedule| schedule.schedule_id == *schedule_id),
        ClockCommandKindV1::SetScheduleEnabled {
            schedule_id,
            enabled,
        } => snapshot.schedules.iter().any(|schedule| {
            schedule.schedule_id == *schedule_id
                && matches!(
                    &schedule.schedule,
                    ClockScheduleKindV1::Alarm(alarm) if alarm.enabled == *enabled
                )
        }),
        ClockCommandKindV1::Acknowledge {
            occurrence_id,
            acknowledgement,
        } => snapshot.occurrences.iter().any(|occurrence| {
            occurrence.occurrence_id == *occurrence_id
                && occurrence.acknowledgement.as_ref() == Some(acknowledgement)
        }),
        ClockCommandKindV1::UpsertStopwatch { stopwatch } => snapshot
            .stopwatches
            .iter()
            .find(|candidate| candidate.stopwatch_id == stopwatch.stopwatch_id)
            .is_some_and(|candidate| {
                let mut candidate = candidate.clone();
                let mut expected = stopwatch.clone();
                candidate.revision = 1;
                expected.revision = 1;
                candidate == expected
            }),
        ClockCommandKindV1::SetSettings { settings } => snapshot.settings == *settings,
    }
}

fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "localhost".into())
}
fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub(crate) fn clock_panel(ui: &mut egui::Ui, state: &mut ClockState) {
    let _ = AppFrame::new("Clock").show(ui);
    ui.add_space(Style::SP_S);
    ui.horizontal_wrapped(|ui| {
        for section in ClockSection::ALL {
            if ui
                .selectable_label(state.section == section, section.label())
                .clicked()
            {
                state.emit(ClockUiAction::SelectSection(section));
            }
        }
    });
    ui.separator();
    let snapshot = state.snapshot.clone();
    if let Some(error) = &state.error {
        ui.colored_label(Style::WARN, error);
    }
    if let Some(error) = &state.projection_error {
        ui.colored_label(Style::WARN, error);
    }
    if let Some(note) = &state.command_note {
        ui.colored_label(Style::TEXT_DIM, note);
    }
    if let Some(pending) = &state.in_flight {
        ui.colored_label(
            Style::WARN,
            format!(
                "Awaiting Clock daemon confirmation ({:.1}s).",
                pending.published_at.elapsed().as_secs_f32()
            ),
        );
    }
    match state.section {
        ClockSection::WorldClock => world_clock(ui, state, snapshot.as_ref()),
        ClockSection::Alarms => alarms(ui, state, snapshot.as_ref()),
        ClockSection::Timers => timers(ui, state, snapshot.as_ref()),
        ClockSection::Stopwatch => stopwatch(ui, state, snapshot.as_ref()),
    }
}

fn empty(ui: &mut egui::Ui, text: &str) {
    ui.add_space(Style::SP_M);
    ui.colored_label(Style::TEXT_DIM, text);
}
fn world_clock(ui: &mut egui::Ui, state: &mut ClockState, snapshot: Option<&ClockSnapshotV1>) {
    ui.heading("World Clock");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.drafts.world_zone);
        if ui.button("Add city").clicked() {
            let z = state.drafts.world_zone.trim().to_owned();
            if !z.is_empty() {
                state.emit(ClockUiAction::AddWorldClock { time_zone: z });
            }
        }
    });
    let Some(s) = snapshot else {
        return empty(ui, "Waiting for daemon-projected Clock settings.");
    };
    render_zone_time(ui, &s.settings.this_node_time_zone, now_unix(), true);
    for zone in &s.settings.world_clock_time_zones {
        ui.horizontal(|ui| {
            render_zone_time(ui, zone, now_unix(), false);
            if ui.button("Remove").clicked() {
                state.emit(ClockUiAction::RemoveWorldClock {
                    time_zone: zone.clone(),
                });
            }
        });
    }
}

fn render_zone_time(ui: &mut egui::Ui, zone: &str, unix_secs: i64, primary: bool) {
    match exact_zone_time(zone, unix_secs) {
        Ok(time) => {
            let text = format!("{} · {zone}", time.clock);
            if primary {
                ui.label(RichText::new(text).strong());
            } else {
                ui.label(text);
            }
        }
        Err(_) => {
            ui.colored_label(Style::WARN, format!("Unavailable · {zone}"));
        }
    }
}

fn alarms(ui: &mut egui::Ui, state: &mut ClockState, snapshot: Option<&ClockSnapshotV1>) {
    ui.heading("Alarms");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.drafts.alarm_label);
        ui.add(egui::DragValue::new(&mut state.drafts.alarm_hour).range(0..=23));
        ui.label(":");
        ui.add(egui::DragValue::new(&mut state.drafts.alarm_minute).range(0..=59));
        if ui.button("Add alarm").clicked() {
            state.emit(ClockUiAction::CreateAlarm {
                label: state.drafts.alarm_label.trim().to_owned(),
                hour: state.drafts.alarm_hour,
                minute: state.drafts.alarm_minute,
            });
        }
    });
    let Some(s) = snapshot else {
        return empty(ui, "Waiting for daemon-projected alarms.");
    };
    ringing_occurrences(ui, state, s);
    let mut any = false;
    for schedule in &s.schedules {
        if let ClockScheduleKindV1::Alarm(alarm) = &schedule.schedule {
            any = true;
            ui.horizontal(|ui| {
                let when = match &alarm.recurrence {
                    ClockAlarmRecurrenceV1::OneTime { due_at_utc_ms } => {
                        exact_zone_time(&s.settings.this_node_time_zone, due_at_utc_ms / 1000)
                            .map_or_else(|_| "Unavailable".into(), |time| time.clock)
                    }
                    ClockAlarmRecurrenceV1::Weekdays { local_time, .. } => {
                        format!("{:02}:{:02}", local_time.hour, local_time.minute)
                    }
                };
                ui.label(RichText::new(when).strong());
                ui.label(&schedule.label);
                let mut enabled = alarm.enabled;
                if ui.checkbox(&mut enabled, "Enabled").changed() {
                    state.emit(ClockUiAction::SetAlarmEnabled {
                        schedule_id: schedule.schedule_id.clone(),
                        enabled,
                    });
                }
                if ui.button("Remove").clicked() {
                    state.emit(ClockUiAction::RemoveAlarm {
                        schedule_id: schedule.schedule_id.clone(),
                    });
                }
            });
        }
    }
    if !any {
        empty(ui, "No alarms in the Clock daemon projection.");
    }
}

fn ringing_occurrences(ui: &mut egui::Ui, state: &mut ClockState, snapshot: &ClockSnapshotV1) {
    for occurrence in snapshot
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == ClockOccurrencePhase::Ringing)
    {
        let Some(schedule) = snapshot
            .schedules
            .iter()
            .find(|schedule| schedule.schedule_id == occurrence.schedule_id)
        else {
            continue;
        };
        if !matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_)) {
            continue;
        }
        ui.horizontal(|ui| {
            ui.colored_label(Style::WARN, format!("Ringing · {}", schedule.label));
            if ui.button("Snooze").clicked() {
                state.emit(ClockUiAction::AcknowledgeOccurrence {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    stop: false,
                });
            }
            if ui.button("Stop").clicked() {
                state.emit(ClockUiAction::AcknowledgeOccurrence {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    stop: true,
                });
            }
        });
    }
}
fn timers(ui: &mut egui::Ui, state: &mut ClockState, snapshot: Option<&ClockSnapshotV1>) {
    ui.heading("Timers");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.drafts.timer_label);
        ui.add(
            egui::DragValue::new(&mut state.drafts.timer_hours)
                .range(0..=99)
                .suffix("h"),
        );
        ui.add(
            egui::DragValue::new(&mut state.drafts.timer_minutes)
                .range(0..=59)
                .suffix("m"),
        );
        ui.add(
            egui::DragValue::new(&mut state.drafts.timer_seconds)
                .range(0..=59)
                .suffix("s"),
        );
        let ms = (u64::from(state.drafts.timer_hours) * 3600
            + u64::from(state.drafts.timer_minutes) * 60
            + u64::from(state.drafts.timer_seconds))
            * 1000;
        if ui.add_enabled(ms > 0, egui::Button::new("Start")).clicked() {
            state.emit(ClockUiAction::CreateTimer {
                label: state.drafts.timer_label.trim().to_owned(),
                duration_ms: ms,
            });
        }
    });
    let Some(s) = snapshot else {
        return empty(ui, "Waiting for daemon-projected timers.");
    };
    let now = now_unix().saturating_mul(1000);
    let mut any = false;
    for schedule in &s.schedules {
        if let ClockScheduleKindV1::Timer(timer) = &schedule.schedule {
            any = true;
            ui.horizontal(|ui| {
                ui.label(&schedule.label);
                let left = match timer.phase {
                    ClockTimerPhase::Running => timer
                        .absolute_deadline_utc_ms
                        .map_or(0, |d| u64::try_from(d.saturating_sub(now)).unwrap_or(0)),
                    ClockTimerPhase::Paused => timer.paused_remaining_ms.unwrap_or(0),
                    ClockTimerPhase::Expired => 0,
                };
                ui.label(RichText::new(fmt_duration(left)).strong());
                if timer.phase == ClockTimerPhase::Expired {
                    let ringing = s.occurrences.iter().any(|occurrence| {
                        occurrence.schedule_id == schedule.schedule_id
                            && occurrence.phase == ClockOccurrencePhase::Ringing
                    });
                    if ringing {
                        if ui.button("Stop").clicked() {
                            state.emit(ClockUiAction::ControlTimer {
                                schedule_id: schedule.schedule_id.clone(),
                                action: TimerAction::Stop,
                            });
                        }
                    } else {
                        if ui.button("Add 1 minute").clicked() {
                            state.emit(ClockUiAction::ControlTimer {
                                schedule_id: schedule.schedule_id.clone(),
                                action: TimerAction::AddMinute,
                            });
                        }
                        if ui.button("Restart").clicked() {
                            state.emit(ClockUiAction::ControlTimer {
                                schedule_id: schedule.schedule_id.clone(),
                                action: TimerAction::Restart,
                            });
                        }
                    }
                } else {
                    let action = if timer.phase == ClockTimerPhase::Running {
                        TimerAction::Pause
                    } else {
                        TimerAction::Resume
                    };
                    if ui.button(format!("{action:?}")).clicked() {
                        state.emit(ClockUiAction::ControlTimer {
                            schedule_id: schedule.schedule_id.clone(),
                            action,
                        });
                    }
                }
                if ui.button("Remove").clicked() {
                    state.emit(ClockUiAction::ControlTimer {
                        schedule_id: schedule.schedule_id.clone(),
                        action: TimerAction::Remove,
                    });
                }
            });
        }
    }
    if !any {
        empty(ui, "No timers in the Clock daemon projection.");
    }
}
fn stopwatch(ui: &mut egui::Ui, state: &mut ClockState, snapshot: Option<&ClockSnapshotV1>) {
    ui.heading("Stopwatch");
    let Some(s) = snapshot else {
        return empty(ui, "Waiting for daemon-projected stopwatch state.");
    };
    let sw = s.stopwatches.first();
    let now_ms = now_unix().saturating_mul(1_000);
    let elapsed = sw.map_or(0, |value| stopwatch_elapsed(value, now_ms));
    ui.label(RichText::new(fmt_duration(elapsed)).size(Style::DISPLAY));
    let phase = sw.map_or(ClockStopwatchPhase::Reset, |v| v.phase);
    let id = sw.map(|v| v.stopwatch_id.clone());
    ui.horizontal(|ui| {
        let primary = if phase == ClockStopwatchPhase::Running {
            StopwatchAction::Pause
        } else {
            StopwatchAction::Start
        };
        if ui.button(format!("{primary:?}")).clicked() {
            state.emit(ClockUiAction::ControlStopwatch {
                stopwatch_id: id.clone(),
                action: primary,
            });
        }
        if ui
            .add_enabled(
                phase == ClockStopwatchPhase::Running,
                egui::Button::new("Lap"),
            )
            .clicked()
        {
            state.emit(ClockUiAction::ControlStopwatch {
                stopwatch_id: id.clone(),
                action: StopwatchAction::Lap,
            });
        }
        if ui.button("Reset").clicked() {
            state.emit(ClockUiAction::ControlStopwatch {
                stopwatch_id: id,
                action: StopwatchAction::Reset,
            });
        }
    });
    if let Some(sw) = sw {
        for lap in sw.laps.iter().rev() {
            ui.label(format!(
                "Lap {} · {}",
                lap.lap_id,
                fmt_duration(lap.total_elapsed_ms)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> ClockSnapshotV1 {
        ClockSnapshotV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            node_id: "node-a".into(),
            revision: 7,
            produced_at_utc_ms: 1_720_646_365_000,
            schedules: Vec::new(),
            occurrences: Vec::new(),
            stopwatches: Vec::new(),
            settings: mackes_mesh_types::clock::ClockSettingsV1::defaults(
                "America/New_York".into(),
            ),
        }
    }

    fn ringing_snapshot() -> ClockSnapshotV1 {
        let mut value = snapshot();
        value.schedules = vec![
            ClockScheduleV1 {
                schedule_id: "alarm-1".into(),
                origin_node_id: "node-a".into(),
                revision: 3,
                label: "Wake up".into(),
                selected_target_ids: vec!["node-a".into()],
                schedule: ClockScheduleKindV1::Alarm(ClockAlarmV1 {
                    enabled: true,
                    label: "Wake up".into(),
                    recurrence: ClockAlarmRecurrenceV1::OneTime {
                        due_at_utc_ms: value.produced_at_utc_ms,
                    },
                    sound: bundled_tone(),
                    vibrate: false,
                }),
            },
            ClockScheduleV1 {
                schedule_id: "timer-1".into(),
                origin_node_id: "node-a".into(),
                revision: 4,
                label: "Tea".into(),
                selected_target_ids: vec!["node-a".into()],
                schedule: ClockScheduleKindV1::Timer(ClockTimerV1 {
                    original_duration_ms: 60_000,
                    phase: ClockTimerPhase::Expired,
                    absolute_deadline_utc_ms: None,
                    paused_remaining_ms: None,
                    expired_at_utc_ms: Some(value.produced_at_utc_ms),
                    sound: bundled_tone(),
                    vibrate: false,
                }),
            },
        ];
        value.occurrences = [
            ("occurrence-alarm", "alarm-1", 11),
            ("occurrence-timer", "timer-1", 12),
        ]
        .into_iter()
        .map(
            |(occurrence_id, schedule_id, revision)| mackes_mesh_types::clock::ClockOccurrenceV1 {
                occurrence_id: occurrence_id.into(),
                global_event_id: format!("global-{occurrence_id}"),
                schedule_id: schedule_id.into(),
                revision,
                due_at_utc_ms: value.produced_at_utc_ms,
                phase: ClockOccurrencePhase::Ringing,
                targets: Vec::new(),
                acknowledgement: None,
            },
        )
        .collect();
        value
    }

    #[test]
    fn clock_banners_expose_only_fixed_actions_with_generation_identity() {
        let ctx = egui::Context::default();
        let snapshot = ringing_snapshot();
        publish_clock_banner_projection(&ctx, &snapshot);
        let banners = clock_banner_projection(&ctx);
        assert_eq!(banners.len(), 2);
        assert_eq!(
            banners[0]
                .actions
                .clone()
                .map(|action| (action.label, action.verb)),
            [
                ("Snooze", ClockBannerVerb::Snooze),
                ("Stop", ClockBannerVerb::Stop),
            ]
        );
        assert_eq!(
            banners[1]
                .actions
                .clone()
                .map(|action| (action.label, action.verb)),
            [
                ("Add 1 minute", ClockBannerVerb::AddMinute),
                ("Stop", ClockBannerVerb::Stop),
            ]
        );
        assert_eq!(banners[0].actions[0].occurrence_revision, 11);
        assert_eq!(banners[1].actions[0].schedule_revision, 4);
    }

    #[test]
    fn retained_clock_action_refuses_stale_generation_and_maps_to_signed_path() {
        let snapshot = ringing_snapshot();
        let ctx = egui::Context::default();
        publish_clock_banner_projection(&ctx, &snapshot);
        let banners = clock_banner_projection(&ctx);
        let mut state = ClockState::with_bus_root(None, "node-a".into());
        state.snapshot = Some(snapshot);

        assert!(matches!(
            state
                .validate_banner_action(banners[0].actions[0].clone())
                .expect("fresh alarm action"),
            ClockUiAction::AcknowledgeOccurrence { stop: false, .. }
        ));
        assert!(matches!(
            state
                .validate_banner_action(banners[1].actions[0].clone())
                .expect("fresh timer action"),
            ClockUiAction::ControlTimer {
                action: TimerAction::AddMinute,
                ..
            }
        ));

        state.snapshot.as_mut().unwrap().occurrences[0].revision += 1;
        assert!(state
            .validate_banner_action(banners[0].actions[1].clone())
            .is_err());
    }

    #[test]
    fn ringing_timer_cannot_cross_the_alarm_snooze_authority_boundary() {
        let snapshot = ringing_snapshot();

        let result = command_body(
            ClockUiAction::AcknowledgeOccurrence {
                occurrence_id: "occurrence-timer".into(),
                stop: false,
            },
            &snapshot,
            snapshot.produced_at_utc_ms,
            1,
        );

        assert_eq!(result, Err("only a ringing alarm can be snoozed".to_owned()));
    }

    #[test]
    fn banner_click_publishes_a_signed_typed_clock_command_without_navigation() {
        let bus = tempfile::tempdir().expect("fixture Bus");
        let ctx = egui::Context::default();
        let snapshot = ringing_snapshot();
        publish_clock_banner_projection(&ctx, &snapshot);
        let action = clock_banner_projection(&ctx)[0].actions[0].clone();
        request_clock_banner_action(&ctx, action);

        let mut state = ClockState::with_bus_root(Some(bus.path().to_path_buf()), "node-a".into());
        state.signer_id = Some("clock-shell".into());
        state.snapshot = Some(snapshot);
        state.pump(&ctx);

        let persist = Persist::open(bus.path().to_path_buf()).expect("fixture Bus");
        let body = persist
            .read_latest("action/clock/command/node-a")
            .expect("read command")
            .expect("banner command")
            .body
            .expect("command body");
        let context = ClockValidationContext {
            wall_utc_ms: now_unix_ms().expect("clock"),
            monotonic_ms: monotonic_ms(),
            zone_exists: &zone_exists,
        };
        let command = ClockCommandV1::from_json_at(body.as_bytes(), &context)
            .expect("closed Clock command")
            .admit_at(
                "clock-shell",
                &SigningKey::from_bytes(&[7_u8; 32]).verifying_key(),
                &context,
            )
            .expect("valid Clock signature");
        assert!(matches!(
            command.body,
            ClockCommandKindV1::Acknowledge {
                acknowledgement: ClockAcknowledgementV1 { stop: false, .. },
                ..
            }
        ));
    }

    #[test]
    fn exact_iana_zoneinfo_handles_dst_and_refuses_unknown_zones() {
        let winter = exact_zone_time("America/New_York", 1_704_067_200).expect("winter");
        let summer = exact_zone_time("America/New_York", 1_719_792_000).expect("summer");
        assert_eq!(winter.offset_seconds, -5 * 3_600);
        assert_eq!(summer.offset_seconds, -4 * 3_600);
        assert!(exact_zone_time("Mars/Olympus_Mons", 1_719_792_000).is_err());
        assert!(zone_offset_seconds_at("Mars/Olympus_Mons", 1_719_792_000).is_err());
    }

    #[test]
    fn typed_clock_action_is_signed_and_published_on_the_canonical_topic() {
        let bus = tempfile::tempdir().expect("fixture Bus");
        let mut state = ClockState::with_bus_root(Some(bus.path().to_path_buf()), "node-a".into());
        state.signer_id = Some("clock-shell".into());
        state.snapshot = Some(snapshot());
        state.emit(ClockUiAction::CreateTimer {
            label: "Tea".into(),
            duration_ms: 300_000,
        });
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.in_flight.is_some());

        let persist = Persist::open(bus.path().to_path_buf()).expect("fixture Bus");
        let message = persist
            .read_latest("action/clock/command/node-a")
            .expect("read command")
            .expect("published command");
        let body = message.body.expect("command body");
        let now_ms = now_unix_ms().expect("clock");
        let context = ClockValidationContext {
            wall_utc_ms: now_ms,
            monotonic_ms: monotonic_ms(),
            zone_exists: &zone_exists,
        };
        let command = ClockCommandV1::from_json_at(body.as_bytes(), &context)
            .expect("closed Clock command")
            .admit_at(
                "clock-shell",
                &SigningKey::from_bytes(&[7_u8; 32]).verifying_key(),
                &context,
            )
            .expect("valid Clock signature");
        assert_eq!(command.expected_revision, 7);
        assert!(matches!(
            command.body,
            ClockCommandKindV1::UpsertSchedule {
                schedule: ClockScheduleV1 {
                    schedule: ClockScheduleKindV1::Timer(_),
                    ..
                }
            }
        ));
    }

    #[test]
    fn mirrored_stopwatch_refuses_local_control_commands() {
        let mut snapshot = snapshot();
        snapshot.stopwatches.push(ClockStopwatchV1 {
            stopwatch_id: "peer-stopwatch".into(),
            origin_node_id: "node-b".into(),
            mirror_target_ids: vec!["node-a".into()],
            revision: 6,
            phase: ClockStopwatchPhase::Running,
            started_wall_utc_ms: Some(snapshot.produced_at_utc_ms - 5_000),
            started_monotonic_ms: Some(1),
            accumulated_elapsed_ms: 0,
            laps: Vec::new(),
        });

        let result = command_body(
            ClockUiAction::ControlStopwatch {
                stopwatch_id: Some("peer-stopwatch".into()),
                action: StopwatchAction::Pause,
            },
            &snapshot,
            snapshot.produced_at_utc_ms,
            6_000,
        );

        assert_eq!(
            result,
            Err("a mirrored stopwatch is read-only on this node".to_owned())
        );
    }

    #[test]
    fn absent_signer_configuration_fails_visibly_closed() {
        let bus = tempfile::tempdir().expect("fixture Bus");
        let mut state = ClockState::with_bus_root(Some(bus.path().to_path_buf()), "node-a".into());
        state.signer_id = None;
        state.snapshot = Some(snapshot());
        state.emit(ClockUiAction::CreateTimer {
            label: "Tea".into(),
            duration_ms: 300_000,
        });
        assert!(state.in_flight.is_none());
        assert!(state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("MDE_CLOCK_SIGNER_ID")));
    }
    #[test]
    fn four_sections_are_explicit_and_stable() {
        assert_eq!(
            ClockSection::ALL.map(ClockSection::label),
            ["World Clock", "Alarms", "Timers", "Stopwatch"]
        );
    }
    #[test]
    fn shell_has_no_scheduling_or_store_authority() {
        let source = include_str!("timers.rs");
        let forbidden = [
            ["timers", "-alarms.json"].concat(),
            ["event/notify/", "timer"].concat(),
            ["fn tick", "_at"].concat(),
            ["fn pers", "ist("].concat(),
            ["deadline", "_unix"].concat(),
            ["us_daylight", "_time"].concat(),
            ["standard_offset", "_seconds"].concat(),
        ];
        for forbidden in forbidden {
            assert!(!source.contains(&forbidden), "stale authority: {forbidden}");
        }
    }
}
