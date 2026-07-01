//! AIR-20 (v6.1) — Devices → Music settings panel.
//!
//! Manages the native music player's configuration without leaving the
//! Workbench: the Airsonic server connection (URL + credentials, saved
//! to the mesh-shared `airsonic-creds.json`), a test-connection probe,
//! the mesh audio cache (current size + cap slider that triggers
//! eviction, plus a clear button), and sign-out (deletes the creds).
//! Reuses the `mde-musicd` library modules (creds / cache / airsonic) so
//! the panel and the daemon agree byte-for-byte on storage + auth.
//!
//! The "allow this peer to be taken over" toggle is deferred until the
//! AIR-8 handoff path reads a setting (AIR-20.b); everything here works
//! end-to-end against the shipped modules with no audio backend.

use cosmic::iced::widget::{column, container, row, slider, text, text_input, Space};
use cosmic::iced::{Element, Length, Padding, Task};

use mde_musicd::airsonic::Client;
use mde_musicd::cache;
use mde_musicd::creds::{self, Creds};

use crate::components::connect_progress::{self, ConnectProgress};
use crate::controls::{styled_text_input, variant_button, ButtonVariant};

/// Cache cap slider bounds (GiB).
const CAP_MIN_GB: u64 = 1;
const CAP_MAX_GB: u64 = 50;

/// MESH-CONNECT-DIALOG-1 — the connect modal's title for this panel.
const CONNECT_TITLE: &str = "Test Airsonic connection";

#[derive(Debug, Clone, Default)]
pub struct MusicPanel {
    pub server_url: String,
    pub username: String,
    pub password: String,
    /// Human-readable current cache size (e.g. "2.4 GiB").
    pub cache_size: String,
    /// Cache cap in GiB (slider; default 10).
    pub cache_cap_gb: u64,
    pub status: String,
    pub busy: bool,
    /// MESH-CONNECT-DIALOG-1 — the connect/configure progress modal for
    /// the "Test connection" daemon probe (pending → success / failure).
    pub connect: ConnectProgress,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        creds: Option<Creds>,
        cache_size: String,
    },
    UrlChanged(String),
    UserChanged(String),
    PassChanged(String),
    Save,
    TestConnection,
    /// The async ping landed: `Ok(api_version)` connected, `Err(detail)`
    /// failed — resolves the connect modal to success / failure.
    TestResult(Result<String, String>),
    /// MESH-CONNECT-DIALOG-1 — re-run the probe from the modal's failure
    /// state.
    ConnectRetry,
    /// MESH-CONNECT-DIALOG-1 — close the connect modal (Dismiss / backdrop).
    ConnectDismiss,
    CacheCapChanged(u64),
    ClearCache,
    SignOut,
}

