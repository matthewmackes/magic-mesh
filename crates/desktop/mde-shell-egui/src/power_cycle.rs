//! Governed local and mesh power controls for the final Control Panel category.
//!
//! The page presents one explicit target, a fixed allowlist of actions, and a
//! five-second KIRON safety countdown. Remote destructive actions preserve the
//! host-state worker's signed propose/confirm contract; this module does not
//! create a generic command path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{self, RichText};
use mde_egui::nav_chrome::AppFrame;
use mde_egui::Style;
use mde_seat::PowerVerb;
use serde::Deserialize;

use crate::system::SystemState;

const COUNTDOWN: Duration = Duration::from_secs(5);
const TARGET_REFRESH: Duration = Duration::from_secs(2);
const REMOTE_FRESHNESS: Duration = Duration::from_secs(30);
const MIRROR_PREFIX: &str = "state/host/";
const MIRROR_SUFFIX: &str = "/seat";
// WL-FUNC-023 S4 — the legacy Construct entry point must reach the one
// shipped lifecycle authority. Keep the environment override for an
// installed renderer-specific launcher, but default to the shared TUI rather
// than a binary that is not packaged by the RPM.
const ONBOARDING_BIN: &str = "/usr/bin/magic-setup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerCycleAction {
    Restart,
    ShutDown,
    Suspend,
    LogOut,
}

impl PowerCycleAction {
    const ALL: [Self; 4] = [Self::Restart, Self::ShutDown, Self::Suspend, Self::LogOut];

    const fn label(self) -> &'static str {
        match self {
            Self::Restart => "Restart",
            Self::ShutDown => "Shut Down",
            Self::Suspend => "Suspend",
            Self::LogOut => "Log Out",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Restart => "Stop services cleanly, then restart the selected node.",
            Self::ShutDown => "Stop services cleanly, then power off the selected node.",
            Self::Suspend => "Pause the selected node in memory until it wakes.",
            Self::LogOut => "End the selected graphical user session.",
        }
    }

    const fn destructive(self) -> bool {
        matches!(self, Self::Restart | Self::ShutDown)
    }

    const fn power_verb(self) -> Option<PowerVerb> {
        match self {
            Self::Restart => Some(PowerVerb::Reboot),
            Self::ShutDown => Some(PowerVerb::PowerOff),
            Self::Suspend => Some(PowerVerb::Suspend),
            Self::LogOut => None,
        }
    }

    const fn remote_action(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Restart => Some(("reboot", "power:Reboot")),
            Self::ShutDown => Some(("poweroff", "power:PowerOff")),
            Self::Suspend => Some(("suspend", "power:Suspend")),
            Self::LogOut => None,
        }
    }
}

#[derive(Clone, Debug)]
struct TargetNode {
    id: String,
    local: bool,
    fresh: bool,
    age_seconds: Option<u64>,
    batteries: Vec<u8>,
}

#[derive(Debug)]
struct PendingAction {
    action: PowerCycleAction,
    target: String,
    local: bool,
    started: Instant,
}

#[derive(Debug)]
struct PendingRemoteResult {
    target: String,
    after_ms: u64,
}

#[derive(Debug, Deserialize)]
struct SeatMirrorSummary {
    #[serde(default)]
    batteries: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct RemoteResult {
    outcome: String,
    detail: String,
    node: String,
    #[serde(default)]
    requester: String,
}

pub(crate) struct PowerCycleState {
    bus_root: Option<PathBuf>,
    local_host: String,
    targets: Vec<TargetNode>,
    selected_target: String,
    explicit_override: bool,
    pending: Option<PendingAction>,
    status: Option<String>,
    error: Option<String>,
    last_refresh: Option<Instant>,
    submitted_remote: Option<PendingRemoteResult>,
}

impl Default for PowerCycleState {
    fn default() -> Self {
        let local_host = local_hostname();
        Self {
            bus_root: mde_bus::client_data_dir(),
            targets: vec![TargetNode {
                id: local_host.clone(),
                local: true,
                fresh: true,
                age_seconds: Some(0),
                batteries: Vec::new(),
            }],
            selected_target: local_host.clone(),
            local_host,
            explicit_override: false,
            pending: None,
            status: None,
            error: None,
            last_refresh: None,
            submitted_remote: None,
        }
    }
}

impl PowerCycleState {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, system: &mut SystemState) {
        self.refresh();
        self.poll_result();
        self.execute_elapsed(system);

