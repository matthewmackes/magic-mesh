//! TUNE-15.b — Network → Mesh Federation panel.
//!
//! 4-tab accept-pair surface: Mint / Accept / Grant Tuning / Active Pairs.
//! Ships the Workbench leg of the out-of-band passcode pairing protocol
//! defined in docs/design/v1.0-federation-pairing.md §4.
//!
//! Cite: docs/design/v1.0-federation-pairing.md §4;
//! ref: Linear (settings multi-pane flow).

use std::path::PathBuf;

use iced::widget::button::Status as ButtonStatus;
use iced::widget::{button, column, row, text, text_input, Space};
use iced::{alignment, Background, Border, Color, Element, Length, Task};
use mde_theme::{Density, EmptyState, FontSize, Icon, Palette, Radii, TypeRole};

use crate::panel_chrome::{empty_state, panel_container};

// ─── YAML / JSON mirror types ────────────────────────────────────────────────
// Local mirrors so this crate does not need a hard dep on mde-bus.
// Field names match the federation.yaml schema from the design doc §2.

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct FederationYaml {
    #[serde(default)]
    pairs: Vec<FederationPairYaml>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct FederationPairYaml {
    #[serde(rename = "peer-mesh-id", default)]
    pub peer_mesh_id: String,
    #[serde(rename = "peer-mesh-label", default)]
    pub peer_mesh_label: String,
    #[serde(default)]
    pub established: String,
    #[serde(rename = "subscribe-topics", default)]
    pub subscribe_topics: Vec<String>,
    #[serde(rename = "publish-topics", default)]
    pub publish_topics: Vec<String>,
    #[serde(rename = "excluded-topics", default)]
    pub excluded_topics: Vec<String>,
}

// JSON envelope returned by `mde-bus federation mint-passcode --json`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MintJsonOutput {
    #[serde(default)]
    mnemonic: String,
    #[serde(default)]
    ulid: String,
    #[serde(rename = "expires_at_unix_ms", default)]
    expires_at_unix_ms: i64,
}

// Result of a successful accept operation.
#[derive(Debug, Clone)]
pub struct AcceptResult {
    peer_mesh_id: String,
    peer_mesh_label: String,
}

// ─── Tab enum ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Mint,
    Accept,
    Grant,
    Pairs,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Mint => "Mint",
            Self::Accept => "Accept",
            Self::Grant => "Grant Tuning",
            Self::Pairs => "Active Pairs",
        }
    }

    const ALL: [Tab; 4] = [Tab::Mint, Tab::Accept, Tab::Grant, Tab::Pairs];
}

// ─── Per-tab state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct MintTabState {
    pub mnemonic: Option<String>,
    pub ulid: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub loading: bool,
    pub revoking: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AcceptTabState {
    pub input: String,
    pub submitting: bool,
    pub error: Option<String>,
    pub success: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GrantTabState {
    pub peer_mesh_id: Option<String>,
    pub peer_label: Option<String>,
    pub subscribe_topics: Vec<String>,
    pub new_subscribe: String,
    pub publish_topics: Vec<String>,
    pub new_publish: String,
    pub saving: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PairsTabState {
    pub pairs: Vec<FederationPairYaml>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub revoking: Option<String>,
    pub rotating: Option<String>,
}

// ─── Panel struct ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct MeshFederationPanel {
    pub active_tab: Tab,
    pub mint: MintTabState,
    pub accept: AcceptTabState,
    pub grant: GrantTabState,
    pub pairs: PairsTabState,
}

// ─── Messages ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    SelectTab(Tab),
    // Mint tab
    MintClicked,
    MintDone(Result<MintJsonOutput, String>),
    RevokeClicked,
    RevokeDone(Result<(), String>),
    // Accept tab
    AcceptInputChanged(String),
    AcceptSubmitClicked,
    AcceptDone(Result<AcceptResult, String>),
    // Grant tab
    GrantNewSubscribeChanged(String),
    GrantAddSubscribeClicked,
    GrantRemoveSubscribe(String),
    GrantNewPublishChanged(String),
    GrantAddPublishClicked,
    GrantRemovePublish(String),
    GrantSaveClicked,
    GrantSaveDone(Result<(), String>),
    // Pairs tab
    PairsLoaded(Result<Vec<FederationPairYaml>, String>),
    PairsRefreshClicked,
    PairsRevokeClicked(String),
    PairsRevokeDone(Result<String, String>),
    PairsRotateClicked(String),
    PairsRotateDone(Result<String, String>),
}

// ─── Async helpers ────────────────────────────────────────────────────────────

fn federation_data_root() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .map(|d| d.join("mde").join("bus"))
}

async fn mint_passcode() -> Result<MintJsonOutput, String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["federation", "mint-passcode", "--json"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if let Ok(parsed) = serde_json::from_str::<MintJsonOutput>(&stdout) {
        return Ok(parsed);
    }
    // Plain mnemonic fallback (CLI predates --json flag).
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    Ok(MintJsonOutput {
        mnemonic: stdout.trim().to_string(),
        ulid: String::new(),
        expires_at_unix_ms: now_ms + 24 * 3600 * 1000,
    })
}

