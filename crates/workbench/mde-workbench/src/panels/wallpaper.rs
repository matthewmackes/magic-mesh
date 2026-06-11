//! Wallpaper panel — `wallpaper.path` + `wallpaper.mode`
//! settings keys per Phase C.7. The bg applet (Phase E.2 /
//! E1.2.12) watches the same sidecar and re-paints on change.

use std::sync::Arc;

use iced::widget::{column, pick_list, row, text, text_input};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{quote_json, strip_json_quotes};

/// Locked mode table per Phase C.7. `wallpaper.rs` validates
/// against this set before writing.
pub const MODES: &[&str] = &["stretch", "fit", "fill", "center", "tile"];

pub const KEY_PATH: &str = "wallpaper.path";
pub const KEY_MODE: &str = "wallpaper.mode";
/// PD-10 / L23 — whether the live mesh-map wallpaper is enabled.
pub const KEY_LIVE_MAP: &str = "wallpaper.live_map";

/// PD-10 — the systemd `--user` unit name the live-map wallpaper runs
/// under, so the toggle can start/stop it by name (survives the panel
/// closing; disabling restores the static wallpaper beneath).
const LIVE_MAP_UNIT: &str = "mde-mesh-wallpaper";

#[derive(Debug, Clone, Default)]
pub struct WallpaperPanel {
    pub path: String,
    pub mode: String,
    /// PD-10 / L23 — live mesh-map background on/off.
    pub live_map: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        path: String,
        mode: String,
        live_map: bool,
    },
    Error(String),
    Saved,
    PathChanged(String),
    ModeChanged(String),
    SaveClicked,
    /// PD-10 / L23 — flip the live mesh-map wallpaper.
    LiveMapToggled(bool),
    /// PD-10 — the start/stop resolved.
    LiveMapSet(bool),
}

