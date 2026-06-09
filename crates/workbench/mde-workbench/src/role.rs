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

use iced::widget::{button, column, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Theme};
use mde_theme::{mde_icon, Icon, IconSize, Palette};

use crate::model::{nav_model, Group, Panel};
use crate::Message;

/// The card-header glyph for a role — 1:1 with the sidebar group icon.
const fn role_icon(group: Group) -> Icon {
    match group {
        Group::Dashboard => Icon::Dashboard,
        Group::Apps => Icon::Apps,
        Group::Devices => Icon::Devices,
        Group::Fleet => Icon::Fleet,
        Group::Compute => Icon::Compute,
        Group::LookAndFeel => Icon::LookAndFeel,
        Group::Maintain => Icon::Maintain,
        Group::Network => Icon::Network,
        Group::System => Icon::System,
        Group::Help => Icon::Help,
    }
}

/// One-line, task-oriented role description (the "Manage Your Server"
/// console voice). Shown under the role title on the card.
const fn role_description(group: Group) -> &'static str {
    match group {
        Group::Dashboard => {
            "At-a-glance system and fleet status, with quick links into every management role."
        }
        Group::Apps => {
            "Install, update, and remove software; manage package sources and default apps."
        }
        Group::Devices => {
            "Configure displays, sound, printers, input devices, power, and connected peripherals."
        }
        Group::Fleet => {
            "Drive multi-host deployment — inventory, playbooks, run history, and config revisions."
        }
        Group::Compute => {
            "Run local and fleet VMs and containers — create, start, stop, migrate, and open consoles."
        }
        Group::LookAndFeel => {
            "Restyle the desktop — themes, fonts, wallpaper, and panel sync status."
        }
        Group::Maintain => {
            "Keep the workstation healthy — snapshots, debloat, health checks, repair, and drift."
        }
        Group::Network => {
            "Manage the mesh and local networking — peers, VPN, firewall, and remote desktop."
        }
        Group::System => {
            "Administer core system settings — date & time, logs, resources, updates, notifications."
        }
        Group::Help => "Browse help topics and the embedded disclaimer.",
    }
}

/// Related roles surfaced in the card's "See also" sidebar. Each is a
/// live [`Message::SelectGroup`] jump; never includes `group` itself.
const fn see_also(group: Group) -> &'static [Group] {
    match group {
        Group::Dashboard => &[Group::Maintain, Group::System, Group::Fleet],
        Group::Apps => &[Group::System, Group::Maintain],
        Group::Devices => &[Group::LookAndFeel, Group::System],
        Group::Fleet => &[Group::Compute, Group::Network, Group::Maintain],
        Group::Compute => &[Group::Fleet, Group::System],
        Group::LookAndFeel => &[Group::Devices, Group::System],
        Group::Maintain => &[Group::System, Group::Fleet],
        Group::Network => &[Group::Fleet, Group::System],
        Group::System => &[Group::Maintain, Group::Apps],
        Group::Help => &[Group::Dashboard],
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
pub fn role_landing<'a>(group: Group) -> Element<'a, Message> {
    let palette = Palette::dark();

    // ── header: icon + title + description ──────────────────────────
    let icon_widget = header_icon(role_icon(group), palette);
    let title = text(group.label().to_string())
        .size(26)
        .color(palette.text.into_iced_color());
    let description = text(role_description(group).to_string())
        .size(13)
        .color(palette.text_muted.into_iced_color());
    let header = row![
        icon_widget,
        Space::new().width(Length::Fixed(14.0)),
        column![title, Space::new().height(Length::Fixed(4.0)), description].spacing(0),
    ]
    .align_y(iced::alignment::Vertical::Center);

    // ── tasks: one action-link per panel in this group ──────────────
    let panels = role_action_panels(group);
    let mut task_links: Vec<Element<'a, Message>> = vec![section_label("Tasks", palette)];
    for panel in panels {
        task_links.push(action_link(panel.label(), group, panel.slug(), palette));
    }
    let tasks_col = column(task_links).spacing(4).width(Length::FillPortion(3));

    // ── see also: related-role jumps ────────────────────────────────
    let mut see_also_links: Vec<Element<'a, Message>> = vec![section_label("See also", palette)];
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
    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    iced::widget::container(content)
        .padding(Padding::from([24u16, 28u16]))
        .width(Length::Fill)
        .style(move |_t: &Theme| iced::widget::container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..iced::widget::container::Style::default()
        })
        .into()
}

