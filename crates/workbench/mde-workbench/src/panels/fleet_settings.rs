//! Fleet Settings panel — push one setting key to the
//! fleet's reconcile loop via `mackesd fleet push-setting`.
//!
//! CB-1.5 partial: replaces the v1.x
//! `mackes/workbench/fleet/settings.py` GTK3 panel. F.11
//! already shipped the Python wrapper around the same `mackesd`
//! subcommand (Phase G.4); this Iced port mirrors the
//! grammar.

use std::process::Stdio;

use cosmic::iced::widget::{column, row, text, text_input};
use cosmic::iced::{Element, Length, Task};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct FleetSettingsPanel {
    pub key_input: String,
    pub value_input: String,
    pub peers_input: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    KeyChanged(String),
    ValueChanged(String),
    PeersChanged(String),
    PushClicked,
    PushCompleted(Result<String, String>),
}

impl FleetSettingsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            peers_input: "all".into(),
            ..Self::default()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::KeyChanged(v) => {
                self.key_input = v;
                Task::none()
            }
            Message::ValueChanged(v) => {
                self.value_input = v;
                Task::none()
            }
            Message::PeersChanged(v) => {
                self.peers_input = v;
                Task::none()
            }
            Message::PushClicked => {
                if self.busy {
                    return Task::none();
                }
                let key = self.key_input.trim().to_string();
                let value = self.value_input.trim().to_string();
                let peers = self.peers_input.trim().to_string();
                if key.is_empty() {
                    self.status = "Key cannot be empty.".into();
                    return Task::none();
                }
                if value.is_empty() {
                    self.status = "Value JSON cannot be empty.".into();
                    return Task::none();
                }
                let peers_arg = if peers.is_empty() {
                    "all".to_string()
                } else {
                    peers
                };
                self.busy = true;
                self.status = "Pushing…".into();
                let args = push_setting_args(&key, &value, &peers_arg);
                Task::perform(async move { run_mackesd(&args).await }, |result| {
                    crate::Message::FleetSettings(Message::PushCompleted(result))
                })
            }
            Message::PushCompleted(Ok(out)) => {
                self.status = if out.trim().is_empty() {
                    "Push accepted.".into()
                } else {
                    format!("Push accepted: {}", out.trim())
                };
                self.busy = false;
                Task::none()
            }
            Message::PushCompleted(Err(e)) => {
                self.status = format!("Push failed: {e}");
                self.busy = false;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let push_label = if self.busy {
            "Pushing…"
        } else {
            "Push to fleet"
        };
        // UX-7.a — push routed through the shared button variant.
        let push_btn = variant_button(
            push_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::FleetSettings(Message::PushClicked)),
            crate::live_theme::palette(),
        );

        column![
            row![
                text("Key").width(Length::Fixed(120.0)),
                text_input("theme.name", &self.key_input)
                    .on_input(|v| { crate::Message::FleetSettings(Message::KeyChanged(v)) }),
            ]
            .spacing(12),
            row![
                text("Value (JSON)").width(Length::Fixed(120.0)),
                text_input("\"Adwaita-dark\"", &self.value_input)
                    .on_input(|v| { crate::Message::FleetSettings(Message::ValueChanged(v)) }),
            ]
            .spacing(12),
            row![
                text("Peers").width(Length::Fixed(120.0)),
                text_input("all", &self.peers_input)
                    .on_input(|v| { crate::Message::FleetSettings(Message::PeersChanged(v)) }),
            ]
            .spacing(12),
            row![push_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Pure-fn argument builder — the exact argv `mackesd` sees when
/// the Push button fires. Lifted so tests can assert the
/// grammar without invoking the binary.
#[must_use]
pub fn push_setting_args(key: &str, value: &str, peers: &str) -> Vec<String> {
    vec![
        "fleet".to_string(),
        "push-setting".to_string(),
        key.to_string(),
        value.to_string(),
        "--peers".to_string(),
        peers.to_string(),
    ]
}

/// Spawn `mackesd <args>` and return stdout on success, an
/// error message on failure (binary missing / non-zero exit
/// status / output decode failure).
pub async fn run_mackesd(args: &[String]) -> Result<String, String> {
    let output = Command::new("mackesd")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "`mackesd` not on PATH — install MackesWorkstation or check $PATH".into()
            } else {
                format!("spawning mackesd failed: {e}")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "mackesd exited {} — {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_seeds_peers_to_all_by_default() {
        let panel = FleetSettingsPanel::new();
        assert_eq!(panel.peers_input, "all");
        assert!(panel.key_input.is_empty());
        assert!(!panel.busy);
    }

    #[test]
    fn push_setting_args_match_locked_grammar() {
        assert_eq!(
            push_setting_args("theme.name", "\"Adwaita-dark\"", "all"),
            vec![
                "fleet",
                "push-setting",
                "theme.name",
                "\"Adwaita-dark\"",
                "--peers",
                "all",
            ]
        );
    }

    #[test]
    fn push_setting_args_carry_node_selector_grammar() {
        let args = push_setting_args("font.name", "\"Inter 11\"", "node:laptop-01");
        assert!(args.iter().any(|a| a == "node:laptop-01"));
        assert!(args.windows(2).any(|w| w[0] == "--peers"));
    }

    #[test]
    fn empty_key_surfaces_validation_without_pushing() {
        let mut panel = FleetSettingsPanel::new();
        panel.value_input = "\"x\"".into();
        let _ = panel.update(Message::PushClicked);
        assert!(panel.status.contains("Key"));
        assert!(!panel.busy);
    }

    #[test]
    fn empty_value_surfaces_validation_without_pushing() {
        let mut panel = FleetSettingsPanel::new();
        panel.key_input = "theme.name".into();
        let _ = panel.update(Message::PushClicked);
        assert!(panel.status.contains("Value"));
        assert!(!panel.busy);
    }

    #[test]
    fn empty_peers_collapses_to_all_keyword_on_save() {
        // The reducer rewrites blank peers to "all" before
        // building argv — surface this via the validation
        // path (push with blank peers does not error).
        let mut panel = FleetSettingsPanel::new();
        panel.key_input = "theme.name".into();
        panel.value_input = "\"X\"".into();
        panel.peers_input = String::new();
        // Don't dispatch the actual run; the reducer sets
        // busy + status before firing the Task. We just need
        // to assert it didn't bail out as validation error.
        let _ = panel.update(Message::PushClicked);
        // busy is true while the Task runs; the validation
        // paths return without setting busy.
        assert!(panel.busy);
        assert!(panel.status.contains("Pushing"));
    }

    #[test]
    fn push_completed_ok_clears_busy_and_sets_status() {
        let mut panel = FleetSettingsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::PushCompleted(Ok("r-2026-05-20-0001".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("r-2026-05-20-0001"));
    }

    #[test]
    fn push_completed_err_clears_busy_and_surfaces_message() {
        let mut panel = FleetSettingsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::PushCompleted(Err("timeout".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("timeout"));
    }

    #[test]
    fn push_completed_ok_with_empty_stdout_falls_back_to_generic_message() {
        let mut panel = FleetSettingsPanel::new();
        let _ = panel.update(Message::PushCompleted(Ok(String::new())));
        assert_eq!(panel.status, "Push accepted.");
    }
}
