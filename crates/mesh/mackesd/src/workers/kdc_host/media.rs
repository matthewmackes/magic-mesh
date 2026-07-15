//! KDC-MESH-6 — MPRIS media-key handling for the KDC host worker.
//!
//! Split out of the parent `kdc_host` god-file (behavior-preserving
//! relocation): the injectable [`MediaControl`] runner plus the pure
//! `playerctl` command/response helpers a paired phone drives over
//! `kdeconnect.mpris`.

use super::*;

// ───────────────────────── KDC-MESH-6: MPRIS media keys ───────────────────
//
// A paired phone sends `kdeconnect.mpris` command bodies for transport and player
// volume controls. The desktop-side standard control surface is MPRIS, so this
// worker shells the narrow, allowlisted action set through `playerctl` and never
// executes the raw phone-provided string.

const BROWSER_MPRIS_PLAYER: &str = "mde-browser";
const STATE_BROWSER_MEDIA_PREFIX: &str = "state/browser-media/";
const ACTION_BROWSER_MEDIA_CONTROL_PREFIX: &str = "action/browser/media-control/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PlayerctlInvocation {
    audit: &'static str,
    pub(super) args: &'static [&'static str],
}

/// Injectable runner for phone-originated media transport commands.
pub(super) trait MediaControl {
    /// Run one `playerctl` invocation. Returns whether a helper accepted the
    /// request; a missing helper is an honest no-op for the caller.
    fn run_playerctl(&self, invocation: PlayerctlInvocation) -> bool;

    /// Query `playerctl` and return stdout. A missing helper or failed query is
    /// an honest no-state result.
    fn query_playerctl(&self, args: &[String]) -> Option<String>;
}

/// Production media-control runner.
pub(super) struct PlayerctlMediaControl;