/// The card-header icon (PanelHeader size), falling back to the glyph
/// char when no SVG resolves.
fn header_icon<'a>(icon: Icon, palette: Palette) -> Element<'a, Message> {
    let resolved = mde_icon(icon, IconSize::PanelHeader);
    if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        let tint = palette.text.into_iced_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style { color: Some(tint) },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .color(palette.text.into_iced_color())
            .into()
    }
}

/// A small uppercase section divider label ("Tasks" / "See also").
fn section_label<'a>(label: &'static str, palette: Palette) -> Element<'a, Message> {
    text(label)
        .size(11)
        .color(palette.text_muted.into_iced_color())
        .into()
}

/// One Tasks action-link → opens the named panel under `group`.
fn action_link<'a>(
    label: &'static str,
    group: Group,
    panel_slug: &'static str,
    palette: Palette,
) -> Element<'a, Message> {
    let accent = palette.accent.into_iced_color();
    button(text(label.to_string()).size(13).color(accent))
        .padding(Padding::from([4u16, 8u16]))
        .style(link_button_style(palette))
        .on_press(Message::SelectPanel {
            group,
            panel: panel_slug,
        })
        .into()
}

/// One See-also link → jumps to the related role's landing.
fn group_link<'a>(group: Group, palette: Palette) -> Element<'a, Message> {
    let muted = palette.text_muted.into_iced_color();
    button(text(group.label().to_string()).size(13).color(muted))
        .padding(Padding::from([4u16, 8u16]))
        .style(link_button_style(palette))
        .on_press(Message::SelectGroup(group))
        .into()
}

