//! E6.1 — "Manage Your Server" role-card landing.
//!
//! Renders a [`Group`]'s root view as a Server-2003-style **role
//! card**: an icon + title header, a one-line task-oriented role
//! description, a **Tasks** list of action-links (one per
//! [`nav_model`] panel → [`Message::SelectPanel`]), and a **See also**
//! sidebar of related roles (→ [`Message::SelectGroup`]).
//!
//! This replaces the old `panel_under_construction` group-root
//! placeholder (a §3 not-ready-yet surface). Every link is wired to a
//! real, already-existing navigation message — no stubs: a panel whose
//! reducer hasn't shipped still lands on the friendly per-panel
//! empty-state, and every See-also link is a live group jump.
//!
//! Themed only through `mde_theme::Palette` tokens (no raw hex), the
//! same dark palette every workbench panel reads. E6.2–6.9 then fill
//! each role's bespoke card view; this is the shared foundation.

use cosmic::iced::widget::{button, column, row, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding};
use cosmic::Theme;
use mde_theme::{mde_icon, Icon, IconSize, Palette};

use crate::cosmic_compat::prelude::*;
use crate::model::{nav_model, Group, Panel};
use crate::Message;

/// The card-header glyph for a role — 1:1 with the sidebar group icon.
/// PLANES-1: the four new plane groups reuse existing Material glyphs
/// (no new SVG assets) — kept distinct per the `role_icons_*` test.
const fn role_icon(group: Group) -> Icon {
    // NAV-1 — distinct glyph per visible section.
    match group {
        Group::Dashboard => Icon::Dashboard,
        Group::ThisNode => Icon::Workbench,
        Group::Mesh => Icon::Network,
        Group::Fleet => Icon::Fleet,
        Group::Provisioning => Icon::Update,
        Group::Monitoring => Icon::Maintain,
        Group::System => Icon::System,
    }
}

/// One-line, task-oriented role description (the "Manage Your Server"
/// console voice). Shown under the role title on the card.
const fn role_description(group: Group) -> &'static str {
    match group {
        Group::Dashboard => {
            "At-a-glance system and fleet status, with quick links into every section."
        }
        Group::ThisNode => {
            "This box — hardware, services, and its local networking (interfaces, Wi-Fi, VPN, firewall, remote access)."
        }
        Group::Mesh => {
            "The mesh — every peer plus mesh-wide services: control, storage, DNS, routing, federation, bus, publishing, and join."
        }
        Group::Fleet => {
            "Drive the fleet — roster, rollup, tags, and orchestration (jobs, playbooks, remediation)."
        }
        Group::Provisioning => {
            "Build and enrol nodes — node roles, install profiles, images (ISO/VM/container/USB), mirrors, and compute instances."
        }
        Group::Monitoring => {
            "Observe everything — health, logs and metrics, fleet logs, run history, audit, mesh history, and resources."
        }
        Group::System => {
            "Configure and maintain — local/fleet config, policy, snapshots, debloat, repair, and help."
        }
    }
}

/// Related roles surfaced in the card's "See also" sidebar. Each is a
/// live [`Message::SelectGroup`] jump; never includes `group` itself.
const fn see_also(group: Group) -> &'static [Group] {
    match group {
        Group::Dashboard => &[Group::Mesh, Group::Fleet, Group::Monitoring],
        Group::ThisNode => &[Group::Mesh, Group::Monitoring, Group::System],
        Group::Mesh => &[Group::Fleet, Group::ThisNode, Group::Monitoring],
        Group::Fleet => &[Group::Mesh, Group::Provisioning, Group::Monitoring],
        Group::Provisioning => &[Group::Fleet, Group::Mesh, Group::System],
        Group::Monitoring => &[Group::Mesh, Group::Fleet, Group::System],
        Group::System => &[Group::ThisNode, Group::Monitoring, Group::Provisioning],
    }
}

/// The panels surfaced as Tasks action-links on a role's card — the
/// group's [`nav_model`] entry, in its locked order. Each maps to a
/// `Message::SelectPanel { group, panel }`, so the card and the sidebar
/// stay in lock-step. Exposed for the E6 role-contract tests.
#[must_use]
pub fn role_action_panels(group: Group) -> Vec<Panel> {
    nav_model()
        .into_iter()
        .find(|e| e.group == group)
        .map(|e| e.panels)
        .unwrap_or_default()
}

