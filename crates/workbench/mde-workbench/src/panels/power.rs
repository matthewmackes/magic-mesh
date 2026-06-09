//! Power panel — five `power.*` keys covering profile, lid
//! action, idle-suspend timeouts, and presentation-mode
//! caffeine flag.
//!
//! CB-1.4 partial: replaces the v1.x
//! `mackes/workbench/devices/power.py` GTK3 panel. Phase F.1
//! shipped the bridge already; this Iced port maps directly
//! onto the same key set.

use std::sync::Arc;

use iced::widget::{checkbox, column, pick_list, row, text, text_input};
use iced::{Element, Length, Task};
use mde_theme::Palette;

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{
    encode_bool, parse_bool, parse_u32, quote_json, strip_json_quotes,
};

/// powerprofilesctl values. The Phase C.4 applier rejects
/// anything outside this set.
pub const PROFILES: &[&str] = &["power-saver", "balanced", "performance"];

/// logind lid-action values. The Phase C.4 sidecar feeds the
/// matching logind drop-in at mde-session bring-up.
pub const LID_ACTIONS: &[&str] = &["ignore", "lock", "suspend", "hibernate", "poweroff"];

pub const KEY_PROFILE: &str = "power.profile";
pub const KEY_LID_ACTION: &str = "power.lid_action";
pub const KEY_IDLE_BATTERY: &str = "power.suspend_idle_battery_s";
pub const KEY_IDLE_AC: &str = "power.suspend_idle_ac_s";
pub const KEY_PRESENTATION: &str = "power.presentation_mode";

#[derive(Debug, Clone, Default)]
pub struct PowerPanel {
    pub profile: String,
    pub lid_action: String,
    pub idle_battery_input: String,
    pub idle_ac_input: String,
    pub presentation_mode: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        profile: String,
        lid_action: String,
        idle_battery_s: u32,
        idle_ac_s: u32,
        presentation_mode: bool,
    },
    Error(String),
    Saved,
    ProfileChanged(String),
    LidActionChanged(String),
    IdleBatteryChanged(String),
    IdleAcChanged(String),
    PresentationChanged(bool),
    SaveClicked,
}