impl MediaControl for PlayerctlMediaControl {
    fn run_playerctl(&self, invocation: PlayerctlInvocation) -> bool {
        use std::process::{Command, Stdio};
        Command::new("playerctl")
            .args(invocation.args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    fn query_playerctl(&self, args: &[String]) -> Option<String> {
        use std::process::{Command, Stdio};
        let output = Command::new("playerctl")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }
}

/// Map KDE Connect's MPRIS action token to an allowlisted `playerctl` command.
fn playerctl_invocation_for_mpris_action(action: &str) -> Option<PlayerctlInvocation> {
    match action.trim().to_ascii_lowercase().as_str() {
        "play" => Some(PlayerctlInvocation {
            audit: "play",
            args: &["play"],
        }),
        "pause" => Some(PlayerctlInvocation {
            audit: "pause",
            args: &["pause"],
        }),
        "playpause" | "play-pause" | "toggle" => Some(PlayerctlInvocation {
            audit: "play-pause",
            args: &["play-pause"],
        }),
        "next" => Some(PlayerctlInvocation {
            audit: "next",
            args: &["next"],
        }),
        "previous" | "prev" => Some(PlayerctlInvocation {
            audit: "previous",
            args: &["previous"],
        }),
        "stop" => Some(PlayerctlInvocation {
            audit: "stop",
            args: &["stop"],
        }),
        "volumeup" | "volume-up" | "volup" | "raisevolume" | "raise-volume" => {
            Some(PlayerctlInvocation {
                audit: "volume-up",
                args: &["volume", "0.05+"],
            })
        }
        "volumedown" | "volume-down" | "voldown" | "lowervolume" | "lower-volume" => {
            Some(PlayerctlInvocation {
                audit: "volume-down",
                args: &["volume", "0.05-"],
            })
        }
        _ => None,
    }
}

fn browser_media_action_for_mpris(action: &str) -> Option<(&'static str, &'static str)> {
    match action.trim().to_ascii_lowercase().as_str() {
        "play" => Some(("play", "play")),
        "pause" => Some(("pause", "pause")),
        "playpause" | "play-pause" | "toggle" => Some(("play-pause", "play-pause")),
        "next" => Some(("next", "next")),
        "previous" | "prev" => Some(("previous", "previous")),
        "stop" => Some(("stop", "stop")),
        _ => None,
    }
}

fn browser_player_requested(player: &str) -> bool {
    matches!(
        player.trim().to_ascii_lowercase().as_str(),
        "mde-browser" | "browser" | "magic mesh browser"
    )
}

fn browser_media_status_topic(host: &str) -> String {
    format!("{STATE_BROWSER_MEDIA_PREFIX}{host}")
}

fn browser_media_control_topic(host: &str) -> String {
    format!("{ACTION_BROWSER_MEDIA_CONTROL_PREFIX}{host}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BrowserMediaStatus {
    state: String,
    tab_id: Option<u64>,
    title: String,
    artist: String,
    album: String,
    artwork_url: String,
    duration_ms: i64,
    position_ms: i64,
}

impl BrowserMediaStatus {
    fn is_active(&self) -> bool {
        self.state != "idle"
    }

    fn is_playing(&self) -> bool {
        self.state == "playing"
    }
}

fn json_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn json_i64(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .unwrap_or_default()
}

pub(super) fn parse_browser_media_status(body: &str) -> Option<BrowserMediaStatus> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("op").and_then(Value::as_str) != Some("browser_media_status") {
        return None;
    }
    let metadata = value.get("metadata").unwrap_or(&Value::Null);
    Some(BrowserMediaStatus {
        state: json_string(&value, "state"),
        tab_id: value.get("tab_id").and_then(Value::as_u64),
        title: json_string(metadata, "title").or_else_if_empty(|| json_string(&value, "label")),
        artist: json_string(metadata, "artist"),
        album: json_string(metadata, "album"),
        artwork_url: json_string(metadata, "artwork_url"),
        duration_ms: json_i64(metadata, "duration_ms"),
        position_ms: json_i64(metadata, "position_ms"),
    })
    .filter(BrowserMediaStatus::is_active)
}

trait EmptyStringExt {
    fn or_else_if_empty<F: FnOnce() -> String>(self, fallback: F) -> String;
}

impl EmptyStringExt for String {
    fn or_else_if_empty<F: FnOnce() -> String>(self, fallback: F) -> String {
        if self.is_empty() {
            fallback()
        } else {
            self
        }
    }
}

pub(super) fn browser_media_status_from_bus(
    bus_root: Option<&Path>,
    host: &str,
) -> Option<BrowserMediaStatus> {
    let persist = Persist::open(bus_root?.to_path_buf()).ok()?;
    let topic = browser_media_status_topic(host);
    persist
        .list_since(&topic, None)
        .ok()?
        .into_iter()
        .rev()
        .filter_map(|msg| msg.body.and_then(|body| parse_browser_media_status(&body)))
        .next()
}

fn browser_now_playing(status: &BrowserMediaStatus) -> String {
    match (status.artist.is_empty(), status.title.is_empty()) {
        (false, false) => format!("{} - {}", status.artist, status.title),
        (true, false) => status.title.clone(),
        (false, true) => status.artist.clone(),
        (true, true) => String::new(),
    }
}

fn browser_mpris_state_body(status: &BrowserMediaStatus, include_now_playing: bool) -> MprisBody {
    let mut body = MprisBody {
        player: BROWSER_MPRIS_PLAYER.to_owned(),
        can_pause: Some(true),
        can_play: Some(true),
        can_go_next: Some(true),
        can_go_previous: Some(true),
        can_seek: Some(status.duration_ms > 0),
        is_playing: status.is_playing(),
        pos: status.position_ms,
        length: status.duration_ms,
        ..Default::default()
    };
    if include_now_playing {
        body.artist = status.artist.clone();
        body.title = status.title.clone();
        body.album = status.album.clone();
        body.album_art_url = status.artwork_url.clone();
        body.now_playing = browser_now_playing(status);
    }
    body
}

fn publish_browser_media_control(
    bus_root: Option<&Path>,
    host: &str,
    action: &str,
    tab_id: Option<u64>,
) -> bool {
    let Some(root) = bus_root else {
        return false;
    };
    let Ok(persist) = Persist::open(root.to_path_buf()) else {
        return false;
    };
    let body = json!({
        "op": "browser_media_control",
        "source": "kdc_host",
        "player": BROWSER_MPRIS_PLAYER,
        "action": action,
        "tab_id": tab_id,
        "updated_ms": now_ms(),
    })
    .to_string();
    persist
        .write(
            &browser_media_control_topic(host),
            Priority::Default,
            None,
            Some(&body),
        )
        .is_ok()
}

/// Apply one inbound MPRIS command body. Returns the allowlisted command that was
/// attempted so the caller can audit the action without logging arbitrary input.
pub(super) fn apply_mpris_media_command<C: MediaControl>(
    control: &C,
    body: &MprisBody,
) -> Option<&'static str> {
    if body.kind() != MprisKind::Command {
        return None;
    }
    let invocation = playerctl_invocation_for_mpris_action(&body.action)?;
    let _accepted = control.run_playerctl(invocation);
    Some(invocation.audit)
}

pub(super) fn apply_browser_mpris_media_command(
    bus_root: Option<&Path>,
    host: &str,
    body: &MprisBody,
    status: Option<&BrowserMediaStatus>,
) -> Option<&'static str> {
    if body.kind() != MprisKind::Command || !browser_player_requested(&body.player) {
        return None;
    }
    let (audit, action) = browser_media_action_for_mpris(&body.action)?;
    let tab_id = status.and_then(|status| status.tab_id);
    let _accepted = publish_browser_media_control(bus_root, host, action, tab_id);
    Some(audit)
}

pub(super) fn apply_mpris_request_command<C: MediaControl>(
    control: &C,
    body: &MprisRequestBody,
) -> Option<&'static str> {
    if body.action.trim().is_empty() {
        return None;
    }
    let invocation = playerctl_invocation_for_mpris_action(&body.action)?;
    let _accepted = control.run_playerctl(invocation);
    Some(invocation.audit)
}

pub(super) fn apply_browser_mpris_request_command(
    bus_root: Option<&Path>,
    host: &str,
    body: &MprisRequestBody,
    status: Option<&BrowserMediaStatus>,
) -> Option<&'static str> {
    if body.action.trim().is_empty() || !browser_player_requested(&body.player) {
        return None;
    }
    let (audit, action) = browser_media_action_for_mpris(&body.action)?;
    let tab_id = status.and_then(|status| status.tab_id);
    let _accepted = publish_browser_media_control(bus_root, host, action, tab_id);
    Some(audit)
}