/// Render the role-card landing for a group-root view.
#[must_use]
pub fn role_landing<'a>(group: Group) -> Element<'a, Message, Theme> {
    let palette = crate::live_theme::palette();

    // ── header: icon + title + description ──────────────────────────
    let icon_widget = header_icon(role_icon(group), palette);
    let title = text(group.label().to_string())
        .size(26)
        .colr(palette.text.into_cosmic_color());
    let description = text(role_description(group).to_string())
        .size(13)
        .colr(palette.text_muted.into_cosmic_color());
    let header = row![
        icon_widget,
        Space::new().width(Length::Fixed(14.0)),
        column![title, Space::new().height(Length::Fixed(4.0)), description].spacing(0),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // ── tasks: one action-link per panel in this group ──────────────
    let panels = role_action_panels(group);
    let mut task_links: Vec<Element<'a, Message, Theme>> = vec![section_label("Tasks", palette)];
    for panel in panels {
        task_links.push(action_link(panel.label(), group, panel.slug(), palette));
    }
    let tasks_col = column(task_links).spacing(4).width(Length::FillPortion(3));

    // ── see also: related-role jumps ────────────────────────────────
    let mut see_also_links: Vec<Element<'a, Message, Theme>> =
        vec![section_label("See also", palette)];
    for related in see_also(group).iter().copied() {
        see_also_links.push(group_link(related, palette));
    }
    let see_also_col = column(see_also_links)
        .spacing(4)
        .width(Length::FillPortion(1));

    let body = row![
        tasks_col,
        Space::new().width(Length::Fixed(24.0)),
        see_also_col,
    ]
    .width(Length::Fill);

    let content = column![header, Space::new().height(Length::Fixed(20.0)), body].spacing(0);

    // The card surface: raised background + 1 px border, palette-tokened.
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    cosmic::iced::widget::container(content)
        .padding(Padding::from([24u16, 28u16]))
        .width(Length::Fill)
        .sty(move |_t: &Theme| cosmic::iced::widget::container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..cosmic::iced::widget::container::Style::default()
        })
        .into()
}

/// The card-header icon (PanelHeader size), falling back to the glyph
/// char when no SVG resolves.
fn header_icon<'a>(icon: Icon, palette: Palette) -> Element<'a, Message, Theme> {
    let resolved = mde_icon(icon, IconSize::PanelHeader);
    if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        let tint = palette.text.into_cosmic_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(tint) })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .colr(palette.text.into_cosmic_color())
            .into()
    }
}

/// A small uppercase section divider label ("Tasks" / "See also").
fn section_label<'a>(label: &'static str, palette: Palette) -> Element<'a, Message, Theme> {
    text(label)
        .size(11)
        .colr(palette.text_muted.into_cosmic_color())
        .into()
}

/// One Tasks action-link → opens the named panel under `group`.
fn action_link<'a>(
    label: &'static str,
    group: Group,
    panel_slug: &'static str,
    palette: Palette,
) -> Element<'a, Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    button(text(label.to_string()).size(13).colr(accent))
        .padding(Padding::from([4u16, 8u16]))
        .sty(link_button_style(palette))
        .on_press(Message::SelectPanel {
            group,
            panel: panel_slug,
        })
        .into()
}

/// One See-also link → jumps to the related role's landing.
fn group_link<'a>(group: Group, palette: Palette) -> Element<'a, Message, Theme> {
    let muted = palette.text_muted.into_cosmic_color();
    button(text(group.label().to_string()).size(13).colr(muted))
        .padding(Padding::from([4u16, 8u16]))
        .sty(link_button_style(palette))
        .on_press(Message::SelectGroup(group))
        .into()
}