        let _ = AppFrame::new("Safe Power Cycle Controls").show(ui);
        ui.colored_label(
            Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
            "Governed local and remote-node session and power actions.",
        );
        ui.add_space(Style::SP_M);

        self.show_target(ui, system);
        ui.add_space(Style::SP_M);
        self.show_countdown(ui);
        self.show_actions(ui, system);
        ui.add_space(Style::SP_L);
        self.show_onboarding_link(ui);
    }

    fn refresh(&mut self) {
        if self
            .last_refresh
            .is_some_and(|last| last.elapsed() < TARGET_REFRESH)
        {
            return;
        }
        self.last_refresh = Some(Instant::now());
        let mut targets = vec![TargetNode {
            id: self.local_host.clone(),
            local: true,
            fresh: true,
            age_seconds: Some(0),
            batteries: Vec::new(),
        }];
        let Some(root) = self.bus_root.clone() else {
            self.targets = targets;
            self.selected_target = self.local_host.clone();
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            self.targets = targets;
            return;
        };
        let now = unix_millis();
        let Ok(topics) = persist.list_topics_with_prefix(MIRROR_PREFIX) else {
            self.targets = targets;
            return;
        };
        for topic in topics {
            let Some(node) = topic
                .strip_prefix(MIRROR_PREFIX)
                .and_then(|rest| rest.strip_suffix(MIRROR_SUFFIX))
                .filter(|node| !node.is_empty() && !node.contains('/'))
            else {
                continue;
            };
            if node == self.local_host || node == "local" {
                continue;
            }
            let Ok(Some(message)) = persist.read_latest(&topic) else {
                continue;
            };
            let age_ms = now.saturating_sub(u64::try_from(message.ts_unix_ms).unwrap_or(0));
            let age_seconds = age_ms / 1_000;
            let batteries = message
                .body
                .as_deref()
                .and_then(|body| serde_json::from_str::<SeatMirrorSummary>(body).ok())
                .map_or_else(Vec::new, |mirror| mirror.batteries);
            targets.push(TargetNode {
                id: node.to_owned(),
                local: false,
                fresh: age_ms <= REMOTE_FRESHNESS.as_millis() as u64,
                age_seconds: Some(age_seconds),
                batteries,
            });
        }
        targets.sort_by(|a, b| b.local.cmp(&a.local).then_with(|| a.id.cmp(&b.id)));
        if !targets
            .iter()
            .any(|target| target.id == self.selected_target)
        {
            self.selected_target = self.local_host.clone();
        }
        self.targets = targets;
    }