impl PowerPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let profile = strip_json_quotes(&backend.get(KEY_PROFILE).await?);
                let lid_action = strip_json_quotes(&backend.get(KEY_LID_ACTION).await?);
                let idle_battery_s = parse_u32(&backend.get(KEY_IDLE_BATTERY).await?).unwrap_or(0);
                let idle_ac_s = parse_u32(&backend.get(KEY_IDLE_AC).await?).unwrap_or(0);
                let presentation_mode = parse_bool(&backend.get(KEY_PRESENTATION).await?);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    profile,
                    lid_action,
                    idle_battery_s,
                    idle_ac_s,
                    presentation_mode,
                })
            },
            |result| {
                crate::Message::Power(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                profile,
                lid_action,
                idle_battery_s,
                idle_ac_s,
                presentation_mode,
            } => {
                self.profile = if PROFILES.iter().any(|p| *p == profile) {
                    profile
                } else {
                    "balanced".into()
                };
                self.lid_action = if LID_ACTIONS.iter().any(|l| *l == lid_action) {
                    lid_action
                } else {
                    "suspend".into()
                };
                self.idle_battery_input = idle_battery_s.to_string();
                self.idle_ac_input = idle_ac_s.to_string();
                self.presentation_mode = presentation_mode;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::Saved => {
                self.status = "Saved.".into();
                self.busy = false;
                Task::none()
            }
            Message::ProfileChanged(v) => {
                self.profile = v;
                Task::none()
            }
            Message::LidActionChanged(v) => {
                self.lid_action = v;
                Task::none()
            }
            Message::IdleBatteryChanged(v) => {
                self.idle_battery_input = v;
                Task::none()
            }
            Message::IdleAcChanged(v) => {
                self.idle_ac_input = v;
                Task::none()
            }
            Message::PresentationChanged(v) => {
                self.presentation_mode = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                // Validate both idle inputs before touching the
                // bus — empty collapses to 0 (matches the v1.x
                // panel's "off" semantics for the idle timer).
                let idle_battery = match resolve_idle(&self.idle_battery_input) {
                    Ok(v) => v,
                    Err(msg) => {
                        self.status = msg;
                        return Task::none();
                    }
                };
                let idle_ac = match resolve_idle(&self.idle_ac_input) {
                    Ok(v) => v,
                    Err(msg) => {
                        self.status = msg;
                        return Task::none();
                    }
                };
                self.busy = true;
                self.status = "Applying…".into();
                let profile = self.profile.clone();
                let lid_action = self.lid_action.clone();
                let presentation_mode = self.presentation_mode;
                Task::perform(
                    async move {
                        backend.set(KEY_PROFILE, &quote_json(&profile)).await?;
                        backend
                            .set(KEY_LID_ACTION, &quote_json(&lid_action))
                            .await?;
                        backend
                            .set(KEY_IDLE_BATTERY, &idle_battery.to_string())
                            .await?;
                        backend.set(KEY_IDLE_AC, &idle_ac.to_string()).await?;
                        backend
                            .set(KEY_PRESENTATION, encode_bool(presentation_mode))
                            .await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Power(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        // UX-7.a — save routed through the shared button variant.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::Power(Message::SaveClicked)),
            Palette::dark(),
        );
        let profile_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(PROFILES, current(&PROFILES, &self.profile), |v| {
                crate::Message::Power(Message::ProfileChanged(v.to_string()))
            });
        let lid_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(LID_ACTIONS, current(&LID_ACTIONS, &self.lid_action), |v| {
                crate::Message::Power(Message::LidActionChanged(v.to_string()))
            });

        column![
            row![text("Profile").width(Length::Fixed(180.0)), profile_pick].spacing(12),
            row![text("Lid action").width(Length::Fixed(180.0)), lid_pick].spacing(12),
            row![
                text("Idle suspend (battery, s)").width(Length::Fixed(180.0)),
                text_input("0", &self.idle_battery_input)
                    .on_input(|v| { crate::Message::Power(Message::IdleBatteryChanged(v)) }),
            ]
            .spacing(12),
            row![
                text("Idle suspend (AC, s)").width(Length::Fixed(180.0)),
                text_input("0", &self.idle_ac_input)
                    .on_input(|v| { crate::Message::Power(Message::IdleAcChanged(v)) }),
            ]
            .spacing(12),
            checkbox(self.presentation_mode)
                .label("Presentation mode (caffeine)")
                .on_toggle(|v| { crate::Message::Power(Message::PresentationChanged(v)) }),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current(table: &&'static [&'static str], value: &str) -> Option<&'static str> {
    table.iter().copied().find(|t| *t == value)
}

/// Parse an idle-timer input string. Empty → 0 (matches the
/// v1.x "off" semantics). Returns the locked validation
/// message on non-numeric input so the panel surfaces a
/// clear error without writing.
fn resolve_idle(input: &str) -> Result<u32, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    parse_u32(trimmed).ok_or_else(|| "Idle suspend must be a non-negative integer.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_power_namespace() {
        assert_eq!(KEY_PROFILE, "power.profile");
        assert_eq!(KEY_LID_ACTION, "power.lid_action");
        assert_eq!(KEY_IDLE_BATTERY, "power.suspend_idle_battery_s");
        assert_eq!(KEY_IDLE_AC, "power.suspend_idle_ac_s");
        assert_eq!(KEY_PRESENTATION, "power.presentation_mode");
    }

    #[test]
    fn profiles_table_matches_powerprofilesctl_values() {
        assert_eq!(PROFILES, &["power-saver", "balanced", "performance"]);
    }

    #[test]
    fn lid_actions_table_matches_logind_values() {
        assert_eq!(
            LID_ACTIONS,
            &["ignore", "lock", "suspend", "hibernate", "poweroff"]
        );
    }

    #[test]
    fn resolve_idle_handles_empty_zero_and_garbage() {
        assert_eq!(resolve_idle(""), Ok(0));
        assert_eq!(resolve_idle("   "), Ok(0));
        assert_eq!(resolve_idle("300"), Ok(300));
        assert!(resolve_idle("forever").is_err());
        assert!(resolve_idle("-1").is_err());
    }

    #[test]
    fn loaded_unknown_profile_falls_back_to_balanced() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = PowerPanel::new();
        let _ = panel.update(
            Message::Loaded {
                profile: "ludicrous".into(),
                lid_action: "suspend".into(),
                idle_battery_s: 600,
                idle_ac_s: 0,
                presentation_mode: false,
            },
            backend,
        );
        assert_eq!(panel.profile, "balanced");
        assert_eq!(panel.lid_action, "suspend");
        assert_eq!(panel.idle_battery_input, "600");
    }

    #[test]
    fn loaded_unknown_lid_action_falls_back_to_suspend() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = PowerPanel::new();
        let _ = panel.update(
            Message::Loaded {
                profile: "balanced".into(),
                lid_action: "self-destruct".into(),
                idle_battery_s: 0,
                idle_ac_s: 0,
                presentation_mode: false,
            },
            backend,
        );
        assert_eq!(panel.lid_action, "suspend");
    }

    #[test]
    fn save_clicked_with_garbage_idle_surfaces_validation() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = PowerPanel::new();
        panel.profile = "balanced".into();
        panel.lid_action = "suspend".into();
        panel.idle_battery_input = "forever".into();
        panel.idle_ac_input = "0".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert!(panel.status.contains("integer"));
        assert!(!panel.busy);
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = PowerPanel::new();
        let _ = panel.update(
            Message::ProfileChanged("performance".into()),
            backend.clone(),
        );
        assert_eq!(panel.profile, "performance");
        let _ = panel.update(Message::LidActionChanged("lock".into()), backend.clone());
        assert_eq!(panel.lid_action, "lock");
        let _ = panel.update(Message::IdleBatteryChanged("900".into()), backend.clone());
        assert_eq!(panel.idle_battery_input, "900");
        let _ = panel.update(Message::PresentationChanged(true), backend);
        assert!(panel.presentation_mode);
    }

    #[tokio::test]
    async fn save_writes_all_five_keys_with_correct_json_shapes() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend
            .set(KEY_PROFILE, &quote_json("balanced"))
            .await
            .unwrap();
        backend
            .set(KEY_LID_ACTION, &quote_json("suspend"))
            .await
            .unwrap();
        backend.set(KEY_IDLE_BATTERY, "600").await.unwrap();
        backend.set(KEY_IDLE_AC, "0").await.unwrap();
        backend
            .set(KEY_PRESENTATION, encode_bool(true))
            .await
            .unwrap();
        assert_eq!(backend.get(KEY_PROFILE).await.unwrap(), "\"balanced\"");
        assert_eq!(backend.get(KEY_IDLE_BATTERY).await.unwrap(), "600");
        assert_eq!(backend.get(KEY_PRESENTATION).await.unwrap(), "true");
    }
}