impl WallpaperPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let path = strip_json_quotes(&backend.get(KEY_PATH).await?);
                let mode = strip_json_quotes(&backend.get(KEY_MODE).await?);
                // L23 — tolerate an absent key (upgrades won't have it):
                // missing/unreadable ⇒ off, never a load failure.
                let live_map = backend
                    .get(KEY_LIVE_MAP)
                    .await
                    .map(|v| strip_json_quotes(&v) == "true")
                    .unwrap_or(false);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    path,
                    mode,
                    live_map,
                })
            },
            |result| {
                crate::Message::Wallpaper(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                path,
                mode,
                live_map,
            } => {
                self.path = path;
                self.mode = if MODES.iter().any(|m| *m == mode) {
                    mode
                } else {
                    "fill".into()
                };
                self.live_map = live_map;
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
            Message::PathChanged(v) => {
                self.path = v;
                Task::none()
            }
            Message::ModeChanged(v) => {
                self.mode = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying…".into();
                let path = self.path.clone();
                let mode = self.mode.clone();
                Task::perform(
                    async move {
                        backend.set(KEY_PATH, &quote_json(&path)).await?;
                        backend.set(KEY_MODE, &quote_json(&mode)).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Wallpaper(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
            Message::LiveMapToggled(on) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.live_map = on; // optimistic; LiveMapSet/Error confirms
                self.status = if on {
                    "Enabling live mesh-map background…".into()
                } else {
                    "Disabling live map…".into()
                };
                Task::perform(
                    async move {
                        // Persist the intent first, then drive the unit.
                        let stored = quote_json(if on { "true" } else { "false" });
                        backend.set(KEY_LIVE_MAP, &stored).await?;
                        match set_live_map(on).await {
                            Ok(()) => {
                                Ok::<_, crate::backend::BackendError>(Message::LiveMapSet(on))
                            }
                            Err(e) => Ok(Message::Error(format!("live map: {e}"))),
                        }
                    },
                    |result| {
                        crate::Message::Wallpaper(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
            Message::LiveMapSet(on) => {
                self.live_map = on;
                self.busy = false;
                self.status = if on {
                    "Live mesh-map background enabled.".into()
                } else {
                    "Live map disabled — static wallpaper restored.".into()
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        // UX-7.a — save routed through the shared button variant.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then_some(crate::Message::Wallpaper(Message::SaveClicked)),
            crate::live_theme::palette(),
        );
        let mode_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(MODES, current_mode(&self.mode), |selected| {
                crate::Message::Wallpaper(Message::ModeChanged(selected.to_string()))
            });

        // PD-10 / L23 — live mesh-map background toggle. A button (not a
        // toggler) keeps it consistent with this panel's existing
        // controls; the label reflects the current state.
        let live_label = if self.live_map {
            "Disable live mesh map"
        } else {
            "Enable live mesh map"
        };
        let live_btn = variant_button(
            live_label,
            if self.live_map {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Primary
            },
            (!self.busy).then_some(crate::Message::Wallpaper(Message::LiveMapToggled(
                !self.live_map,
            ))),
            crate::live_theme::palette(),
        );
        let live_state = if self.live_map {
            "on — desktop shows the mesh map"
        } else {
            "off — using the static wallpaper above"
        };

        column![
            row![
                text("Image path").width(Length::Fixed(120.0)),
                text_input("/usr/share/backgrounds/mde-default.png", &self.path)
                    .on_input(|v| crate::Message::Wallpaper(Message::PathChanged(v))),
            ]
            .spacing(12),
            row![text("Mode").width(Length::Fixed(120.0)), mode_pick].spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
            row![
                text("Live mesh map").width(Length::Fixed(120.0)),
                live_btn,
                text(live_state).size(13),
            ]
            .spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_mode(value: &str) -> Option<&'static str> {
    MODES.iter().copied().find(|m| *m == value)
}

/// PD-10 / L23 — start or stop the live-map wallpaper as a named systemd
/// `--user` transient unit. `systemd-run` gives it a stable name so the
/// stop side can find it; `--collect` reaps the unit when the surface
/// exits. Stopping restores whatever static wallpaper sits beneath the
/// layer-shell Background surface.
async fn set_live_map(on: bool) -> Result<(), String> {
    let status = if on {
        tokio::process::Command::new("systemd-run")
            .args([
                "--user",
                "--collect",
                &format!("--unit={LIVE_MAP_UNIT}"),
                LIVE_MAP_UNIT,
            ])
            .status()
            .await
    } else {
        tokio::process::Command::new("systemctl")
            .args(["--user", "stop", LIVE_MAP_UNIT])
            .status()
            .await
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{} exited {s}", if on { "start" } else { "stop" })),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_wallpaper_namespace() {
        assert_eq!(KEY_PATH, "wallpaper.path");
        assert_eq!(KEY_MODE, "wallpaper.mode");
    }

    #[test]
    fn modes_match_phase_c7_lock() {
        assert_eq!(MODES, &["stretch", "fit", "fill", "center", "tile"]);
    }

    #[test]
    fn loaded_unknown_mode_falls_back_to_fill() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        let _ = panel.update(
            Message::Loaded {
                path: "/usr/share/backgrounds/x.png".into(),
                mode: "wibbly".into(),
                live_map: false,
            },
            backend,
        );
        assert_eq!(panel.mode, "fill");
        assert_eq!(panel.path, "/usr/share/backgrounds/x.png");
    }

    #[test]
    fn loaded_clears_busy_and_status() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(
            Message::Loaded {
                path: String::new(),
                mode: "fill".into(),
                live_map: false,
            },
            backend,
        );
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn loaded_reflects_live_map_state() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        let _ = panel.update(
            Message::Loaded {
                path: String::new(),
                mode: "fill".into(),
                live_map: true,
            },
            backend,
        );
        assert!(panel.live_map);
    }

    #[test]
    fn live_map_toggle_sets_optimistic_state_and_busy() {
        // L23 — toggling on flips the state immediately (optimistic) and
        // marks the panel busy while the unit starts.
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        assert!(!panel.live_map);
        let _ = panel.update(Message::LiveMapToggled(true), backend);
        assert!(panel.live_map);
        assert!(panel.busy);
    }

    #[test]
    fn live_map_set_confirms_and_clears_busy() {
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::LiveMapSet(true), Arc::new(DemoBackend::new()));
        assert!(panel.live_map);
        assert!(!panel.busy);
        assert!(panel.status.contains("enabled"));
        let _ = panel.update(Message::LiveMapSet(false), Arc::new(DemoBackend::new()));
        assert!(!panel.live_map);
        assert!(panel.status.contains("disabled") || panel.status.contains("restored"));
    }

    #[test]
    fn live_map_toggle_while_busy_is_noop() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        panel.live_map = false;
        let _ = panel.update(Message::LiveMapToggled(true), backend);
        // Busy guard: state unchanged.
        assert!(!panel.live_map);
    }

    #[test]
    fn live_map_key_is_namespaced() {
        assert_eq!(KEY_LIVE_MAP, "wallpaper.live_map");
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        let _ = panel.update(
            Message::PathChanged("/home/me/Pictures/bg.jpg".into()),
            backend.clone(),
        );
        assert_eq!(panel.path, "/home/me/Pictures/bg.jpg");
        let _ = panel.update(Message::ModeChanged("center".into()), backend);
        assert_eq!(panel.mode, "center");
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[tokio::test]
    async fn save_writes_both_keys_as_quoted_strings() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend.set(KEY_PATH, &quote_json("/x.png")).await.unwrap();
        backend.set(KEY_MODE, &quote_json("fill")).await.unwrap();
        assert_eq!(backend.get(KEY_PATH).await.unwrap(), "\"/x.png\"");
        assert_eq!(backend.get(KEY_MODE).await.unwrap(), "\"fill\"");
    }

    #[test]
    fn current_mode_returns_none_for_unknown() {
        assert_eq!(current_mode("fill"), Some("fill"));
        assert_eq!(current_mode("wibbly"), None);
    }
}
