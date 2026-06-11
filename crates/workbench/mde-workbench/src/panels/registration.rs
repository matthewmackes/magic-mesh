//! PLANES-4 — the **Registration** panel (Controller plane), the
//! CLI/daemon side of ENT-4 (`mesh init`) + ENT-5 (unified `leave`).
//!
//! This node's enrollment identity + lifecycle in one place: the signing
//! fingerprint (hex) with its human word-pair (W25, from
//! `mackesd identity --json`), the capability tags (W26, from
//! `mackesd tag`), and the cert-lifecycle actions — mint an invite token
//! (W18, `mackesd enroll-token`), re-enroll, and leave (W17). Shells the
//! existing daemon verbs (the established panel pattern).
//!
//! Build-now-defer-visual: the identity-JSON projection is pure +
//! unit-tested; the enroll-with-token text input + the on-Cosmic
//! `/preview` are the deferred tail.

use iced::widget::{column, row, text};
use iced::{Element, Length, Task};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::panel_container;
use crate::panels::fleet_settings::run_mackesd;

/// `mackesd identity --json` document.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Identity {
    pub fingerprint: String,
    pub word_pair: String,
}

/// Parse the identity JSON; tolerant of a garbled/empty body.
#[must_use]
pub fn parse_identity(raw: &str) -> Identity {
    serde_json::from_str(raw).unwrap_or_default()
}

/// The Registration panel state.
#[derive(Debug, Clone, Default)]
pub struct RegistrationPanel {
    pub identity: Identity,
    pub tags: String,
    pub last_token: String,
    pub status: String,
    pub busy: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded { identity: Identity, tags: String },
    Error(String),
    MintTokenClicked,
    TokenMinted(String),
    ReenrollClicked,
    LeaveClicked,
    ActionDone(String),
    RefreshClicked,
}

impl RegistrationPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let identity = match run_mackesd(&["identity".into(), "--json".into()]).await {
                    Ok(out) => parse_identity(&out),
                    Err(_) => Identity::default(),
                };
                let tags = run_mackesd(&["tag".into()])
                    .await
                    .unwrap_or_else(|_| "tags: (unavailable)".into())
                    .trim()
                    .to_string();
                Message::Loaded { identity, tags }
            },
            crate::Message::Registration,
        )
    }

    fn shell(args: Vec<String>, label: &'static str) -> Task<crate::Message> {
        Task::perform(
            async move {
                match run_mackesd(&args).await {
                    Ok(out) if args.first().map(String::as_str) == Some("enroll-token") => {
                        Message::TokenMinted(out.trim().to_string())
                    }
                    Ok(_) => Message::ActionDone(format!("{label}: done")),
                    Err(e) => Message::ActionDone(format!("{label} failed: {e}")),
                }
            },
            crate::Message::Registration,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded { identity, tags } => {
                self.identity = identity;
                self.tags = tags;
                self.loaded = true;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::Error(e) => {
                self.status = e;
                self.busy = false;
                Task::none()
            }
            Message::MintTokenClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Minting a single-use invite token…".into();
                Self::shell(vec!["enroll-token".into()], "mint token")
            }
            Message::TokenMinted(tok) => {
                self.last_token = tok;
                self.busy = false;
                self.status = "Invite token minted — share it with the joining node.".into();
                Task::none()
            }
            Message::ReenrollClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Re-enrolling…".into();
                Self::shell(vec!["reenroll".into()], "re-enroll")
            }
            Message::LeaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Leaving the mesh…".into();
                Self::shell(vec!["leave".into(), "--yes".into()], "leave")
            }
            Message::ActionDone(msg) => {
                self.status = msg;
                self.busy = false;
                Self::load() // refresh identity/tags after a lifecycle change
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        let btn = |label: &'static str, msg: Message| {
            variant_button(
                label,
                ButtonVariant::Ghost,
                (!self.busy).then_some(crate::Message::Registration(msg)),
                palette,
            )
        };

        let fp = if self.identity.fingerprint.is_empty() {
            "(not enrolled / mackesd unavailable)".to_string()
        } else {
            self.identity.fingerprint.clone()
        };

        // PLANES-2 — Registration is the Nebula overlay-identity surface.
        let nebula = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Nebula,
            crate::panel_chrome::pkg_version_cached("nebula").as_deref(),
            palette,
        );
        let mut col = column![
            row![
                text("Registration").size(20),
                iced::widget::Space::new().width(Length::Fill),
                nebula,
            ]
            .align_y(iced::Alignment::Center),
            text("Identity fingerprint").size(14),
            text(fp).size(12).font(iced::Font::MONOSPACE),
            row![
                text("word-pair:").size(14),
                text(if self.identity.word_pair.is_empty() {
                    "—".to_string()
                } else {
                    self.identity.word_pair.clone()
                })
                .size(16),
            ]
            .spacing(8),
            text(self.tags.clone()).size(13),
            row![
                btn("Mint invite token", Message::MintTokenClicked),
                btn("Re-enroll", Message::ReenrollClicked),
                btn("Leave mesh", Message::LeaveClicked),
                variant_button(
                    "Refresh",
                    ButtonVariant::Ghost,
                    (!self.busy).then_some(crate::Message::Registration(Message::RefreshClicked)),
                    palette,
                ),
            ]
            .spacing(12),
        ]
        .spacing(10);

        if !self.last_token.is_empty() {
            col = col.push(text("Invite token (single-use):").size(13));
            col = col.push(
                text(self.last_token.clone())
                    .size(12)
                    .font(iced::Font::MONOSPACE),
            );
        }
        if !self.status.is_empty() {
            col = col.push(text(self.status.clone()).size(13));
        }

        panel_container(col.width(Length::Fill).into(), density)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_identity_reads_fingerprint_and_word_pair() {
        let id = parse_identity(r#"{"fingerprint":"abcd1234","word_pair":"otter-pine"}"#);
        assert_eq!(id.fingerprint, "abcd1234");
        assert_eq!(id.word_pair, "otter-pine");
    }

    #[test]
    fn parse_identity_tolerates_garbage() {
        assert_eq!(parse_identity("not json"), Identity::default());
        assert_eq!(parse_identity(""), Identity::default());
    }

    #[test]
    fn token_minted_stores_and_clears_busy() {
        let mut p = RegistrationPanel::new();
        p.busy = true;
        let _ = p.update(Message::TokenMinted("mesh:m1@10.42.0.1:4242#tok".into()));
        assert_eq!(p.last_token, "mesh:m1@10.42.0.1:4242#tok");
        assert!(!p.busy);
    }

    #[test]
    fn loaded_populates_identity_and_tags() {
        let mut p = RegistrationPanel::new();
        let _ = p.update(Message::Loaded {
            identity: parse_identity(r#"{"fingerprint":"ff","word_pair":"jade-reef"}"#),
            tags: "tags for pine: execution".into(),
        });
        assert!(p.loaded);
        assert_eq!(p.identity.word_pair, "jade-reef");
        assert!(p.tags.contains("execution"));
    }
}