async fn revoke_mint(ulid: String) -> Result<(), String> {
    if ulid.is_empty() {
        return Err("No mint ULID recorded — passcode may have already expired.".to_string());
    }
    let out = tokio::process::Command::new("mde-bus")
        .args(["federation", "revoke-mint", &ulid])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

async fn accept_passcode(passcode: String) -> Result<AcceptResult, String> {
    let word_count = passcode.split_whitespace().count();
    if word_count != 6 {
        return Err(format!(
            "Enter exactly 6 BIP-39 words ({word_count} entered)."
        ));
    }
    let out = tokio::process::Command::new("mde-bus")
        .args(["federation", "accept", &passcode, "--json"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (id, label) = serde_json::from_str::<serde_json::Value>(&stdout)
        .map(|v| {
            (
                v["peer-mesh-id"].as_str().unwrap_or("").to_string(),
                v["peer-mesh-label"]
                    .as_str()
                    .unwrap_or("Remote mesh")
                    .to_string(),
            )
        })
        .unwrap_or_else(|_| (String::new(), "Remote mesh".to_string()));
    Ok(AcceptResult {
        peer_mesh_id: id,
        peer_mesh_label: label,
    })
}

async fn load_federation_pairs() -> Result<Vec<FederationPairYaml>, String> {
    let root = federation_data_root().ok_or_else(|| "no data dir".to_string())?;
    let path = root.join("federation.yaml");
    let txt = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let yaml: FederationYaml = serde_yaml::from_str(&txt).unwrap_or_default();
    Ok(yaml.pairs)
}

async fn revoke_pair(peer_mesh_id: String) -> Result<String, String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["federation", "revoke", &peer_mesh_id])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(peer_mesh_id)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

async fn rotate_pair(peer_mesh_id: String) -> Result<String, String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["federation", "rotate", &peer_mesh_id])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(peer_mesh_id)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

async fn save_grants(
    peer_mesh_id: String,
    subscribe_topics: Vec<String>,
    publish_topics: Vec<String>,
) -> Result<(), String> {
    let root = federation_data_root().ok_or_else(|| "no data dir".to_string())?;
    let path = root.join("federation.yaml");
    let txt = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut yaml: FederationYaml = serde_yaml::from_str(&txt).unwrap_or_default();

    if let Some(pair) = yaml
        .pairs
        .iter_mut()
        .find(|p| p.peer_mesh_id == peer_mesh_id)
    {
        pair.subscribe_topics = subscribe_topics;
        pair.publish_topics = publish_topics;
        let out = serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?;
        tokio::fs::write(&path, out.as_bytes())
            .await
            .map_err(|e| e.to_string())
    } else {
        // Pair not in federation.yaml yet (TUNE-15.c daemon side not complete).
        // Persist as a pending-grant sidecar the daemon can apply on pair establishment.
        #[derive(serde::Serialize)]
        struct PendingGrant<'a> {
            #[serde(rename = "peer-mesh-id")]
            peer_mesh_id: &'a str,
            #[serde(rename = "subscribe-topics")]
            subscribe_topics: &'a [String],
            #[serde(rename = "publish-topics")]
            publish_topics: &'a [String],
        }
        let pending = PendingGrant {
            peer_mesh_id: &peer_mesh_id,
            subscribe_topics: &subscribe_topics,
            publish_topics: &publish_topics,
        };
        let pending_path = root.join("federation-pending-grant.yaml");
        let out = serde_yaml::to_string(&pending).map_err(|e| e.to_string())?;
        if let Some(parent) = pending_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&pending_path, out.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }
}

fn format_expiry(expires_at_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let remaining_s = (expires_at_ms - now_ms).max(0) / 1000;
    if remaining_s == 0 {
        return "expired".to_string();
    }
    let h = remaining_s / 3600;
    let m = (remaining_s % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m remaining")
    } else {
        format!("{m}m remaining")
    }
}

// ─── Panel impl ───────────────────────────────────────────────────────────────

