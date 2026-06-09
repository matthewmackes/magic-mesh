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

use iced::widget::{column, container, row, slider, text, text_input, Space};
use iced::{Element, Length, Padding, Task};

use mde_musicd::airsonic::Client;
use mde_musicd::cache;
use mde_musicd::creds::{self, Creds};
use mde_theme::Palette;

use crate::controls::{styled_text_input, variant_button, ButtonVariant};

/// Cache cap slider bounds (GiB).
const CAP_MIN_GB: u64 = 1;
const CAP_MAX_GB: u64 = 50;

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
    TestResult(String),
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
            Message::TestConnection => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Testing…".to_string();
                let (url, user, pass) = (
                    self.server_url.clone(),
                    self.username.clone(),
                    self.password.clone(),
                );
                Task::perform(
                    async move {
                        let client = Client::new(&url, &user, &pass);
                        let msg = match client.ping().await {
                            Ok(v) => format!("Connected (API v{v})."),
                            Err(e) => format!("Failed: {e}"),
                        };
                        Message::TestResult(msg)
                    },
                    crate::Message::Music,
                )
            }
            Message::TestResult(s) => {
                self.status = s;
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

    pub fn view(&self) -> Element<'_, crate::Message> {
        let pal = Palette::dark();
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

        container(
            column![
                text("Music").size(20),
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
        .into()
    }
}
