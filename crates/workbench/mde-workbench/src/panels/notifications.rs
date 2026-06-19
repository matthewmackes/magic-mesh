//! Notifications panel — DND toggle + placement combo + default
//! expire-ms numeric input.
//!
//! CB-1.9 partial: replaces the v1.x
//! `mackes/workbench/system/notifications.py` GTK3 panel. The
//! Phase F.5 + C.5 wiring already routes:
//!   * `notification.do_not_disturb` → `$XDG_CACHE_HOME/mde/
//!     notifications-dnd` flag file
//!   * `notification.location` → notifications-prefs.json sidecar
//!   * `notification.default_expire_ms` → same sidecar

use std::sync::Arc;

use cosmic::iced::widget::{checkbox, column, pick_list, row, text};
use cosmic::iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{
    encode_bool, parse_bool, parse_u32, quote_json, strip_json_quotes,
};

/// Five-corner placement table the Phase C.5 applier accepts.
pub const LOCATIONS: &[&str] = &[
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
    "center",
];

/// Default expire-ms is locked to 5000 by Phase C.5; the panel
/// only enforces a sane floor (>=0) on parse.
pub const DEFAULT_EXPIRE_MS: u32 = 5000;

pub const KEY_DND: &str = "notification.do_not_disturb";
pub const KEY_LOCATION: &str = "notification.location";
pub const KEY_EXPIRE_MS: &str = "notification.default_expire_ms";

/// NOTIFY-6 — the fixed (non-`Peer`) alert groups whose sound the operator can
/// mute from this single settings surface. `Peer(<host>)` is dynamic and not a
/// fixed toggle. Order is the table's display order.
#[must_use]
pub fn sound_groups() -> [mde_notify::Source; 6] {
    use mde_notify::Source;
    [
        Source::Security,
        Source::Presence,
        Source::Firewall,
        Source::Compute,
        Source::DesktopApp,
        Source::System,
    ]
}

#[derive(Debug, Clone, Default)]
pub struct NotificationsPanel {
    pub dnd: bool,
    pub location: String,
    /// Held as a string for the text_input + numeric parse on
    /// Save. Empty string ⇒ "use default" (5000 ms).
    pub expire_ms_input: String,
    pub status: String,
    pub busy: bool,
    /// NOTIFY-6 — master notification-sound switch (mde-notify `SoundSettings`).
    pub sound_enabled: bool,
    /// NOTIFY-6 — muted group labels (per [`mde_notify::Source::label`]).
    pub muted_sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        dnd: bool,
        location: String,
        expire_ms: u32,
        sound_enabled: bool,
        muted_sources: Vec<String>,
    },
    Error(String),
    Saved,
    DndChanged(bool),
    LocationChanged(String),
    SaveClicked,
    /// NOTIFY-6 — master sound switch toggled (persists immediately).
    SoundMasterChanged(bool),
    /// NOTIFY-6 — a group's sound mute toggled (persists immediately).
    GroupMuteChanged {
        label: String,
        mute: bool,
    },
}