impl MusicPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_cap_gb: 10,
            ..Self::default()
        }
    }

    /// Read the saved creds + current cache size off disk.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let creds = creds::load().ok();
                let size = cache::human_bytes(cache::read_index(&cache::cache_dir()).total_bytes());
                Message::Loaded {
                    creds,
                    cache_size: size,
                }
            },
            crate::Message::Music,
        )
    }

    fn cap_bytes(&self) -> u64 {
        self.cache_cap_gb * 1024 * 1024 * 1024
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded { creds, cache_size } => {
                if let Some(c) = creds {
                    self.server_url = c.server_url;
                    self.username = c.username;
                    self.password = c.password;
                }
                self.cache_size = cache_size;
                self.busy = false;
                Task::none()
            }
            Message::UrlChanged(s) => {
                self.server_url = s;
                Task::none()
            }
            Message::UserChanged(s) => {
                self.username = s;
                Task::none()
            }
            Message::PassChanged(s) => {
                self.password = s;
                Task::none()
            }
            Message::Save => {
                if creds::is_valid(&self.server_url, &self.username) {
                    let c = Creds {
                        server_url: self.server_url.trim().to_string(),
                        username: self.username.trim().to_string(),
                        password: self.password.clone(),
                    };
                    self.status = match creds::save(&c) {
                        Ok(()) => "Applied.".to_string(),
                        Err(e) => format!("Couldn't apply: {e}"),
                    };
                } else {
                    self.status = "Enter an http(s):// URL and a username.".to_string();
                }
                Task::none()
            }
            Message::TestConnection | Message::ConnectRetry => {
                if self.busy {
                    return Task::none();
                }
                // Don't probe a half-filled form — show the failure state
                // immediately rather than spinning forever on a bad URL.
                if !creds::is_valid(&self.server_url, &self.username) {
                    self.connect = ConnectProgress::pending(CONNECT_TITLE, "")
                        .failure("Enter an http(s):// URL and a username first.");
                    return Task::none();
                }
                self.busy = true;
                self.connect = ConnectProgress::pending(
                    CONNECT_TITLE,
                    format!("Contacting {}…", self.server_url.trim()),
                );
                let (url, user, pass) = (
                    self.server_url.clone(),
                    self.username.clone(),
                    self.password.clone(),
                );
                Task::perform(
                    async move {
                        let client = Client::new(&url, &user, &pass);
                        let result = match client.ping().await {
                            Ok(v) => Ok(format!("Connected — Airsonic API v{v}.")),
                            Err(e) => Err(format!("Could not connect: {e}")),
                        };
                        Message::TestResult(result)
                    },
                    crate::Message::Music,
                )
            }
            Message::TestResult(result) => {
                self.busy = false;
                // Only resolve a modal that's still in flight: if the operator
                // dismissed the dialog while the ping was outstanding, the stale
                // result must NOT resurrect a closed modal (it would pop back up
                // with an empty title over an unrelated screen).
                if self.connect.is_pending() {
                    self.connect = match result {
                        Ok(msg) => self.connect.success(msg),
                        Err(err) => self.connect.failure(err),
                    };
                }
                Task::none()
            }
            Message::ConnectDismiss => {
                self.connect = ConnectProgress::Closed;
                // Clear busy too: a dismiss during a pending probe must not leave
                // the "Test connection" button disabled (it's gated on !busy) with
                // no modal to re-trigger from.
                self.busy = false;
                Task::none()
            }
            Message::CacheCapChanged(gb) => {
                self.cache_cap_gb = gb.clamp(CAP_MIN_GB, CAP_MAX_GB);
                // Apply eviction immediately so the cache fits the new cap.
                let evicted =
                    cache::run_gc(&cache::cache_dir(), self.cap_bytes()).unwrap_or_default();
                let size = cache::human_bytes(cache::read_index(&cache::cache_dir()).total_bytes());
                self.cache_size = size;
                if !evicted.is_empty() {
                    self.status = format!(
                        "Trimmed {} track(s) to fit {} GiB.",
                        evicted.len(),
                        self.cache_cap_gb
                    );
                }
                Task::none()
            }
            Message::ClearCache => {
                // Evict everything (cap 0) — starred pins still apply.
                let _ = cache::run_gc(&cache::cache_dir(), 0);
                self.cache_size =
                    cache::human_bytes(cache::read_index(&cache::cache_dir()).total_bytes());
                self.status = "Cache cleared.".to_string();
                Task::none()
            }
            Message::SignOut => {
                let path = creds::default_path();
                let _ = std::fs::remove_file(&path);
                self.server_url.clear();
                self.username.clear();
                self.password.clear();
                self.status = "Signed out — creds removed.".to_string();
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let pal = crate::live_theme::palette();
        let m = |msg: Message| crate::Message::Music(msg);

        let password = text_input("password", &self.password)
            .secure(true)
            .on_input(move |s| m(Message::PassChanged(s)))
            .padding(Padding {
                top: 0.0,
                right: 10.0,
                bottom: 0.0,
                left: 10.0,
            })
            .size(13);

        let server = column![
            text("Airsonic server").size(16),
            styled_text_input(
                "https://music.your-mesh:4040",
                &self.server_url,
                move |s| m(Message::UrlChanged(s)),
                pal,
            ),
            styled_text_input(
                "username",
                &self.username,
                move |s| m(Message::UserChanged(s)),
                pal
            ),
            password,
            row![
                variant_button("Apply", ButtonVariant::Primary, Some(m(Message::Save)), pal),
                variant_button(
                    "Test connection",
                    ButtonVariant::Ghost,
                    (!self.busy).then(|| m(Message::TestConnection)),
                    pal,
                ),
            ]
            .spacing(8),
        ]
        .spacing(8);

        let cache_section = column![
            text("Cache").size(16),
            text(format!(
                "Using {} (cap {} GiB)",
                self.cache_size, self.cache_cap_gb
            ))
            .size(13),
            slider(
                (CAP_MIN_GB as u32)..=(CAP_MAX_GB as u32),
                self.cache_cap_gb as u32,
                move |v| m(Message::CacheCapChanged(u64::from(v))),
            ),
            row![
                variant_button(
                    "Clear cache",
                    ButtonVariant::Ghost,
                    Some(m(Message::ClearCache)),
                    pal
                ),
                variant_button(
                    "Sign out",
                    ButtonVariant::Ghost,
                    Some(m(Message::SignOut)),
                    pal
                ),
            ]
            .spacing(8),
        ]
        .spacing(8);

        let body: Element<'_, crate::Message, cosmic::Theme> = container(
            column![
                // CTRLSURF-8 — "Music" is already the page title (app.rs); the
                // panel no longer repeats it as a redundant self-heading.
                server,
                Space::new().height(Length::Fixed(8.0)),
                cache_section,
                text(&self.status).size(13),
            ]
            .spacing(16)
            .padding(20)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

        // MESH-CONNECT-DIALOG-1 — stack the connect/configure progress modal
        // over the panel body when a "Test connection" probe is open.
        connect_progress::overlay(
            &self.connect,
            body,
            pal,
            m(Message::ConnectRetry),
            m(Message::ConnectDismiss),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_pending() -> MusicPanel {
        let mut p = MusicPanel::new();
        // A valid form so TestConnection actually fires the probe (opens Pending).
        p.server_url = "https://music.example".into();
        p.username = "u".into();
        let _ = p.update(Message::TestConnection);
        p
    }

    #[test]
    fn test_connection_opens_the_pending_modal_and_marks_busy() {
        let p = panel_pending();
        assert!(p.busy, "probe in flight => busy");
        assert!(
            p.connect.is_pending(),
            "modal is pending while the probe runs"
        );
    }

    #[test]
    fn invalid_form_fails_fast_without_a_probe() {
        let mut p = MusicPanel::new(); // empty url/user => invalid
        let _ = p.update(Message::TestConnection);
        assert!(!p.busy, "no probe fired for an invalid form");
        assert!(
            matches!(p.connect, ConnectProgress::Failure { .. }),
            "invalid form resolves straight to the failure state"
        );
    }

    #[test]
    fn dismiss_during_pending_clears_busy_and_closes() {
        let mut p = panel_pending();
        let _ = p.update(Message::ConnectDismiss);
        assert!(
            !p.busy,
            "dismiss must clear busy so Test connection re-enables"
        );
        assert!(!p.connect.is_open(), "dismissed modal is closed");
    }

    #[test]
    fn stale_result_after_dismiss_does_not_resurrect_the_modal() {
        let mut p = panel_pending();
        let _ = p.update(Message::ConnectDismiss); // close while the ping is outstanding
                                                   // The in-flight ping finally lands — it must NOT reopen the closed modal.
        let _ = p.update(Message::TestResult(Ok(
            "Connected — Airsonic API v1.16.1.".into()
        )));
        assert!(
            !p.connect.is_open(),
            "a stale result must not resurrect a dismissed modal"
        );
    }

    #[test]
    fn result_while_pending_resolves_the_modal() {
        let mut p = panel_pending();
        let _ = p.update(Message::TestResult(
            Err("Could not connect: refused".into()),
        ));
        assert!(!p.busy);
        assert!(matches!(p.connect, ConnectProgress::Failure { .. }));
    }
}
