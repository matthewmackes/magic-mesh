//! PLANES-21 — Provisioning ▸ Install Profiles panel.
//!
//! An install profile is a named deployment template (W56): a role pin,
//! capability tags, the kickstart `%post` fragments it injects, and
//! whether the firstboot auto-join slot is baked in (W60). One image
//! carries every profile; the boot menu picks one at install (W57). This
//! panel shells `mackesd profiles --json` and renders the catalog (the
//! shipped per-role core pack + any TOML profiles on LizardFS).
//!
//! Read-only renderer (W88): profiles are authored as TOML; the write
//! side (form edit) + the actual image bake (PLANES-22) build on this.

use std::time::SystemTime;

use cosmic::iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, Space,
};
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;

/// W56 — the roles a profile may pin (the backend validates against the
/// same set; surfaced here as the form's picker).
pub const ROLES: &[&str] = &["lighthouse", "server", "workstation"];

/// One install profile, parsed from `mackesd profiles --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub name: String,
    pub description: String,
    pub role: String,
    pub tags: Vec<String>,
    pub ks_fragments: Vec<String>,
    pub auto_join: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProfilesPanel {
    pub rows: Vec<ProfileRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    // W56 — the form-edit write side. A blank name = create; an existing
    // name = overwrite (the backend overwrites a same-named profile).
    pub form_name: String,
    pub form_role: String,
    pub form_tags: String,
    pub form_ks: String,
    pub form_auto_join: bool,
    pub form_busy: bool,
    pub form_msg: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<ProfileRow>, String>),
    RefreshClicked,
    // W56 — form-edit write side.
    FormName(String),
    FormRole(String),
    FormTags(String),
    FormKs(String),
    FormAutoJoin(bool),
    /// Load an existing row into the form to edit it.
    EditRow(String),
    SaveClicked,
    Saved(Result<(), String>),
    DeleteClicked(String),
    Deleted(Result<(), String>),
}

impl ProfilesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_profiles() }, |result| {
            crate::Message::Profiles(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(rows)) => {
                self.rows = rows;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.rows = Vec::new();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
            Message::FormName(v) => {
                self.form_name = v;
                Task::none()
            }
            Message::FormRole(v) => {
                self.form_role = v;
                Task::none()
            }
            Message::FormTags(v) => {
                self.form_tags = v;
                Task::none()
            }
            Message::FormKs(v) => {
                self.form_ks = v;
                Task::none()
            }
            Message::FormAutoJoin(v) => {
                self.form_auto_join = v;
                Task::none()
            }
            Message::EditRow(name) => {
                if let Some(r) = self.rows.iter().find(|r| r.name == name) {
                    self.form_name = r.name.clone();
                    self.form_role = r.role.clone();
                    self.form_tags = r.tags.join(", ");
                    self.form_ks = r.ks_fragments.join(", ");
                    self.form_auto_join = r.auto_join;
                    self.form_msg = format!("editing {name} — Save overwrites it");
                }
                Task::none()
            }
            Message::SaveClicked => {
                if self.form_busy {
                    return Task::none();
                }
                let name = self.form_name.trim().to_string();
                if name.is_empty() {
                    self.form_msg = "name is required".into();
                    return Task::none();
                }
                if self.form_role.is_empty() {
                    self.form_msg = "pick a role".into();
                    return Task::none();
                }
                self.form_busy = true;
                self.form_msg = format!("saving {name}…");
                let args = build_set_args(
                    &name,
                    &self.form_role,
                    &self.form_tags,
                    &self.form_ks,
                    self.form_auto_join,
                );
                Task::perform(run_profiles_cmd(args), |r| {
                    crate::Message::Profiles(Message::Saved(r))
                })
            }
            Message::Saved(Ok(())) => {
                self.form_busy = false;
                self.form_msg = "saved.".into();
                Self::load()
            }
            Message::Saved(Err(e)) => {
                self.form_busy = false;
                self.form_msg = format!("save failed: {e}");
                Task::none()
            }
            Message::DeleteClicked(name) => {
                if self.form_busy {
                    return Task::none();
                }
                self.form_busy = true;
                self.form_msg = format!("deleting {name}…");
                let args = vec!["profiles".to_string(), "--rm".into(), name];
                Task::perform(run_profiles_cmd(args), |r| {
                    crate::Message::Profiles(Message::Deleted(r))
                })
            }
            Message::Deleted(Ok(())) => {
                self.form_busy = false;
                self.form_msg = "deleted (core profiles revert to the shipped default).".into();
                Self::load()
            }
            Message::Deleted(Err(e)) => {
                self.form_busy = false;
                self.form_msg = format!("delete failed: {e}");
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Install Profiles")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} profile{} — one image carries them all (boot-menu choice at install)",
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" }
            )
        } else {
            "click Refresh to load".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::Profiles(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(profile_row(r, palette));
        }
        if self.rows.is_empty() && self.last_run_at.is_some() {
            rows_col = rows_col.push(empty_state_card(palette, self.error.as_deref()));
        }

        // W56 — the form-edit write side: create a profile or overwrite
        // an existing one (Edit on a row populates this), validated +
        // written by `mackesd profiles --set`.
        let role_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message, cosmic::Theme> =
            pick_list(ROLES, current_role(&self.form_role), |r| {
                crate::Message::Profiles(Message::FormRole(r.to_string()))
            });
        let auto_btn = variant_button(
            if self.form_auto_join {
                "auto-join: on"
            } else {
                "auto-join: off"
            },
            ButtonVariant::Secondary,
            Some(crate::Message::Profiles(Message::FormAutoJoin(
                !self.form_auto_join,
            ))),
            palette,
        );
        let save_btn = variant_button(
            if self.form_busy {
                "Saving…"
            } else {
                "Save profile"
            },
            ButtonVariant::Primary,
            (!self.form_busy).then_some(crate::Message::Profiles(Message::SaveClicked)),
            palette,
        );
        let form = column![
            text("New / edit profile").size(14),
            row![
                text_input("name (a-z0-9-)", &self.form_name)
                    .on_input(|v| crate::Message::Profiles(Message::FormName(v)))
                    .width(Length::Fixed(200.0)),
                role_pick,
                auto_btn,
            ]
            .spacing(8),
            text_input("tags (comma: hop, execution, headless)", &self.form_tags)
                .on_input(|v| crate::Message::Profiles(Message::FormTags(v))),
            text_input("kickstart %post fragments (comma)", &self.form_ks)
                .on_input(|v| crate::Message::Profiles(Message::FormKs(v))),
            row![save_btn, text(self.form_msg.clone()).size(12)].spacing(12),
        ]
        .spacing(6);

        container(
            column![
                header,
                Space::new().height(Length::Fixed(16.0)),
                form,
                Space::new().height(Length::Fixed(16.0)),
                scrollable(rows_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// W56 — the form role picker's current selection (None until picked).
#[must_use]
fn current_role(v: &str) -> Option<&'static str> {
    ROLES.iter().copied().find(|r| *r == v)
}

fn profile_row<'a>(r: &'a ProfileRow, palette: Palette) -> Element<'a, crate::Message> {
    let accent = palette.accent.into_cosmic_color();
    let resolved = mde_icon(Icon::Update, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(accent),
            })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(accent).into()
    };

