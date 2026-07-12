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

pub(super) fn mpris_response_bodies_for_request<C: MediaControl>(
    control: &C,
    body: &MprisRequestBody,
) -> Vec<MprisBody> {
    let mut reports = Vec::new();
    if body.request_player_list {
        reports.push(MprisBody {
            player_list: mpris_player_list(control),
            support_album_art_payload: Some(false),
            ..Default::default()
        });
    }
    if body.request_now_playing || body.request_volume {
        if let Some(player) = selected_mpris_player(control, &body.player) {
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