    fn show_target(&mut self, ui: &mut egui::Ui, system: &SystemState) {
        mde_egui::card().show(ui, |ui| {
            ui.label(RichText::new("Target").strong().size(Style::TITLE));
            ui.add_space(Style::SP_S);
            egui::ComboBox::from_id_salt("safe-power-target")
                .selected_text(self.target_label(&self.selected_target))
                .width((ui.available_width() * 0.55).max(240.0))
                .show_ui(ui, |ui| {
                    for target in &self.targets {
                        let label = if target.local {
                            format!("This Node · {}", target.id)
                        } else if target.fresh {
                            format!("{} · online", target.id)
                        } else {
                            format!("{} · stale", target.id)
                        };
                        ui.selectable_value(&mut self.selected_target, target.id.clone(), label);
                    }
                });

            if let Some(target) = self.selected().cloned() {
                ui.add_space(Style::SP_M);
                egui::Grid::new("safe-power-target-facts")
                    .num_columns(2)
                    .spacing([Style::SP_L, Style::SP_XS])
                    .show(ui, |ui| {
                        fact(ui, "State", if target.fresh { "Ready" } else { "Stale" });
                        fact(
                            ui,
                            "Observed",
                            if target.local {
                                "Live seat provider".to_owned()
                            } else {
                                format!("{} seconds ago", target.age_seconds.unwrap_or(0))
                            },
                        );
                        fact(
                            ui,
                            "Uptime",
                            if target.local {
                                local_uptime().unwrap_or_else(|| "Unavailable".to_owned())
                            } else {
                                "Not published by this node".to_owned()
                            },
                        );
                        fact(
                            ui,
                            "Controls",
                            if target.local {
                                if system.power_action_caps().is_some() {
                                    "Local logind provider"
                                } else {
                                    "Waiting for local capabilities"
                                }
                            } else {
                                "Signed host-state provider"
                            },
                        );
                        fact(
                            ui,
                            "Battery",
                            if target.batteries.is_empty() {
                                "No reading".to_owned()
                            } else {
                                target
                                    .batteries
                                    .iter()
                                    .map(|value| format!("{value}%"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            },
                        );
                    });
                ui.add_space(Style::SP_S);
                let blocker = if !target.fresh {
                    "Blocker: the target mirror is stale; controls remain disabled."
                } else if target.local && system.power_action_caps().is_none() {
                    "Blocker: local power capabilities have not been observed yet."
                } else {
                    "No target-level blocker detected. Individual provider policies still apply."
                };
                ui.colored_label(Style::resolve_color(ui.ctx(), Style::TEXT_DIM), blocker);
            }
        });
    }

    fn show_countdown(&mut self, ui: &mut egui::Ui) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let elapsed = pending.started.elapsed().min(COUNTDOWN);
        let remaining = COUNTDOWN.saturating_sub(elapsed);
        let seconds = remaining
            .as_secs()
            .saturating_add(u64::from(remaining.subsec_millis() > 0));
        let fill = elapsed.as_secs_f32() / COUNTDOWN.as_secs_f32();
        let mut cancel = false;
        egui::Frame::new()
            .fill(Style::SUPPORT_ERROR.gamma_multiply(0.10))
            .stroke(egui::Stroke::new(1.0, Style::SUPPORT_ERROR))
            .corner_radius(Style::RADIUS_M)
            .inner_margin(Style::SP_M)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("KIRON · SAFETY COUNTDOWN")
                        .strong()
                        .color(Style::SUPPORT_ERROR),
                );
                ui.label(format!(
                    "{} on {} in {seconds} seconds",
                    pending.action.label(),
                    pending.target
                ));
                ui.add(
                    egui::ProgressBar::new(fill)
                        .fill(Style::SUPPORT_ERROR)
                        .show_percentage(),
                );
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        if cancel {
            self.pending = None;
            self.status = Some("KIRON countdown cancelled; no action was sent.".to_owned());
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add_space(Style::SP_M);
    }

    fn show_actions(&mut self, ui: &mut egui::Ui, system: &SystemState) {
        let Some(target) = self.selected().cloned() else {
            return;
        };
        let pending = self.pending.is_some();
        let remote_destructive = !target.local;

        mde_egui::card().show(ui, |ui| {
            ui.label(RichText::new("Actions").strong().size(Style::TITLE));
            ui.colored_label(
                Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
                "Every available action pauses for a five-second KIRON countdown.",
            );
            if remote_destructive {
                ui.add_space(Style::SP_S);
                let override_response = ui.checkbox(
                    &mut self.explicit_override,
                    "Explicit override for remote restart or shutdown",
                );
                let _ = mde_egui::hover_text(
                    override_response,
                    "Confirms the operator accepts service interruption and permits the mesh leader interlock. It cannot bypass missing authority or an offline target.",
                );
            }
            ui.add_space(Style::SP_M);

            for action in PowerCycleAction::ALL {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(action.label()).strong());
                        ui.colored_label(
                            Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
                            action.description(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (available, reason) = action_availability(
                            action,
                            &target,
                            system,
                            self.explicit_override,
                            pending,
                        );
                        let response = ui.add_enabled(
                            available,
                            egui::Button::new(format!("{}…", action.label())),
                        );
                        let clicked = response.clicked();
                        if !available {
                            let _ = mde_egui::disabled_hover_text(response, reason);
                        }
                        if clicked {
                            self.pending = Some(PendingAction {
                                action,
                                target: target.id.clone(),
                                local: target.local,
                                started: Instant::now(),
                            });
                            self.error = None;
                            self.status = Some(format!(
                                "KIRON countdown started for {}.",
                                action.label()
                            ));
                        }
                    });
                });
                if action != PowerCycleAction::LogOut {
                    ui.separator();
                }
            }
        });

        if let Some(error) = &self.error {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::SUPPORT_ERROR, error);
        } else if let Some(status) = &self.status {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::resolve_color(ui.ctx(), Style::TEXT_DIM), status);
        }
    }

    fn execute_elapsed(&mut self, system: &mut SystemState) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if pending.started.elapsed() < COUNTDOWN {
            return;
        }
        let pending = self.pending.take().expect("elapsed pending action");
        let result = if pending.local {
            pending
                .action
                .power_verb()
                .ok_or_else(|| {
                    "No safe graphical session provider is available for Log Out.".to_owned()
                })
                .and_then(|verb| {
                    system
                        .dispatch_power_action(verb)
                        .then_some(())
                        .ok_or_else(|| {
                            format!(
                                "{} was refused by the local power provider.",
                                pending.action.label()
                            )
                        })
                })
        } else {
            self.publish_remote(&pending.target, pending.action)
        };
        match result {
            Ok(()) => {
                self.status = Some(format!(
                    "{} was accepted for {}.",
                    pending.action.label(),
                    pending.target
                ));
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                self.status = None;
            }
        }
    }

    fn publish_remote(&mut self, node: &str, action: PowerCycleAction) -> Result<(), String> {
        let (wire_action, kind_key) = action
            .remote_action()
            .ok_or_else(|| "Remote Log Out has no typed host-state provider.".to_owned())?;
        let root = self
            .bus_root
            .clone()
            .ok_or_else(|| "The mesh action Bus is unavailable.".to_owned())?;
        let persist =
            Persist::open(root).map_err(|error| format!("Open mesh action Bus: {error}"))?;
        let phases: &[&str] = if action.destructive() {
            &["propose", "confirm"]
        } else {
            &["confirm"]
        };
        let topic = format!("action/host/{node}/verb");
        let submitted_after_ms = unix_millis();
        for phase in phases {
            let unsigned = serde_json::json!({
                "schema_version": 1,
                "verb": "power",
                "action": wire_action,
                "confirm": action.destructive() && self.explicit_override,
                "phase": phase,
                "requester": self.local_host,
            })
            .to_string();
            let body =
                crate::iac::authorize_root_mutation_body(&unsigned, "host-state", node, kind_key)?;
            persist
                .write(&topic, Priority::High, None, Some(&body))
                .map_err(|error| format!("Publish remote {phase}: {error}"))?;
        }
        self.submitted_remote = Some(PendingRemoteResult {
            target: node.to_owned(),
            after_ms: submitted_after_ms,
        });
        Ok(())
    }

    fn poll_result(&mut self) {
        let Some(submitted) = self.submitted_remote.as_ref() else {
            return;
        };
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let topic = format!("state/host/{}/verb-result", submitted.target);
        let Ok(Some(message)) = persist.read_latest(&topic) else {
            return;
        };
        if u64::try_from(message.ts_unix_ms).unwrap_or(0) < submitted.after_ms {
            return;
        }
        let Some(result) = message
            .body
            .as_deref()
            .and_then(|body| serde_json::from_str::<RemoteResult>(body).ok())
            .filter(|result| {
                result.node == submitted.target && result.requester == self.local_host
            })
        else {
            return;
        };
        if result.outcome == "refused" {
            self.submitted_remote = None;
            self.error = Some(format!("Remote action refused: {}", result.detail));
            self.status = None;
        } else if result.outcome == "proposed" {
            self.status =
                Some("Remote action proposed; awaiting governed confirmation.".to_owned());
            self.error = None;
        } else {
            self.submitted_remote = None;
            self.status = Some(format!(
                "Remote action {}: {}",
                result.outcome, result.detail
            ));
            self.error = None;
        }
    }

    fn show_onboarding_link(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(Style::SP_M);
        ui.label(RichText::new("Node lifecycle").strong().size(Style::TITLE));
        ui.colored_label(
            Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
            "Add and remove people or nodes in the separate lifecycle experience.",
        );
        let path = onboarding_path();
        let available = trusted_executable(&path);
        let response = ui.add_enabled(
            available,
            egui::Button::new("Open Onboarding & Offboarding ↗"),
        );
        if response.clicked() {
            match Command::new(&path).spawn() {
                Ok(_) => {
                    self.status = Some("Opened Onboarding & Offboarding.".to_owned());
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(format!("Open Onboarding & Offboarding: {error}"));
                    self.status = None;
                }
            }
        } else if !available {
            let _ = mde_egui::disabled_hover_text(
                response,
                "Onboarding & Offboarding is not installed on this node.",
            );
            ui.colored_label(
                Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
                "Onboarding & Offboarding is not installed on this node.",
            );
        }
    }

    fn selected(&self) -> Option<&TargetNode> {
        self.targets
            .iter()
            .find(|target| target.id == self.selected_target)
    }

    fn target_label(&self, id: &str) -> String {
        self.targets
            .iter()
            .find(|target| target.id == id)
            .map_or_else(
                || id.to_owned(),
                |target| {
                    if target.local {
                        format!("This Node · {}", target.id)
                    } else {
                        target.id.clone()
                    }
                },
            )
    }
}