    let head = row![
        icon_widget,
        text(r.name.clone())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
        text(format!("role: {}", r.role))
            .size(10)
            .colr(palette.accent.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(if r.auto_join {
            "auto-join"
        } else {
            "manual enroll"
        })
        .size(10)
        .colr(if r.auto_join {
            palette.success.into_cosmic_color()
        } else {
            palette.text_muted.into_cosmic_color()
        }),
        // W56 — per-row edit/delete (delete reverts a core profile to its
        // shipped default; removes an operator-authored one).
        variant_button(
            "Edit",
            ButtonVariant::Ghost,
            Some(crate::Message::Profiles(Message::EditRow(r.name.clone()))),
            palette,
        ),
        variant_button(
            "Delete",
            ButtonVariant::Ghost,
            Some(crate::Message::Profiles(Message::DeleteClicked(
                r.name.clone()
            ))),
            palette,
        ),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let desc = text(r.description.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    let tags_line = text(format!(
        "tags: {} · ks: {}",
        if r.tags.is_empty() {
            "—".to_string()
        } else {
            r.tags.join(", ")
        },
        if r.ks_fragments.is_empty() {
            "—".to_string()
        } else {
            r.ks_fragments.join(", ")
        }
    ))
    .size(10)
    .colr(palette.text_muted.into_cosmic_color());

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![head, desc, tags_line].spacing(3))
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn empty_state_card<'a>(palette: Palette, error: Option<&'a str>) -> Element<'a, crate::Message> {
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_cosmic_color(),
            "Couldn't read profiles".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_cosmic_color(),
            "No install profiles".to_string(),
            "The core pack ships one profile per role (lighthouse / server / workstation). \
             Drop a TOML profile under the workgroup's profiles/ dir to add more."
                .to_string(),
        )
    };
    let icon_kind = if error.is_some() {
        Icon::StatusError
    } else {
        Icon::Update
    };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .colr(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading).size(14).colr(palette.text.into_cosmic_color()),
            text(body)
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

// ---- I/O ------------------------------------------------------