impl NotificationsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let dnd = parse_bool(&backend.get(KEY_DND).await?);
                let location = strip_json_quotes(&backend.get(KEY_LOCATION).await?);
                let expire_ms =
                    parse_u32(&backend.get(KEY_EXPIRE_MS).await?).unwrap_or(DEFAULT_EXPIRE_MS);
                // NOTIFY-6 — per-group sound settings live in the same
                // notify-sound.yaml the Hub + toast read (single source).
                let sound = load_sound_settings();
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    dnd,
                    location,
                    expire_ms,
                    sound_enabled: sound.enabled,
                    muted_sources: sound.muted_sources,
                })
            },
            |result| {
                crate::Message::Notifications(
                    result.unwrap_or_else(|e| Message::Error(e.to_string())),
                )
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                dnd,
                location,
                expire_ms,
                sound_enabled,
                muted_sources,
            } => {
                self.dnd = dnd;
                self.location = if LOCATIONS.iter().any(|l| *l == location) {
                    location
                } else {
                    "bottom-right".into()
                };
                self.expire_ms_input = expire_ms.to_string();
                self.sound_enabled = sound_enabled;
                self.muted_sources = muted_sources;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::SoundMasterChanged(v) => {
                self.sound_enabled = v;
                self.persist_sound();
                Task::none()
            }
            Message::GroupMuteChanged { label, mute } => {
                self.muted_sources.retain(|l| l != &label);
                if mute {
                    self.muted_sources.push(label);
                }
                self.persist_sound();
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
            Message::DndChanged(v) => {
                self.dnd = v;
                Task::none()
            }
            Message::LocationChanged(v) => {
                self.location = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                // Empty input collapses to the locked default
                // (matches the v1.x panel's blank-as-default
                // behaviour); non-numeric input surfaces a
                // validation error without touching the bus.
                let expire_ms = if self.expire_ms_input.trim().is_empty() {
                    DEFAULT_EXPIRE_MS
                } else {
                    match parse_u32(&self.expire_ms_input) {
                        Some(v) => v,
                        None => {
                            self.status = "Expire-ms must be a non-negative integer.".into();
                            return Task::none();
                        }
                    }
                };
                self.busy = true;
                self.status = "Applying…".into();
                let dnd = self.dnd;
                let location = self.location.clone();
                Task::perform(
                    async move {
                        backend.set(KEY_DND, encode_bool(dnd)).await?;
                        backend.set(KEY_LOCATION, &quote_json(&location)).await?;
                        backend.set(KEY_EXPIRE_MS, &expire_ms.to_string()).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Notifications(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
        }
    }

    /// NOTIFY-6 — write the current sound state to the shared
    /// `notify-sound.yaml` (the single source the Hub + toast read). Sync +
    /// tiny; best-effort (a missing bus root just sets a status note).
    fn persist_sound(&mut self) {
        let settings = mde_notify::SoundSettings {
            enabled: self.sound_enabled,
            muted_sources: self.muted_sources.clone(),
        };
        match mde_bus::client_data_dir() {
            Some(root) => match settings.save(&root) {
                Ok(()) => self.status = "Sound settings saved.".into(),
                Err(e) => self.status = format!("sound save failed: {e}"),
            },
            None => self.status = "no bus root — sound settings not saved".into(),
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        // UX-7.a — save routed through the shared button variant.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then_some(crate::Message::Notifications(Message::SaveClicked)),
            crate::live_theme::palette(),
        );
        let location_pick: pick_list::PickList<
            '_,
            &'static str,
            _,
            _,
            crate::Message,
            cosmic::Theme,
        > = pick_list(LOCATIONS, current_location(&self.location), |selected| {
            crate::Message::Notifications(Message::LocationChanged(selected.to_string()))
        });

        // NOTIFY-6 — the per-group sound section (single source: notify-sound.yaml).
        let master_enabled = self.sound_enabled;
        let mut sounds = column![
            text("Sounds").size(15),
            checkbox(self.sound_enabled)
                .label("Play notification sounds")
                .on_toggle(|v| crate::Message::Notifications(Message::SoundMasterChanged(v))),
        ]
        .spacing(8);
        for source in sound_groups() {
            let label = source.label();
            let muted = self.muted_sources.contains(&label);
            let group_label = label.clone();
            // Checked = audible; mute when unchecked. Disabled visually when the
            // master switch is off (toggling still records intent).
            sounds = sounds.push(
                checkbox(master_enabled && !muted)
                    .label(format!("  {label}"))
                    .on_toggle(move |audible| {
                        crate::Message::Notifications(Message::GroupMuteChanged {
                            label: group_label.clone(),
                            mute: !audible,
                        })
                    }),
            );
        }

        column![
            // NOTIFY-PREFS-2 — make the panel scope self-evident (the operator
            // had to ask what it controls vs the Notification Hub).
            text("Toast popups + sounds for this machine — the alert list lives in the Notification Hub.")
                .size(12),
            checkbox(self.dnd)
                .label("Do Not Disturb")
                .on_toggle(|v| { crate::Message::Notifications(Message::DndChanged(v)) }),
            row![text("Placement").width(Length::Fixed(160.0)), location_pick,].spacing(12),
            // NOTIFY-PREFS-1 — the default expire is fixed at 5000 ms (Phase C.5)
            // and the toast daemon doesn't read a per-machine override, so this is
            // shown read-only rather than as an editable field that's silently
            // ignored.
            row![
                text("Default expire").width(Length::Fixed(160.0)),
                text(format!("{DEFAULT_EXPIRE_MS} ms — fixed")).size(13),
            ]
            .spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
            sounds,
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_location(value: &str) -> Option<&'static str> {
    LOCATIONS.iter().copied().find(|l| *l == value)
}

/// NOTIFY-6 — load the shared sound settings (the same `notify-sound.yaml` the
/// Hub + toast read). Missing bus root or file → defaults (sound on).
fn load_sound_settings() -> mde_notify::SoundSettings {
    mde_bus::client_data_dir()
        .map(|root| mde_notify::SoundSettings::load(&root))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_notification_namespace() {
        assert_eq!(KEY_DND, "notification.do_not_disturb");
        assert_eq!(KEY_LOCATION, "notification.location");
        assert_eq!(KEY_EXPIRE_MS, "notification.default_expire_ms");
    }

    #[test]
    fn locations_table_is_five_corners() {
        assert_eq!(LOCATIONS.len(), 5);
        assert!(LOCATIONS.contains(&"bottom-right"));
        assert!(LOCATIONS.contains(&"center"));
    }

    #[test]
    fn default_expire_ms_matches_phase_c5_lock() {
        assert_eq!(DEFAULT_EXPIRE_MS, 5000);
    }

    #[test]
    fn loaded_unknown_location_falls_back_to_bottom_right() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        let _ = panel.update(
            Message::Loaded {
                dnd: true,
                location: "ghost".into(),
                expire_ms: 4000,
                sound_enabled: true,
                muted_sources: vec![],
            },
            backend,
        );
        assert_eq!(panel.location, "bottom-right");
        assert!(panel.dnd);
        assert_eq!(panel.expire_ms_input, "4000");
    }

    #[test]
    fn group_mute_toggle_adds_and_removes_one_label() {
        // NOTIFY-6: muting a group records its label; unmuting removes it —
        // single-sourced into the same list the Hub reads. (persist is
        // best-effort; with no bus root it just sets a status note.)
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        let sec = mde_notify::Source::Security.label();
        let _ = panel.update(
            Message::GroupMuteChanged {
                label: sec.clone(),
                mute: true,
            },
            backend.clone(),
        );
        assert!(panel.muted_sources.contains(&sec));
        let _ = panel.update(
            Message::GroupMuteChanged {
                label: sec.clone(),
                mute: false,
            },
            backend.clone(),
        );
        assert!(!panel.muted_sources.contains(&sec));
        // No duplicate when muted twice.
        let _ = panel.update(
            Message::GroupMuteChanged {
                label: sec.clone(),
                mute: true,
            },
            backend.clone(),
        );
        let _ = panel.update(
            Message::GroupMuteChanged {
                label: sec.clone(),
                mute: true,
            },
            backend,
        );
        assert_eq!(panel.muted_sources.iter().filter(|l| **l == sec).count(), 1);
    }

    #[test]
    fn sound_groups_are_the_six_fixed_non_peer_sources() {
        assert_eq!(sound_groups().len(), 6);
    }

    #[test]
    fn save_clicked_with_non_numeric_expire_surfaces_validation_error() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        panel.expire_ms_input = "forever".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert!(panel.status.contains("integer"));
        assert!(!panel.busy);
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        let _ = panel.update(Message::DndChanged(true), backend.clone());
        assert!(panel.dnd);
        let _ = panel.update(Message::LocationChanged("top-left".into()), backend.clone());
        assert_eq!(panel.location, "top-left");
        let _ = panel.update(Message::ExpireMsChanged("8000".into()), backend);
        assert_eq!(panel.expire_ms_input, "8000");
    }

    #[tokio::test]
    async fn save_writes_all_three_keys_in_canonical_json_shapes() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend.set(KEY_DND, encode_bool(true)).await.unwrap();
        backend
            .set(KEY_LOCATION, &quote_json("top-right"))
            .await
            .unwrap();
        backend.set(KEY_EXPIRE_MS, "3000").await.unwrap();
        assert_eq!(backend.get(KEY_DND).await.unwrap(), "true");
        assert_eq!(backend.get(KEY_LOCATION).await.unwrap(), "\"top-right\"");
        assert_eq!(backend.get(KEY_EXPIRE_MS).await.unwrap(), "3000");
    }
}