impl MeshFederationPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(load_federation_pairs(), |r| {
            crate::Message::MeshFederation(Message::PairsLoaded(r))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::SelectTab(tab) => {
                self.active_tab = tab;
                if tab == Tab::Pairs && !self.pairs.loaded && !self.pairs.loading {
                    self.pairs.loading = true;
                    return Task::perform(load_federation_pairs(), |r| {
                        crate::Message::MeshFederation(Message::PairsLoaded(r))
                    });
                }
                Task::none()
            }

            // ─── Mint ───────────────────────────────────────────────────────
            Message::MintClicked => {
                self.mint.loading = true;
                self.mint.error = None;
                Task::perform(mint_passcode(), |r| {
                    crate::Message::MeshFederation(Message::MintDone(r))
                })
            }

            Message::MintDone(result) => {
                self.mint.loading = false;
                match result {
                    Ok(output) => {
                        self.mint.mnemonic = Some(output.mnemonic);
                        self.mint.ulid = Some(output.ulid).filter(|s| !s.is_empty());
                        self.mint.expires_at_ms =
                            Some(output.expires_at_unix_ms).filter(|&ms| ms > 0);
                        self.mint.error = None;
                    }
                    Err(e) => {
                        self.mint.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::RevokeClicked => {
                let ulid = self.mint.ulid.clone().unwrap_or_default();
                self.mint.revoking = true;
                Task::perform(revoke_mint(ulid), |r| {
                    crate::Message::MeshFederation(Message::RevokeDone(r))
                })
            }

            Message::RevokeDone(result) => {
                self.mint.revoking = false;
                match result {
                    Ok(()) => {
                        self.mint.mnemonic = None;
                        self.mint.ulid = None;
                        self.mint.expires_at_ms = None;
                        self.mint.error = None;
                    }
                    Err(e) => self.mint.error = Some(e),
                }
                Task::none()
            }

            // ─── Accept ─────────────────────────────────────────────────────
            Message::AcceptInputChanged(s) => {
                self.accept.input = s;
                Task::none()
            }

            Message::AcceptSubmitClicked => {
                let passcode = self.accept.input.trim().to_string();
                self.accept.submitting = true;
                self.accept.error = None;
                self.accept.success = None;
                Task::perform(accept_passcode(passcode), |r| {
                    crate::Message::MeshFederation(Message::AcceptDone(r))
                })
            }

            Message::AcceptDone(result) => {
                self.accept.submitting = false;
                match result {
                    Ok(accepted) => {
                        self.accept.input.clear();
                        self.accept.success = Some(format!(
                            "Paired with \"{}\" — configure access in Grant Tuning.",
                            accepted.peer_mesh_label
                        ));
                        // Transition to Grant tab with default subscribe access.
                        self.active_tab = Tab::Grant;
                        self.grant.peer_mesh_id = Some(accepted.peer_mesh_id);
                        self.grant.peer_label = Some(accepted.peer_mesh_label);
                        if self.grant.subscribe_topics.is_empty() {
                            self.grant.subscribe_topics = vec!["#".to_string()];
                        }
                    }
                    Err(e) => self.accept.error = Some(e),
                }
                Task::none()
            }

            // ─── Grant ──────────────────────────────────────────────────────
            Message::GrantNewSubscribeChanged(s) => {
                self.grant.new_subscribe = s;
                Task::none()
            }

            Message::GrantAddSubscribeClicked => {
                let topic = self.grant.new_subscribe.trim().to_string();
                if !topic.is_empty() && !self.grant.subscribe_topics.contains(&topic) {
                    self.grant.subscribe_topics.push(topic);
                    self.grant.new_subscribe.clear();
                }
                Task::none()
            }

            Message::GrantRemoveSubscribe(topic) => {
                self.grant.subscribe_topics.retain(|t| t != &topic);
                Task::none()
            }

            Message::GrantNewPublishChanged(s) => {
                self.grant.new_publish = s;
                Task::none()
            }

            Message::GrantAddPublishClicked => {
                let topic = self.grant.new_publish.trim().to_string();
                if !topic.is_empty() && !self.grant.publish_topics.contains(&topic) {
                    self.grant.publish_topics.push(topic);
                    self.grant.new_publish.clear();
                }
                Task::none()
            }

            Message::GrantRemovePublish(topic) => {
                self.grant.publish_topics.retain(|t| t != &topic);
                Task::none()
            }

            Message::GrantSaveClicked => {
                let peer_id = self.grant.peer_mesh_id.clone().unwrap_or_default();
                let subs = self.grant.subscribe_topics.clone();
                let pubs = self.grant.publish_topics.clone();
                self.grant.saving = true;
                self.grant.error = None;
                Task::perform(save_grants(peer_id, subs, pubs), |r| {
                    crate::Message::MeshFederation(Message::GrantSaveDone(r))
                })
            }

            Message::GrantSaveDone(result) => {
                self.grant.saving = false;
                if let Err(e) = result {
                    self.grant.error = Some(e);
                }
                Task::none()
            }

            // ─── Pairs ──────────────────────────────────────────────────────
            Message::PairsLoaded(result) => {
                self.pairs.loading = false;
                self.pairs.loaded = true;
                match result {
                    Ok(pairs) => {
                        self.pairs.pairs = pairs;
                        self.pairs.error = None;
                    }
                    Err(e) => self.pairs.error = Some(e),
                }
                Task::none()
            }

            Message::PairsRefreshClicked => {
                self.pairs.loaded = false;
                self.pairs.loading = true;
                Task::perform(load_federation_pairs(), |r| {
                    crate::Message::MeshFederation(Message::PairsLoaded(r))
                })
            }

            Message::PairsRevokeClicked(peer_id) => {
                self.pairs.revoking = Some(peer_id.clone());
                Task::perform(revoke_pair(peer_id), |r| {
                    crate::Message::MeshFederation(Message::PairsRevokeDone(r))
                })
            }

            Message::PairsRevokeDone(result) => {
                match result {
                    Ok(revoked_id) => {
                        if self.pairs.revoking.as_deref() == Some(&revoked_id) {
                            self.pairs.revoking = None;
                        }
                        self.pairs.loaded = false;
                        self.pairs.loading = true;
                        return Task::perform(load_federation_pairs(), |r| {
                            crate::Message::MeshFederation(Message::PairsLoaded(r))
                        });
                    }
                    Err(e) => {
                        self.pairs.revoking = None;
                        self.pairs.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::PairsRotateClicked(peer_id) => {
                self.pairs.rotating = Some(peer_id.clone());
                Task::perform(rotate_pair(peer_id), |r| {
                    crate::Message::MeshFederation(Message::PairsRotateDone(r))
                })
            }

            Message::PairsRotateDone(result) => {
                match result {
                    Ok(rotated_id) => {
                        if self.pairs.rotating.as_deref() == Some(&rotated_id) {
                            self.pairs.rotating = None;
                        }
                    }
                    Err(e) => {
                        self.pairs.rotating = None;
                        self.pairs.error = Some(e);
                    }
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = Density::Comfortable;
        let sizes = FontSize::defaults();
        let radii = Radii::defaults();
        let accent = palette.accent.into_iced_color();
        let raised = palette.raised.into_iced_color();

        let title = text("Mesh Federation")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let subtitle = text("Pair independent meshes via out-of-band passcode exchange")
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let tab_bar: Element<'_, crate::Message> = {
            let r = f32::from(radii.sm);
            let buttons: Vec<Element<'_, crate::Message>> = Tab::ALL
                .iter()
                .map(|&tab| {
                    let is_active = tab == self.active_tab;
                    let (bg, fg) = if is_active {
                        (accent, Color::WHITE)
                    } else {
                        (Color::TRANSPARENT, palette.text.into_iced_color())
                    };
                    button(
                        text(tab.label())
                            .size(TypeRole::Body.size_in(sizes))
                            .color(fg),
                    )
                    .padding([6u16, 14u16])
                    .style(move |_t, status: ButtonStatus| {
                        let fill = match (is_active, status) {
                            (true, _) => bg,
                            (false, ButtonStatus::Hovered) => Color {
                                r: accent.r,
                                g: accent.g,
                                b: accent.b,
                                a: 0.08,
                            },
                            _ => bg,
                        };
                        button::Style {
                            snap: false,
                            background: Some(Background::Color(fill)),
                            text_color: fg,
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: r.into(),
                            },
                            shadow: iced::Shadow::default(),
                        }
                    })
                    .on_press(crate::Message::MeshFederation(Message::SelectTab(tab)))
                    .into()
                })
                .collect();
            row(buttons).spacing(4).into()
        };

        let tab_separator = {
            use iced::widget::container;
            container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                .style(move |_t: &iced::Theme| iced::widget::container::Style {
                    snap: false,
                    background: Some(Background::Color(raised)),
                    ..Default::default()
                })
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
        };

        let body: Element<'_, crate::Message> = match self.active_tab {
            Tab::Mint => self.view_mint_tab(palette, sizes, radii),
            Tab::Accept => self.view_accept_tab(palette, sizes, radii),
            Tab::Grant => self.view_grant_tab(palette, sizes, radii),
            Tab::Pairs => self.view_pairs_tab(palette, sizes, radii),
        };

        let header = column![title, subtitle].spacing(4);

        let content = column![
            header,
            Space::new().height(12),
            tab_bar,
            tab_separator,
            Space::new().height(16),
            body,
        ]
        .spacing(0)
        .align_x(alignment::Horizontal::Left);

        panel_container(content.into(), density)
    }

    fn view_mint_tab(
        &self,
        palette: Palette,
        sizes: FontSize,
        radii: Radii,
    ) -> Element<'_, crate::Message> {
        let accent = palette.accent.into_iced_color();
        let text_color = palette.text.into_iced_color();
        let text_muted = palette.text_muted.into_iced_color();
        let r = f32::from(radii.sm);

        let heading = text("Mint a passcode for sharing")
            .size(TypeRole::Subheading.size_in(sizes))
            .color(text_color);

        let hint = text(
            "Read the 6-word passcode to the remote mesh operator over a side channel. \
             Valid for 24 hours; single-use.",
        )
        .size(TypeRole::Caption.size_in(sizes))
        .color(text_muted);

        let mint_label = if self.mint.mnemonic.is_some() {
            "Regenerate"
        } else {
            "Mint passcode"
        };

        let mint_btn: Element<'_, crate::Message> = button(
            text(if self.mint.loading {
                "Minting…"
            } else {
                mint_label
            })
            .size(TypeRole::Body.size_in(sizes))
            .color(Color::WHITE),
        )
        .padding([8u16, 20u16])
        .style(move |_t, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(accent)),
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(crate::Message::MeshFederation(Message::MintClicked))
        .into();

        let mut items: Vec<Element<'_, crate::Message>> = vec![
            heading.into(),
            Space::new().height(4).into(),
            hint.into(),
            Space::new().height(12).into(),
            mint_btn,
        ];

        if let Some(mnemonic) = &self.mint.mnemonic {
            items.push(Space::new().height(20).into());
            items.push(
                text(mnemonic.as_str())
                    .size(TypeRole::Display.size_in(sizes))
                    .color(text_color)
                    .into(),
            );
            if let Some(ms) = self.mint.expires_at_ms {
                items.push(Space::new().height(6).into());
                items.push(
                    text(format_expiry(ms))
                        .size(TypeRole::Caption.size_in(sizes))
                        .color(text_muted)
                        .into(),
                );
            }
            items.push(Space::new().height(12).into());
            let revoke_btn: Element<'_, crate::Message> = button(
                text(if self.mint.revoking {
                    "Revoking…"
                } else {
                    "Revoke"
                })
                .size(TypeRole::Body.size_in(sizes))
                .color(Color {
                    r: 0.9,
                    g: 0.2,
                    b: 0.2,
                    a: 1.0,
                }),
            )
            .padding([6u16, 14u16])
            .style(move |_t, _s: ButtonStatus| button::Style {
                snap: false,
                background: Some(Background::Color(Color {
                    r: 0.8,
                    g: 0.1,
                    b: 0.1,
                    a: 0.12,
                })),
                text_color: Color {
                    r: 0.9,
                    g: 0.2,
                    b: 0.2,
                    a: 1.0,
                },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(crate::Message::MeshFederation(Message::RevokeClicked))
            .into();
            items.push(revoke_btn);
        }

        if let Some(e) = &self.mint.error {
            items.push(Space::new().height(12).into());
            items.push(
                text(format!("Error: {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(Color {
                        r: 0.9,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    })
                    .into(),
            );
        }

        column(items).spacing(0).into()
    }

    fn view_accept_tab(
        &self,
        palette: Palette,
        sizes: FontSize,
        radii: Radii,
    ) -> Element<'_, crate::Message> {
        let accent = palette.accent.into_iced_color();
        let text_color = palette.text.into_iced_color();
        let text_muted = palette.text_muted.into_iced_color();
        let r = f32::from(radii.sm);

        let heading = text("Accept passcode from peer mesh")
            .size(TypeRole::Subheading.size_in(sizes))
            .color(text_color);

        let hint = text("Enter the 6 BIP-39 words given by the remote operator, space-separated.")
            .size(TypeRole::Caption.size_in(sizes))
            .color(text_muted);

        let word_count = self.accept.input.split_whitespace().count();
        let word_hint = if word_count > 0 {
            format!("{word_count}/6 words entered")
        } else {
            String::new()
        };

        let input: Element<'_, crate::Message> =
            text_input("apple bridge clutch dance echo fern", &self.accept.input)
                .on_input(|s| crate::Message::MeshFederation(Message::AcceptInputChanged(s)))
                .on_submit(crate::Message::MeshFederation(Message::AcceptSubmitClicked))
                .width(Length::Fixed(380.0))
                .into();

        let accept_btn: Element<'_, crate::Message> = button(
            text(if self.accept.submitting {
                "Accepting…"
            } else {
                "Accept"
            })
            .size(TypeRole::Body.size_in(sizes))
            .color(Color::WHITE),
        )
        .padding([8u16, 20u16])
        .style(move |_t, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(accent)),
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(crate::Message::MeshFederation(Message::AcceptSubmitClicked))
        .into();

        let input_row: Element<'_, crate::Message> = row![input, Space::new().width(8), accept_btn]
            .align_y(iced::Alignment::Center)
            .into();

        let mut items: Vec<Element<'_, crate::Message>> = vec![
            heading.into(),
            Space::new().height(4).into(),
            hint.into(),
            Space::new().height(12).into(),
            input_row,
        ];

        if !word_hint.is_empty() {
            items.push(Space::new().height(4).into());
            items.push(
                text(word_hint)
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_muted)
                    .into(),
            );
        }

        if let Some(s) = &self.accept.success {
            items.push(Space::new().height(12).into());
            items.push(
                text(s.as_str())
                    .size(TypeRole::Body.size_in(sizes))
                    .color(text_color)
                    .into(),
            );
        }

        if let Some(e) = &self.accept.error {
            items.push(Space::new().height(12).into());
            items.push(
                text(format!("Error: {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(Color {
                        r: 0.9,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    })
                    .into(),
            );
        }

        column(items).spacing(0).into()
    }

    fn view_grant_tab(
        &self,
        palette: Palette,
        sizes: FontSize,
        radii: Radii,
    ) -> Element<'_, crate::Message> {
        let accent = palette.accent.into_iced_color();
        let text_color = palette.text.into_iced_color();
        let text_muted = palette.text_muted.into_iced_color();
        let raised = palette.raised.into_iced_color();
        let r = f32::from(radii.sm);

        if self.grant.peer_mesh_id.is_none() {
            return empty_state(
                EmptyState::info(
                    "No pairing in progress",
                    "Accept a passcode on the Accept tab first, then configure access here.",
                )
                .with_icon(Icon::Network),
                palette,
                || crate::Message::MeshFederation(Message::SelectTab(Tab::Accept)),
            );
        }

        let context_label = if let Some(label) = &self.grant.peer_label {
            format!("Configuring access for: {label}")
        } else {
            "Configuring access for new pair".to_string()
        };

        let context = text(context_label)
            .size(TypeRole::Subheading.size_in(sizes))
            .color(text_color);

        // ─ Subscribe topics ───────────────────────────────────────────────

        let sub_heading = text("Subscribe topics")
            .size(TypeRole::Body.size_in(sizes))
            .color(text_color);

        let sub_hint =
            text("Topic patterns this peer can receive from the remote mesh. Default: # (all).")
                .size(TypeRole::Caption.size_in(sizes))
                .color(text_muted);

        let sub_rows: Vec<Element<'_, crate::Message>> = if self.grant.subscribe_topics.is_empty() {
            vec![text("No patterns — peer receives nothing.")
                .size(TypeRole::Caption.size_in(sizes))
                .color(text_muted)
                .into()]
        } else {
            self.grant
                .subscribe_topics
                .iter()
                .map(|t| {
                    let t2 = t.clone();
                    let rm_btn: Element<'_, crate::Message> =
                        button(text("Remove").size(TypeRole::Caption.size_in(sizes)).color(
                            Color {
                                r: 0.9,
                                g: 0.2,
                                b: 0.2,
                                a: 1.0,
                            },
                        ))
                        .padding([2u16, 8u16])
                        .style(move |_t, _s: ButtonStatus| button::Style {
                            snap: false,
                            background: Some(Background::Color(Color {
                                r: 0.8,
                                g: 0.1,
                                b: 0.1,
                                a: 0.12,
                            })),
                            text_color: Color {
                                r: 0.9,
                                g: 0.2,
                                b: 0.2,
                                a: 1.0,
                            },
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: r.into(),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(crate::Message::MeshFederation(
                            Message::GrantRemoveSubscribe(t2),
                        ))
                        .into();
                    row![
                        text(t.as_str())
                            .size(TypeRole::Body.size_in(sizes))
                            .color(text_color),
                        Space::new().width(Length::Fill),
                        rm_btn,
                    ]
                    .align_y(iced::Alignment::Center)
                    .into()
                })
                .collect()
        };

        let sub_list: Element<'_, crate::Message> = column(sub_rows).spacing(4).into();

        let sub_input: Element<'_, crate::Message> =
            text_input("#, fleet/#, mon/+/alerts", &self.grant.new_subscribe)
                .on_input(|s| crate::Message::MeshFederation(Message::GrantNewSubscribeChanged(s)))
                .on_submit(crate::Message::MeshFederation(
                    Message::GrantAddSubscribeClicked,
                ))
                .width(Length::Fixed(260.0))
                .into();

        let sub_add_btn: Element<'_, crate::Message> = button(
            text("Add")
                .size(TypeRole::Body.size_in(sizes))
                .color(text_color),
        )
        .padding([6u16, 14u16])
        .style(move |_t, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(raised)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(crate::Message::MeshFederation(
            Message::GrantAddSubscribeClicked,
        ))
        .into();

        let sub_add_row: Element<'_, crate::Message> =
            row![sub_input, Space::new().width(8), sub_add_btn]
                .align_y(iced::Alignment::Center)
                .into();

        let excluded_note = text(
            "Always excluded: passcode/*, federation/*, clipboard/*, voip/presence/*, input/*",
        )
        .size(TypeRole::Caption.size_in(sizes))
        .color(text_muted);

        // ─ Publish topics ─────────────────────────────────────────────────

        let pub_heading = text("Publish topics")
            .size(TypeRole::Body.size_in(sizes))
            .color(text_color);

        let pub_hint =
            text("Topic patterns the remote mesh can write into this mesh. Default: empty.")
                .size(TypeRole::Caption.size_in(sizes))
                .color(text_muted);

        let pub_rows: Vec<Element<'_, crate::Message>> = if self.grant.publish_topics.is_empty() {
            vec![text("No patterns — remote mesh can publish nothing.")
                .size(TypeRole::Caption.size_in(sizes))
                .color(text_muted)
                .into()]
        } else {
            self.grant
                .publish_topics
                .iter()
                .map(|t| {
                    let t2 = t.clone();
                    let rm_btn: Element<'_, crate::Message> =
                        button(text("Remove").size(TypeRole::Caption.size_in(sizes)).color(
                            Color {
                                r: 0.9,
                                g: 0.2,
                                b: 0.2,
                                a: 1.0,
                            },
                        ))
                        .padding([2u16, 8u16])
                        .style(move |_t, _s: ButtonStatus| button::Style {
                            snap: false,
                            background: Some(Background::Color(Color {
                                r: 0.8,
                                g: 0.1,
                                b: 0.1,
                                a: 0.12,
                            })),
                            text_color: Color {
                                r: 0.9,
                                g: 0.2,
                                b: 0.2,
                                a: 1.0,
                            },
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: r.into(),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(crate::Message::MeshFederation(Message::GrantRemovePublish(
                            t2,
                        )))
                        .into();
                    row![
                        text(t.as_str())
                            .size(TypeRole::Body.size_in(sizes))
                            .color(text_color),
                        Space::new().width(Length::Fill),
                        rm_btn,
                    ]
                    .align_y(iced::Alignment::Center)
                    .into()
                })
                .collect()
        };

        let pub_list: Element<'_, crate::Message> = column(pub_rows).spacing(4).into();

        let pub_input: Element<'_, crate::Message> =
            text_input("portal/peer-presence/*", &self.grant.new_publish)
                .on_input(|s| crate::Message::MeshFederation(Message::GrantNewPublishChanged(s)))
                .on_submit(crate::Message::MeshFederation(
                    Message::GrantAddPublishClicked,
                ))
                .width(Length::Fixed(260.0))
                .into();

        let pub_add_btn: Element<'_, crate::Message> = button(
            text("Add publish grant")
                .size(TypeRole::Body.size_in(sizes))
                .color(text_color),
        )
        .padding([6u16, 14u16])
        .style(move |_t, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(raised)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(crate::Message::MeshFederation(
            Message::GrantAddPublishClicked,
        ))
        .into();

        let pub_add_row: Element<'_, crate::Message> =
            row![pub_input, Space::new().width(8), pub_add_btn]
                .align_y(iced::Alignment::Center)
                .into();

        // ─ Save ───────────────────────────────────────────────────────────

        let save_btn: Element<'_, crate::Message> = button(
            text(if self.grant.saving {
                "Applying…"
            } else {
                "Apply grants"
            })
            .size(TypeRole::Body.size_in(sizes))
            .color(Color::WHITE),
        )
        .padding([8u16, 20u16])
        .style(move |_t, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(accent)),
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(crate::Message::MeshFederation(Message::GrantSaveClicked))
        .into();

        let mut col = column![
            context,
            Space::new().height(20),
            sub_heading,
            Space::new().height(4),
            sub_hint,
            Space::new().height(8),
            sub_list,
            Space::new().height(8),
            sub_add_row,
            Space::new().height(6),
            excluded_note,
            Space::new().height(24),
            pub_heading,
            Space::new().height(4),
            pub_hint,
            Space::new().height(8),
            pub_list,
            Space::new().height(8),
            pub_add_row,
            Space::new().height(24),
            save_btn,
        ]
        .spacing(0);

        if let Some(e) = &self.grant.error {
            col = col.push(Space::new().height(12)).push(
                text(format!("Error: {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(Color {
                        r: 0.9,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    }),
            );
        }

        col.into()
    }

    fn view_pairs_tab(
        &self,
        palette: Palette,
        sizes: FontSize,
        radii: Radii,
    ) -> Element<'_, crate::Message> {
        if self.pairs.loading {
            return text("Loading…")
                .size(TypeRole::Body.size_in(sizes))
                .color(palette.text_muted.into_iced_color())
                .into();
        }

        if !self.pairs.loaded || self.pairs.pairs.is_empty() {
            return empty_state(
                EmptyState::info(
                    "No active federation pairs",
                    "Mint a passcode and share it with another mesh operator to establish a pair.",
                )
                .with_icon(Icon::Network),
                palette,
                || crate::Message::MeshFederation(Message::SelectTab(Tab::Mint)),
            );
        }

        let text_color = palette.text.into_iced_color();
        let text_muted = palette.text_muted.into_iced_color();
        let raised = palette.raised.into_iced_color();
        let accent = palette.accent.into_iced_color();
        let r = f32::from(radii.sm);

        let mut items: Vec<Element<'_, crate::Message>> = Vec::new();

        for pair in &self.pairs.pairs {
            let label = if pair.peer_mesh_label.is_empty() {
                pair.peer_mesh_id.as_str()
            } else {
                pair.peer_mesh_label.as_str()
            };

            let subs = if pair.subscribe_topics.is_empty() {
                "(none)".to_string()
            } else {
                pair.subscribe_topics.join(", ")
            };
            let pubs = if pair.publish_topics.is_empty() {
                "(none)".to_string()
            } else {
                pair.publish_topics.join(", ")
            };

            let is_revoking = self.pairs.revoking.as_deref() == Some(pair.peer_mesh_id.as_str());
            let is_rotating = self.pairs.rotating.as_deref() == Some(pair.peer_mesh_id.as_str());

            let peer_id_rev = pair.peer_mesh_id.clone();
            let peer_id_rot = pair.peer_mesh_id.clone();

            let revoke_btn: Element<'_, crate::Message> = button(
                text(if is_revoking { "Revoking…" } else { "Revoke" })
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(Color {
                        r: 0.9,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    }),
            )
            .padding([4u16, 10u16])
            .style(move |_t, _s: ButtonStatus| button::Style {
                snap: false,
                background: Some(Background::Color(Color {
                    r: 0.8,
                    g: 0.1,
                    b: 0.1,
                    a: 0.12,
                })),
                text_color: Color {
                    r: 0.9,
                    g: 0.2,
                    b: 0.2,
                    a: 1.0,
                },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(crate::Message::MeshFederation(Message::PairsRevokeClicked(
                peer_id_rev,
            )))
            .into();

            let rotate_btn: Element<'_, crate::Message> = button(
                text(if is_rotating { "Rotating…" } else { "Rotate" })
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_color),
            )
            .padding([4u16, 10u16])
            .style(move |_t, _s: ButtonStatus| button::Style {
                snap: false,
                background: Some(Background::Color(raised)),
                text_color,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(crate::Message::MeshFederation(Message::PairsRotateClicked(
                peer_id_rot,
            )))
            .into();

            let audit_btn: Element<'_, crate::Message> = button(
                text("Audit log")
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(accent),
            )
            .padding([4u16, 10u16])
            .style(move |_t, _s: ButtonStatus| button::Style {
                snap: false,
                background: None,
                text_color: accent,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(crate::Message::SelectPanel {
                group: crate::model::Group::Network,
                panel: "mesh_bus",
            })
            .into();

            let action_row: Element<'_, crate::Message> = row![
                revoke_btn,
                Space::new().width(8),
                rotate_btn,
                Space::new().width(8),
                audit_btn,
            ]
            .into();

            let established_str = if pair.established.is_empty() {
                "unknown".to_string()
            } else {
                pair.established.clone()
            };

            let pair_col: Element<'_, crate::Message> = column![
                text(label)
                    .size(TypeRole::Body.size_in(sizes))
                    .color(text_color),
                text(format!("Established: {established_str}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_muted),
                text("Certificate renews annually via `mde-bus federation rotate`")
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_muted),
                text(format!("Subscribe: {subs}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_muted),
                text(format!("Publish: {pubs}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(text_muted),
                Space::new().height(6),
                action_row,
            ]
            .spacing(2)
            .into();

            items.push(pair_col);
            items.push(Space::new().height(16).into());
        }

        if let Some(e) = &self.pairs.error {
            items.push(
                text(format!("Error: {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .color(Color {
                        r: 0.9,
                        g: 0.2,
                        b: 0.2,
                        a: 1.0,
                    })
                    .into(),
            );
        }

        column(items).spacing(0).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tab_is_mint() {
        let panel = MeshFederationPanel::new();
        assert_eq!(panel.active_tab, Tab::Mint);
    }

    #[test]
    fn four_tabs_declared() {
        assert_eq!(Tab::ALL.len(), 4);
    }

    #[test]
    fn tab_labels_are_non_empty() {
        for tab in Tab::ALL {
            assert!(!tab.label().is_empty());
        }
    }

    #[test]
    fn select_tab_updates_active() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Accept));
        assert_eq!(panel.active_tab, Tab::Accept);
        let _ = panel.update(Message::SelectTab(Tab::Pairs));
        assert_eq!(panel.active_tab, Tab::Pairs);
    }

    #[test]
    fn pairs_loading_set_on_pairs_tab_switch() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Pairs));
        assert!(panel.pairs.loading);
        assert!(!panel.pairs.loaded);
    }

    #[test]
    fn mint_not_loading_on_pairs_tab_switch() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Pairs));
        assert!(!panel.mint.loading);
    }

    #[test]
    fn mint_clicked_sets_loading() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::MintClicked);
        assert!(panel.mint.loading);
    }

    #[test]
    fn mint_done_ok_populates_mnemonic() {
        let mut panel = MeshFederationPanel::new();
        let output = MintJsonOutput {
            mnemonic: "apple bridge clutch dance echo fern".to_string(),
            ulid: "01HZX".to_string(),
            expires_at_unix_ms: 1_700_000_000_000 + 24 * 3600 * 1000,
        };
        let _ = panel.update(Message::MintDone(Ok(output)));
        assert!(!panel.mint.loading);
        assert_eq!(
            panel.mint.mnemonic.as_deref(),
            Some("apple bridge clutch dance echo fern")
        );
        assert_eq!(panel.mint.ulid.as_deref(), Some("01HZX"));
        assert!(panel.mint.error.is_none());
    }

    #[test]
    fn mint_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::MintDone(Err("mde-bus not found".to_string())));
        assert!(!panel.mint.loading);
        assert!(panel.mint.mnemonic.is_none());
        assert!(panel.mint.error.is_some());
    }

    #[test]
    fn revoke_done_ok_clears_mnemonic() {
        let mut panel = MeshFederationPanel::new();
        panel.mint.mnemonic = Some("apple bridge clutch dance echo fern".to_string());
        panel.mint.ulid = Some("01HZX".to_string());
        panel.mint.revoking = true;
        let _ = panel.update(Message::RevokeDone(Ok(())));
        assert!(!panel.mint.revoking);
        assert!(panel.mint.mnemonic.is_none());
        assert!(panel.mint.ulid.is_none());
        assert!(panel.mint.error.is_none());
    }

    #[test]
    fn revoke_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        panel.mint.revoking = true;
        let _ = panel.update(Message::RevokeDone(Err("CLI error".to_string())));
        assert!(!panel.mint.revoking);
        assert!(panel.mint.error.is_some());
    }

    #[test]
    fn accept_input_changed_updates_state() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::AcceptInputChanged("apple bridge".to_string()));
        assert_eq!(panel.accept.input, "apple bridge");
    }

    #[test]
    fn accept_submit_sets_submitting() {
        let mut panel = MeshFederationPanel::new();
        panel.accept.input = "apple bridge clutch dance echo fern".to_string();
        let _ = panel.update(Message::AcceptSubmitClicked);
        assert!(panel.accept.submitting);
    }

    #[test]
    fn accept_done_ok_transitions_to_grant_tab() {
        let mut panel = MeshFederationPanel::new();
        panel.accept.submitting = true;
        let result = AcceptResult {
            peer_mesh_id: "fingerprint-abc".to_string(),
            peer_mesh_label: "Workplace".to_string(),
        };
        let _ = panel.update(Message::AcceptDone(Ok(result)));
        assert!(!panel.accept.submitting);
        assert_eq!(panel.active_tab, Tab::Grant);
        assert_eq!(panel.grant.peer_mesh_id.as_deref(), Some("fingerprint-abc"));
        assert_eq!(panel.grant.peer_label.as_deref(), Some("Workplace"));
        assert!(panel.accept.input.is_empty());
        assert!(panel.accept.success.is_some());
    }

    #[test]
    fn accept_done_ok_seeds_default_subscribe() {
        let mut panel = MeshFederationPanel::new();
        let result = AcceptResult {
            peer_mesh_id: "fp".to_string(),
            peer_mesh_label: "Lab".to_string(),
        };
        let _ = panel.update(Message::AcceptDone(Ok(result)));
        assert_eq!(panel.grant.subscribe_topics, vec!["#"]);
    }

    #[test]
    fn accept_done_ok_preserves_existing_grant_topics() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.subscribe_topics = vec!["fleet/#".to_string()];
        let result = AcceptResult {
            peer_mesh_id: "fp".to_string(),
            peer_mesh_label: "Lab".to_string(),
        };
        let _ = panel.update(Message::AcceptDone(Ok(result)));
        assert_eq!(panel.grant.subscribe_topics, vec!["fleet/#"]);
    }

    #[test]
    fn accept_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        panel.accept.submitting = true;
        let _ = panel.update(Message::AcceptDone(Err("invalid passcode".to_string())));
        assert!(!panel.accept.submitting);
        assert!(panel.accept.error.is_some());
        assert_eq!(panel.active_tab, Tab::Mint);
    }

    #[test]
    fn grant_add_subscribe_appends_topic() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.new_subscribe = "fleet/#".to_string();
        let _ = panel.update(Message::GrantAddSubscribeClicked);
        assert!(panel
            .grant
            .subscribe_topics
            .contains(&"fleet/#".to_string()));
        assert!(panel.grant.new_subscribe.is_empty());
    }

    #[test]
    fn grant_add_subscribe_ignores_duplicate() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.subscribe_topics = vec!["fleet/#".to_string()];
        panel.grant.new_subscribe = "fleet/#".to_string();
        let _ = panel.update(Message::GrantAddSubscribeClicked);
        assert_eq!(panel.grant.subscribe_topics.len(), 1);
    }

    #[test]
    fn grant_remove_subscribe_removes_topic() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.subscribe_topics = vec!["fleet/#".to_string(), "mon/+".to_string()];
        let _ = panel.update(Message::GrantRemoveSubscribe("fleet/#".to_string()));
        assert_eq!(panel.grant.subscribe_topics, vec!["mon/+"]);
    }

    #[test]
    fn grant_add_publish_appends_topic() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.new_publish = "portal/peer-presence/*".to_string();
        let _ = panel.update(Message::GrantAddPublishClicked);
        assert!(panel
            .grant
            .publish_topics
            .contains(&"portal/peer-presence/*".to_string()));
        assert!(panel.grant.new_publish.is_empty());
    }

    #[test]
    fn grant_remove_publish_removes_topic() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.publish_topics = vec!["portal/*".to_string()];
        let _ = panel.update(Message::GrantRemovePublish("portal/*".to_string()));
        assert!(panel.grant.publish_topics.is_empty());
    }

    #[test]
    fn grant_save_clicked_sets_saving() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.peer_mesh_id = Some("fp".to_string());
        let _ = panel.update(Message::GrantSaveClicked);
        assert!(panel.grant.saving);
    }

    #[test]
    fn grant_save_done_ok_clears_saving() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.saving = true;
        let _ = panel.update(Message::GrantSaveDone(Ok(())));
        assert!(!panel.grant.saving);
        assert!(panel.grant.error.is_none());
    }

    #[test]
    fn grant_save_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        panel.grant.saving = true;
        let _ = panel.update(Message::GrantSaveDone(Err("write failed".to_string())));
        assert!(!panel.grant.saving);
        assert!(panel.grant.error.is_some());
    }

    #[test]
    fn pairs_loaded_ok_populates_list() {
        let mut panel = MeshFederationPanel::new();
        let pairs = vec![FederationPairYaml {
            peer_mesh_id: "fp-abc".to_string(),
            peer_mesh_label: "Workplace".to_string(),
            established: "2026-05-27T14:32:00Z".to_string(),
            subscribe_topics: vec!["#".to_string()],
            publish_topics: vec![],
            excluded_topics: vec![],
        }];
        let _ = panel.update(Message::PairsLoaded(Ok(pairs)));
        assert!(panel.pairs.loaded);
        assert!(!panel.pairs.loading);
        assert_eq!(panel.pairs.pairs.len(), 1);
        assert_eq!(panel.pairs.pairs[0].peer_mesh_label, "Workplace");
        assert!(panel.pairs.error.is_none());
    }

    #[test]
    fn pairs_loaded_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::PairsLoaded(Err("no data dir".to_string())));
        assert!(panel.pairs.error.is_some());
        assert!(panel.pairs.pairs.is_empty());
        assert!(panel.pairs.loaded);
    }

    #[test]
    fn pairs_refresh_clears_and_reloads() {
        let mut panel = MeshFederationPanel::new();
        panel.pairs.loaded = true;
        panel.pairs.pairs = vec![FederationPairYaml::default()];
        let _ = panel.update(Message::PairsRefreshClicked);
        assert!(panel.pairs.loading);
        assert!(!panel.pairs.loaded);
    }

    #[test]
    fn pairs_revoke_sets_revoking_flag() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::PairsRevokeClicked("fp".to_string()));
        assert_eq!(panel.pairs.revoking.as_deref(), Some("fp"));
    }

    #[test]
    fn pairs_revoke_done_ok_clears_flag_and_reloads() {
        let mut panel = MeshFederationPanel::new();
        panel.pairs.revoking = Some("fp".to_string());
        let _ = panel.update(Message::PairsRevokeDone(Ok("fp".to_string())));
        assert!(panel.pairs.revoking.is_none());
        assert!(panel.pairs.loading);
    }

    #[test]
    fn pairs_revoke_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        panel.pairs.revoking = Some("fp".to_string());
        let _ = panel.update(Message::PairsRevokeDone(Err("mde-bus failed".to_string())));
        assert!(panel.pairs.revoking.is_none());
        assert!(panel.pairs.error.is_some());
    }

    #[test]
    fn pairs_rotate_sets_rotating_flag() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::PairsRotateClicked("fp".to_string()));
        assert_eq!(panel.pairs.rotating.as_deref(), Some("fp"));
    }

    #[test]
    fn pairs_rotate_done_ok_clears_flag() {
        let mut panel = MeshFederationPanel::new();
        panel.pairs.rotating = Some("fp".to_string());
        let _ = panel.update(Message::PairsRotateDone(Ok("fp".to_string())));
        assert!(panel.pairs.rotating.is_none());
    }

    #[test]
    fn pairs_rotate_done_err_sets_error() {
        let mut panel = MeshFederationPanel::new();
        panel.pairs.rotating = Some("fp".to_string());
        let _ = panel.update(Message::PairsRotateDone(Err("failed".to_string())));
        assert!(panel.pairs.rotating.is_none());
        assert!(panel.pairs.error.is_some());
    }

    #[test]
    fn format_expiry_zero_returns_expired() {
        // Past timestamp → "expired"
        assert_eq!(format_expiry(0), "expired");
    }

    #[test]
    fn format_expiry_far_future_shows_hours() {
        let future_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
            + 6 * 3600 * 1000; // 6 hours from now
        let result = format_expiry(future_ms);
        assert!(
            result.contains('h'),
            "expected 'Xh Ym remaining', got {result}"
        );
    }

    #[test]
    fn grant_new_subscribe_changed_updates_state() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::GrantNewSubscribeChanged("fleet/#".to_string()));
        assert_eq!(panel.grant.new_subscribe, "fleet/#");
    }

    #[test]
    fn grant_new_publish_changed_updates_state() {
        let mut panel = MeshFederationPanel::new();
        let _ = panel.update(Message::GrantNewPublishChanged("portal/*".to_string()));
        assert_eq!(panel.grant.new_publish, "portal/*");
    }
}