/// W56 — split a comma-separated form field into trimmed, non-empty items.
#[must_use]
fn csv_items(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// W56 — build the `mackesd profiles --set …` argv from the form. Pure +
/// tested; the backend does the authoritative role/tag/name validation.
#[must_use]
pub fn build_set_args(
    name: &str,
    role: &str,
    tags_csv: &str,
    ks_csv: &str,
    auto_join: bool,
) -> Vec<String> {
    let mut a = vec![
        "profiles".to_string(),
        "--set".to_string(),
        name.to_string(),
        "--role".to_string(),
        role.to_string(),
    ];
    for t in csv_items(tags_csv) {
        a.push("--tag".to_string());
        a.push(t);
    }
    for k in csv_items(ks_csv) {
        a.push("--ks-fragment".to_string());
        a.push(k);
    }
    if auto_join {
        a.push("--auto-join".to_string());
    }
    a
}

/// W56 — run a `mackesd profiles …` write command; `Ok` on exit 0, else
/// the trimmed stderr (the backend's validation message).
async fn run_profiles_cmd(args: Vec<String>) -> Result<(), String> {
    let out = tokio::process::Command::new("mackesd")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("mackesd profiles failed to spawn: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Shell out to `mackesd profiles --json` and parse the catalog.
pub fn fetch_profiles() -> Result<Vec<ProfileRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["profiles", "--json"])
        .output()
        .map_err(|e| format!("mackesd profiles failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd profiles exited non-zero: {stderr}"));
    }
    Ok(parse_profiles(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `profiles --json` array.
#[must_use]
pub fn parse_profiles(raw: &str) -> Vec<ProfileRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let str_vec = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    top.into_iter()
        .filter_map(|p| {
            let name = p.get("name").and_then(|v| v.as_str())?.to_string();
            Some(ProfileRow {
                name,
                description: p
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                role: p
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tags: str_vec(p.get("tags")),
                ks_fragments: str_vec(p.get("ks_fragments")),
                auto_join: p
                    .get("auto_join")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_profiles_reads_the_catalog_shape() {
        let raw = r#"[
            {"name":"server","description":"d","role":"server",
             "tags":["execution","headless"],"ks_fragments":["role-server"],"auto_join":true}
        ]"#;
        let rows = parse_profiles(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role, "server");
        assert_eq!(
            rows[0].tags,
            vec!["execution".to_string(), "headless".to_string()]
        );
        assert!(rows[0].auto_join);
    }

    #[test]
    fn parse_profiles_returns_empty_for_garbage() {
        assert!(parse_profiles("not json").is_empty());
        assert!(parse_profiles("").is_empty());
    }

    #[test]
    fn build_set_args_assembles_the_cli_invocation() {
        // W56 — name/role + repeated --tag/--ks-fragment + the auto-join flag.
        let a = build_set_args(
            "anvil",
            "server",
            "execution, headless",
            "role-server",
            true,
        );
        assert_eq!(
            a,
            vec![
                "profiles",
                "--set",
                "anvil",
                "--role",
                "server",
                "--tag",
                "execution",
                "--tag",
                "headless",
                "--ks-fragment",
                "role-server",
                "--auto-join",
            ]
        );
        // No tags/ks, manual-enroll → no repeated flags, no --auto-join.
        let b = build_set_args("hop1", "lighthouse", "", "  ", false);
        assert_eq!(b, vec!["profiles", "--set", "hop1", "--role", "lighthouse"]);
    }

    #[test]
    fn save_requires_name_and_role_before_shelling() {
        let mut p = ProfilesPanel::new();
        // No name → refused, not busy.
        let _ = p.update(Message::SaveClicked);
        assert!(!p.form_busy);
        assert!(p.form_msg.contains("name is required"));
        // Name but no role → refused.
        let _ = p.update(Message::FormName("anvil".into()));
        let _ = p.update(Message::SaveClicked);
        assert!(!p.form_busy);
        assert!(p.form_msg.contains("role"));
        // Name + role → goes busy (shells out).
        let _ = p.update(Message::FormRole("server".into()));
        let _ = p.update(Message::SaveClicked);
        assert!(p.form_busy);
    }

    #[test]
    fn edit_row_populates_the_form() {
        let mut p = ProfilesPanel::new();
        p.rows = parse_profiles(
            r#"[{"name":"server","description":"d","role":"server",
                 "tags":["execution"],"ks_fragments":["role-server"],"auto_join":true}]"#,
        );
        let _ = p.update(Message::EditRow("server".into()));
        assert_eq!(p.form_name, "server");
        assert_eq!(p.form_role, "server");
        assert_eq!(p.form_tags, "execution");
        assert_eq!(p.form_ks, "role-server");
        assert!(p.form_auto_join);
    }

    #[test]
    fn save_result_clears_busy_with_a_message() {
        let mut p = ProfilesPanel::new();
        p.form_busy = true;
        let _ = p.update(Message::Saved(Err("bad role 'x'".into())));
        assert!(!p.form_busy);
        assert!(p.form_msg.contains("save failed"));
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = ProfilesPanel::new();
        p.rows = parse_profiles(
            r#"[{"name":"workstation","description":"d","role":"workstation",
                "tags":["execution"],"ks_fragments":["cosmic"],"auto_join":true}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = ProfilesPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("mackesd down".into());
        let _ = empty.view();
    }
}