fn playerctl_query<C: MediaControl>(control: &C, args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    control.query_playerctl(&args)
}

fn mpris_player_list<C: MediaControl>(control: &C) -> Vec<String> {
    playerctl_query(control, &["-l"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn selected_mpris_player<C: MediaControl>(control: &C, requested: &str) -> Option<String> {
    let requested = requested.trim();
    if !requested.is_empty() {
        return Some(requested.to_string());
    }
    mpris_player_list(control).into_iter().next()
}

fn parse_playerctl_position_ms(raw: &str) -> Option<i64> {
    let seconds = raw.trim().parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round().clamp(0.0, i64::MAX as f64) as i64)
}

fn parse_playerctl_volume_percent(raw: &str) -> Option<i64> {
    let volume = raw.trim().parse::<f64>().ok()?;
    if !volume.is_finite() {
        return None;
    }
    Some((volume * 100.0).round().clamp(0.0, 100.0) as i64)
}

fn parse_mpris_length_ms(raw: &str) -> Option<i64> {
    let micros = raw.trim().parse::<i64>().ok()?;
    if micros <= 0 {
        return None;
    }
    Some(micros / 1000)
}

fn playerctl_for_player(player: &str, tail: &[&str]) -> Vec<String> {
    let mut args = vec!["-p".to_string(), player.to_string()];
    args.extend(tail.iter().map(|arg| (*arg).to_string()));
    args
}

fn mpris_state_body_for_player<C: MediaControl>(
    control: &C,
    player: &str,
    include_now_playing: bool,
    include_volume: bool,
) -> Option<MprisBody> {
    let mut body = MprisBody {
        player: player.to_string(),
        can_pause: Some(true),
        can_play: Some(true),
        can_go_next: Some(true),
        can_go_previous: Some(true),
        can_seek: Some(true),
        ..Default::default()
    };

    if let Some(status) = control.query_playerctl(&playerctl_for_player(player, &["status"])) {
        body.is_playing = status.trim().eq_ignore_ascii_case("playing");
    }
    if let Some(pos) = control
        .query_playerctl(&playerctl_for_player(player, &["position"]))
        .and_then(|raw| parse_playerctl_position_ms(&raw))
    {
        body.pos = pos;
    }
    if include_volume {
        body.volume = control
            .query_playerctl(&playerctl_for_player(player, &["volume"]))
            .and_then(|raw| parse_playerctl_volume_percent(&raw));
    }
    if include_now_playing {
        let metadata_args = playerctl_for_player(
            player,
            &[
                "metadata",
                "--format",
                "{{artist}}\n{{title}}\n{{album}}\n{{mpris:length}}\n{{mpris:artUrl}}",
            ],
        );
        if let Some(metadata) = control.query_playerctl(&metadata_args) {
            let mut lines = metadata.lines();
            body.artist = lines.next().unwrap_or_default().trim().to_string();
            body.title = lines.next().unwrap_or_default().trim().to_string();
            body.album = lines.next().unwrap_or_default().trim().to_string();
            body.length = lines
                .next()
                .and_then(parse_mpris_length_ms)
                .unwrap_or_default();
            body.album_art_url = lines.next().unwrap_or_default().trim().to_string();
            body.now_playing = match (body.artist.is_empty(), body.title.is_empty()) {
                (false, false) => format!("{} - {}", body.artist, body.title),
                (true, false) => body.title.clone(),
                (false, true) => body.artist.clone(),
                (true, true) => String::new(),
            };
        }
    }

    Some(body)
}

pub(super) fn mpris_response_bodies_for_request_with_browser<C: MediaControl>(
    control: &C,
    body: &MprisRequestBody,
    browser: Option<&BrowserMediaStatus>,
) -> Vec<MprisBody> {
    let mut reports = Vec::new();
    if body.request_player_list {
        let mut player_list = mpris_player_list(control);
        if browser.is_some_and(BrowserMediaStatus::is_active)
            && !player_list
                .iter()
                .any(|player| player == BROWSER_MPRIS_PLAYER)
        {
            player_list.push(BROWSER_MPRIS_PLAYER.to_owned());
        }
        reports.push(MprisBody {
            player_list,
            support_album_art_payload: Some(false),
            ..Default::default()
        });
    }
    if body.request_now_playing || body.request_volume {
        if browser.is_some_and(BrowserMediaStatus::is_active)
            && (browser_player_requested(&body.player)
                || (body.player.trim().is_empty() && selected_mpris_player(control, "").is_none()))
        {
            if let Some(status) = browser {
                reports.push(browser_mpris_state_body(status, body.request_now_playing));
            }
        } else if let Some(player) = selected_mpris_player(control, &body.player) {
            if let Some(report) = mpris_state_body_for_player(
                control,
                &player,
                body.request_now_playing,
                body.request_volume,
            ) {
                reports.push(report);
            }
        }
    }
    reports
}