/// Shared transparent link-button style: no chrome at rest, a faint
/// raised tint on hover (tint of the palette token, no raw hex).
fn link_button_style(
    palette: Palette,
) -> impl Fn(&Theme, cosmic::iced::widget::button::Status) -> cosmic::iced::widget::button::Style + 'static
{
    let raised = palette.raised.into_cosmic_color();
    move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
        let hover_bg = Color {
            r: raised.r * 1.12,
            g: raised.g * 1.12,
            b: raised.b * 1.12,
            a: raised.a,
        };
        cosmic::iced::widget::button::Style {
            snap: false,
            background: match status {
                cosmic::iced::widget::button::Status::Hovered
                | cosmic::iced::widget::button::Status::Pressed => {
                    Some(Background::Color(hover_bg))
                }
                _ => None,
            },
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 4.0.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..cosmic::iced::widget::button::Style::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_has_a_nonempty_description() {
        for g in Group::all() {
            assert!(
                !role_description(g).is_empty(),
                "group {g:?} has no role description"
            );
        }
    }

    #[test]
    fn see_also_is_nonempty_and_excludes_self() {
        for g in Group::all() {
            let related = see_also(g);
            assert!(!related.is_empty(), "group {g:?} has no see-also links");
            assert!(
                !related.contains(&g),
                "group {g:?} see-also must not include itself"
            );
        }
    }

    #[test]
    fn role_icons_are_distinct_per_group() {
        let icons: Vec<Icon> = Group::all().iter().map(|g| role_icon(*g)).collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b, "two groups share a role icon");
            }
        }
    }

    #[test]
    fn role_landing_constructs_for_every_group() {
        for g in Group::all() {
            let _: Element<'_, Message, Theme> = role_landing(g);
        }
    }

    #[test]
    fn every_role_card_has_at_least_one_action_link() {
        // The card↔nav_model contract: each sidebar role surfaces ≥1
        // Tasks action-link (an empty card would be a dead landing).
        for g in Group::sidebar_groups() {
            assert!(
                !role_action_panels(g).is_empty(),
                "role {g:?} renders no action-links"
            );
        }
    }

    #[test]
    fn mesh_section_leads_with_peers_then_mesh_services() {
        // NAV-1 (Q9) — Peers is the first item under Mesh, followed by the
        // mesh-wide services.
        let slugs: Vec<&str> = role_action_panels(Group::Mesh)
            .iter()
            .map(Panel::slug)
            .collect();
        assert_eq!(slugs.first(), Some(&"peers"));
        for want in [
            "mesh_control",
            "mesh_storage",
            "dns",
            "routing",
            "mesh_join",
        ] {
            assert!(slugs.contains(&want), "Mesh missing {want}: {slugs:?}");
        }
    }

    #[test]
    fn fleet_section_has_roster_plus_orchestration() {
        // NAV-1 (Q6) — Fleet absorbs the old Controller orchestration.
        let slugs: Vec<&str> = role_action_panels(Group::Fleet)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in [
            "fleet_rollup",
            "inventory",
            "tags",
            "jobs",
            "playbooks",
            "drift",
        ] {
            assert!(slugs.contains(&want), "Fleet missing {want}: {slugs:?}");
        }
    }

    #[test]
    fn monitoring_section_gathers_observability() {
        // NAV-1 (Q11) — one Monitoring section across scopes.
        let slugs: Vec<&str> = role_action_panels(Group::Monitoring)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in [
            "health_check",
            "fleet_logs",
            "audit",
            "mesh_history",
            "resources",
        ] {
            assert!(
                slugs.contains(&want),
                "Monitoring missing {want}: {slugs:?}"
            );
        }
    }

    #[test]
    fn system_section_combines_config_maintain_help() {
        // NAV-1 follow-up — System = Config + Maintenance + Help.
        let slugs: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in ["config_apply", "policy", "hub", "repair", "index", "about"] {
            assert!(slugs.contains(&want), "System missing {want}: {slugs:?}");
        }
    }

    #[test]
    fn this_node_role_card_surfaces_relocated_wallpaper_and_notifications() {
        // NAV-1.2 — the Desktop group was retired; wallpaper + notifications
        // (mesh-specific kept panels) now surface as This Node role tasks.
        let slugs: Vec<&str> = role_action_panels(Group::ThisNode)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in ["wallpaper", "notifications"] {
            assert!(
                slugs.contains(&want),
                "This Node missing relocated {want}: {slugs:?}"
            );
        }
    }

    #[test]
    fn system_role_card_surfaces_relocated_update_and_sync_status() {
        // NAV-1.2 — system update + panel sync status relocated into System.
        let slugs: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in ["system_update", "sync_status"] {
            assert!(
                slugs.contains(&want),
                "System missing relocated {want}: {slugs:?}"
            );
        }
    }
}