/// Shared transparent link-button style: no chrome at rest, a faint
/// raised tint on hover (tint of the palette token, no raw hex).
fn link_button_style(
    palette: Palette,
) -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    let raised = palette.raised.into_iced_color();
    move |_t: &Theme, status: iced::widget::button::Status| {
        let hover_bg = Color {
            r: raised.r * 1.12,
            g: raised.g * 1.12,
            b: raised.b * 1.12,
            a: raised.a,
        };
        iced::widget::button::Style {
            snap: false,
            background: match status {
                iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
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
            shadow: iced::Shadow::default(),
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
            let _: Element<'_, Message> = role_landing(g);
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
    fn fleet_role_card_links_match_the_e6_5_acceptance() {
        // E6.5 acceptance #1: the Fleet role card surfaces action-links
        // to Inventory / Playbooks / Run-History / Settings / Revisions.
        // Locks the Fleet role's task set to the acceptance (each panel
        // is wired in app.rs::panel_body, so each link opens its
        // backend, not the not-ready empty-state).
        let slugs: Vec<&str> = role_action_panels(Group::Fleet)
            .iter()
            .map(Panel::slug)
            .collect();
        assert_eq!(
            slugs,
            vec![
                "inventory",
                "playbooks",
                "run_history",
                "settings",
                "revisions"
            ]
        );
    }

    #[test]
    fn look_and_feel_role_card_includes_the_e6_6_acceptance_panels() {
        // E6.6 acceptance #1: the Look & Feel role card surfaces
        // action-links to Themes, Wallpaper, Fonts, and Window Manager
        // (window_manager moved here from System). sync_status stays as
        // a bonus link.
        let slugs: Vec<&str> = role_action_panels(Group::LookAndFeel)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in ["themes", "wallpaper", "fonts", "window_manager"] {
            assert!(
                slugs.contains(&want),
                "Look & Feel card missing {want}: {slugs:?}"
            );
        }
    }

    #[test]
    fn apps_role_card_includes_the_e6_3_acceptance_panels() {
        // E6.3 acceptance #1: the Apps role card surfaces action-links to
        // Install / Installed / Remove / Sources / Default-Apps (install &
        // remove added to nav_model — they were already wired; default_apps
        // moved here from System). `panel` (Panel Apps) stays as a bonus.
        let slugs: Vec<&str> = role_action_panels(Group::Apps)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in ["install", "installed", "remove", "sources", "default_apps"] {
            assert!(slugs.contains(&want), "Apps card missing {want}: {slugs:?}");
        }
    }

    #[test]
    fn default_apps_left_system_for_apps() {
        // E6.3 — default_apps moved out of System into Apps; it must not
        // appear under both.
        let system: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        assert!(
            !system.contains(&"default_apps"),
            "default_apps must leave System (E6.3): {system:?}"
        );
    }

    #[test]
    fn devices_role_card_includes_the_e6_4_acceptance_panels() {
        // E6.4 acceptance #1: the Devices role card surfaces action-links
        // to the 9 device panels (displays/sound/printers/removable/
        // keyboard/mouse/session/power/connect). `music` stays as a bonus
        // pending the E5.3 Media Player app.
        let slugs: Vec<&str> = role_action_panels(Group::Devices)
            .iter()
            .map(Panel::slug)
            .collect();
        for want in [
            "displays",
            "sound",
            "printers",
            "removable",
            "keyboard",
            "mouse",
            "session",
            "power",
            "connect",
        ] {
            assert!(
                slugs.contains(&want),
                "Devices card missing {want}: {slugs:?}"
            );
        }
    }

    #[test]
    fn session_left_system_for_devices() {
        // E6.4 — session moved out of System into Devices.
        let system: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        assert!(
            !system.contains(&"session"),
            "session must leave System (E6.4): {system:?}"
        );
    }

    #[test]
    fn help_role_card_has_index_and_about() {
        // E6.9 acceptance #1: the Help role card surfaces action-links to
        // the help topics index and the About/Help (disclaimer) surface.
        let slugs: Vec<&str> = role_action_panels(Group::Help)
            .iter()
            .map(Panel::slug)
            .collect();
        assert_eq!(slugs, vec!["index", "about"]);
    }

    #[test]
    fn maintain_role_card_matches_the_e6_7_acceptance() {
        // E6.7 acceptance #1: the Maintain role card surfaces action-links
        // to Hub / Snapshots / Debloat / Health / Repair / Drift. The
        // Maintain group root now renders the generic role card (was the
        // bespoke hub dashboard, which becomes the "Hub" sub-panel).
        let slugs: Vec<&str> = role_action_panels(Group::Maintain)
            .iter()
            .map(Panel::slug)
            .collect();
        assert_eq!(
            slugs,
            vec![
                "hub",
                "snapshots",
                "debloat",
                "health_check",
                "repair",
                "drift"
            ]
        );
    }

    #[test]
    fn system_role_card_matches_the_e6_8_acceptance() {
        // E6.8 acceptance #1: the System role card surfaces exactly
        // Date & Time / Logs / Resources / System Update / Notifications
        // (logs/resources/system_update surfaced here from Maintain, where
        // they were wired but orphaned from the nav).
        let slugs: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        assert_eq!(
            slugs,
            vec![
                "datetime",
                "logs",
                "resources",
                "system_update",
                "notifications"
            ]
        );
    }

    #[test]
    fn window_manager_left_system_for_look_and_feel() {
        // E6.6/E6.8 — window_manager moved out of System (E6.8's
        // acceptance excludes it) into Look & Feel; it must not appear
        // under both, or the sidebar would list it twice.
        let system: Vec<&str> = role_action_panels(Group::System)
            .iter()
            .map(Panel::slug)
            .collect();
        assert!(
            !system.contains(&"window_manager"),
            "window_manager must leave System (E6.6): {system:?}"
        );
    }
}
