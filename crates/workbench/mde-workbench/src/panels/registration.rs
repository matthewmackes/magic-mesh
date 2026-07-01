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

use cosmic::iced::widget::{column, row, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use serde::Deserialize;

use crate::controls::{styled_text_input, variant_button, ButtonVariant};
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
    /// Mesh id the operator mints an invite token for (`EnrollToken`
    /// requires `--mesh-id`; the bare shell failed clap).
    pub mesh_id_input: String,
    /// This node's stable id for re-enroll (`Reenroll` takes a positional
    /// `node_id`); pre-filled with `peer:<hostname>`.
    pub node_id_input: String,
    /// Two-stage confirm for the destructive Leave (arm → confirm).
    pub leave_armed: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded { identity: Identity, tags: String },
    Error(String),
    MintTokenClicked,
    MeshIdChanged(String),
    TokenMinted(String),
    ReenrollClicked,
    NodeIdChanged(String),
    LeaveClicked,
    CancelLeave,
    ActionDone(String),
    RefreshClicked,
}

impl RegistrationPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_id_input: default_node_id(),
            ..Self::default()
        }
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
                let mesh = self.mesh_id_input.trim().to_string();
                if mesh.is_empty() {
                    self.status = "Enter a mesh-id to mint an invite token for.".into();
                    return Task::none();
                }
                self.busy = true;
                self.status = "Minting a single-use invite token…".into();
                // EnrollToken requires --mesh-id; the bare `enroll-token` shell
                // failed clap validation (the panel never minted a token).
                Self::shell(
                    vec!["enroll-token".into(), "--mesh-id".into(), mesh],
                    "mint token",
                )
            }
            Message::MeshIdChanged(s) => {
                self.mesh_id_input = s;
                Task::none()
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
                // Reenroll takes a positional node_id; default to THIS node's
                // stable id (peer:<hostname>) so the bare `reenroll` shell (which
                // failed clap) now resolves to re-enrolling this node.
                let node = {
                    let n = self.node_id_input.trim();
                    if n.is_empty() {
                        default_node_id()
                    } else {
                        n.to_string()
                    }
                };
                self.busy = true;
                self.status = format!("Re-enrolling {node}…");
                Self::shell(vec!["reenroll".into(), node], "re-enroll")
            }
            Message::NodeIdChanged(s) => {
                self.node_id_input = s;
                Task::none()
            }
            Message::LeaveClicked => {
                if self.busy {
                    return Task::none();
                }
                if !self.leave_armed {
                    // Destructive — arm on the first click, fire only on the
                    // second (Confirm). One click used to wipe the node.
                    self.leave_armed = true;
                    self.status = "Leave wipes /etc/nebula, keys, and the pinned role on THIS \
                                   node. Click Confirm leave to proceed."
                        .into();
                    return Task::none();
                }
                self.leave_armed = false;
                self.busy = true;
                self.status = "Leaving the mesh…".into();
                Self::shell(vec!["leave".into(), "--yes".into()], "leave")
            }
            Message::CancelLeave => {
                self.leave_armed = false;
                self.status.clear();
                Task::none()
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
        let wrap = |msg: Message| crate::Message::Registration(msg);
        let mesh_input = styled_text_input(
            "mesh-id (e.g. home-mesh)",
            &self.mesh_id_input,
            move |s| wrap(Message::MeshIdChanged(s)),
            palette,
        );
        let node_input = styled_text_input(
            "this node's id (defaults to peer:hostname)",
            &self.node_id_input,
            move |s| wrap(Message::NodeIdChanged(s)),
            palette,
        );
        let mint_row = row![
            mesh_input,
            btn("Mint invite token", Message::MintTokenClicked)
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center);
        let reenroll_row = row![node_input, btn("Re-enroll", Message::ReenrollClicked)]
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);
        let leave_row: Element<'_, crate::Message> = if self.leave_armed {
            row![
                text("⚠ Confirm — leave wipes /etc/nebula, keys + role").size(13),
                variant_button(
                    "Confirm leave",
                    ButtonVariant::Primary,
                    (!self.busy).then_some(wrap(Message::LeaveClicked)),
                    palette,
                ),
                btn("Cancel", Message::CancelLeave),
            ]
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center)
            .into()
        } else {
            btn("Leave mesh", Message::LeaveClicked).into()
        };

        let mut col = column![
            // CTRLSURF-8 — "Registration" is already the page title (app.rs);
            // keep the Nebula identity hero, right-aligned, without repeating it.
            row![
                cosmic::iced::widget::Space::new().width(Length::Fill),
                nebula,
            ]
            .align_y(cosmic::iced::Alignment::Center),
            text("Identity fingerprint").size(14),
            text(fp).size(12).font(cosmic::iced::Font::MONOSPACE),
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
            mint_row,
            reenroll_row,
            leave_row,
            variant_button(
                "Refresh",
                ButtonVariant::Ghost,
                (!self.busy).then_some(crate::Message::Registration(Message::RefreshClicked)),
                palette,
            ),
        ]
        .spacing(10);

        if !self.last_token.is_empty() {
            col = col.push(text("Invite token (single-use):").size(13));
            col = col.push(
                text(self.last_token.clone())
                    .size(12)
                    .font(cosmic::iced::Font::MONOSPACE),
            );
        }
        if !self.status.is_empty() {
            col = col.push(text(self.status.clone()).size(13));
        }

        panel_container(col.width(Length::Fill).into(), density)
    }
}

/// This node's stable id, matching mackesd's `peer:<hostname>` default
/// (`Reconcile`/`default_node_id`). Reads the kernel hostname directly so the
/// GUI never re-derives node-id logic that could diverge from the daemon.
fn default_node_id() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into());
    format!("peer:{host}")
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

    #[test]
    fn new_prefills_node_id_with_peer_prefix() {
        assert!(RegistrationPanel::new().node_id_input.starts_with("peer:"));
    }

    #[test]
    fn leave_first_click_arms_without_firing() {
        let mut p = RegistrationPanel::new();
        let _ = p.update(Message::LeaveClicked);
        assert!(p.leave_armed, "first click must arm, not leave");
        assert!(!p.busy, "no leave fired on the arming click");
    }

    #[test]
    fn leave_second_click_fires() {
        let mut p = RegistrationPanel::new();
        let _ = p.update(Message::LeaveClicked); // arm
        let _ = p.update(Message::LeaveClicked); // confirm
        assert!(!p.leave_armed);
        assert!(p.busy, "confirm fires the leave");
    }

    #[test]
    fn cancel_leave_disarms() {
        let mut p = RegistrationPanel::new();
        let _ = p.update(Message::LeaveClicked); // arm
        let _ = p.update(Message::CancelLeave);
        assert!(!p.leave_armed);
        assert!(!p.busy);
    }

    #[test]
    fn mint_without_mesh_id_is_a_noop() {
        let mut p = RegistrationPanel::new();
        p.mesh_id_input.clear();
        let _ = p.update(Message::MintTokenClicked);
        assert!(!p.busy, "no mint fires without a mesh-id");
        assert!(p.status.to_lowercase().contains("mesh-id"));
    }

    #[test]
    fn reenroll_defaults_to_this_node_when_input_blank() {
        let mut p = RegistrationPanel::new();
        p.node_id_input.clear();
        let _ = p.update(Message::ReenrollClicked);
        // a blank input falls back to peer:<hostname>, so the action fires
        assert!(p.busy, "re-enroll fires with the derived node id");
        assert!(p.status.contains("peer:"));
    }
}