fn action_availability(
    action: PowerCycleAction,
    target: &TargetNode,
    system: &SystemState,
    explicit_override: bool,
    pending: bool,
) -> (bool, &'static str) {
    if pending {
        return (false, "Another KIRON countdown is active.");
    }
    if action == PowerCycleAction::LogOut {
        return (
            false,
            "No typed interactive graphical-session provider is available; Log Out remains disabled.",
        );
    }
    if !target.fresh {
        return (false, "The target's host mirror is stale.");
    }
    if !target.local && action.destructive() && !explicit_override {
        return (
            false,
            "Select Explicit Override before a remote restart or shutdown.",
        );
    }
    if target.local {
        let Some(verb) = action.power_verb() else {
            return (false, "This action has no local provider.");
        };
        let Some(caps) = system.power_action_caps() else {
            return (
                false,
                "Local power capabilities have not been observed yet.",
            );
        };
        let availability = caps.for_verb(verb);
        return (
            availability.offerable(),
            match availability {
                mde_seat::Avail::Yes => "Available",
                mde_seat::Avail::Challenge => "The provider requires authorization.",
                mde_seat::Avail::No => "Refused by local policy.",
                mde_seat::Avail::Na => "Not supported by this node.",
            },
        );
    }
    (true, "Available")
}

fn fact(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.colored_label(Style::resolve_color(ui.ctx(), Style::TEXT_DIM), label);
    ui.label(value.into());
    ui.end_row();
}

fn onboarding_path() -> PathBuf {
    std::env::var_os("MDE_ONBOARDING_OFFBOARDING_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(ONBOARDING_BIN))
}

fn trusted_executable(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "local".to_owned())
}

fn local_uptime() -> Option<String> {
    let seconds = fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()? as u64;
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    Some(if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else {
        format!("{hours}h {minutes}m")
    })
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_keep_the_requested_order_and_logout_has_no_fake_provider() {
        assert_eq!(
            PowerCycleAction::ALL.map(PowerCycleAction::label),
            ["Restart", "Shut Down", "Suspend", "Log Out"]
        );
        assert_eq!(PowerCycleAction::LogOut.power_verb(), None);
        assert_eq!(PowerCycleAction::LogOut.remote_action(), None);
    }

    #[test]
    fn onboarding_launcher_requires_absolute_regular_executable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let executable = directory.path().join("lifecycle-gui");
        fs::write(&executable, "#!/bin/sh\n").expect("write executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
                .expect("make executable");
        }
        assert!(trusted_executable(&executable));
        assert!(!trusted_executable(Path::new("relative-gui")));
    }

    #[test]
    fn panel_renders_at_compact_and_wide_sizes() {
        use mde_egui::egui::{pos2, vec2, Rect};

        for width in [640.0, 1_280.0] {
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let mut state = PowerCycleState::default();
            state.bus_root = None;
            let mut system = SystemState::default();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 800.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| state.show(ui, &mut system));
                },
            );
            assert!(
                !ctx.tessellate(output.shapes, output.pixels_per_point)
                    .is_empty(),
                "Safe Power Cycle panel painted no primitives at width {width}"
            );
        }
    }
}
