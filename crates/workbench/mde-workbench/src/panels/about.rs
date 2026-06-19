//! E6.9 — Help → About: project identity, links, changelog, disclaimer.
//!
//! Single-sourced, never copy-pasted:
//! - the disclaimer is [`mde_disclaimer::TEXT`] (embedded from the
//!   repo-root `DISCLAIMER.md` at build time);
//! - the changelog is the repo-root `CHANGELOG.md`, embedded via
//!   `include_str!`;
//! - the version is `CARGO_PKG_VERSION` (the single workspace version
//!   every crate inherits).
//!
//! The GitHub / Releases / Contact rows open externally via
//! `crate::Message::OpenExternal` (dispatched to `xdg-open`).

use crate::cosmic_compat::prelude::*;
use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding};
use mde_theme::{FontSize, Palette, TypeRole};

/// Project repository (workspace `repository` field).
pub const REPO_URL: &str = "https://github.com/matthewmackes/magic-mesh";
/// Releases + changelog landing page.
pub const RELEASES_URL: &str = "https://github.com/matthewmackes/magic-mesh/releases";
/// Maintainer contact (workspace `authors` field).
pub const CONTACT_EMAIL: &str = "matthewmackes@gmail.com";
/// `mailto:` form of [`CONTACT_EMAIL`] handed to `xdg-open`.
pub const CONTACT_MAILTO: &str = "mailto:matthewmackes@gmail.com";
/// The single workspace version this build was cut from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The repo-root changelog, embedded so the About surface never drifts
/// from the file (mirrors the disclaimer single-source rule).
pub const CHANGELOG: &str = include_str!("../../../../../CHANGELOG.md");

#[derive(Debug, Clone, Copy, Default)]
pub struct AboutPanel;

impl AboutPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The canonical disclaimer text this panel renders — the single
    /// source ([`mde_disclaimer::TEXT`]), exposed so a test can assert
    /// the surface never drifts from `DISCLAIMER.md` (E6.9 acceptance #2).
    #[must_use]
    pub const fn disclaimer_text() -> &'static str {
        mde_disclaimer::TEXT
    }

    /// The changelog text this panel renders — the single source
    /// (repo-root `CHANGELOG.md`), exposed for the drift test.
    #[must_use]
    pub const fn changelog_text() -> &'static str {
        CHANGELOG
    }

    pub fn view<'a>() -> Element<'a, crate::Message, cosmic::Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        // BRAND-11 — the MCNF logo as the About hero (the predominant brand art),
        // from the Brand loader's LogoLockup slot (baked SVG fallback → the new
        // MCNF 11 mark). Square 1:1; rendered at a fixed hero height.
        let hero: Element<'a, crate::Message, cosmic::Theme> = {
            use cosmic::iced::widget::svg;
            let brand = mde_theme::Brand::new();
            let bytes = brand.bytes(mde_theme::BrandSlot::LogoLockup);
            // LogoLockup has no baked default; fall back to the Monogram slot.
            let bytes = if bytes.is_empty() {
                brand.bytes(mde_theme::BrandSlot::Monogram)
            } else {
                bytes
            };
            svg(svg::Handle::from_memory(bytes))
                .width(Length::Fixed(96.0))
                .height(Length::Fixed(96.0))
                .into()
        };
        let title = text("About MCNF — Mackes Cosmic Nebula Fedora")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        // The series codename tracks the major version: 11.x = "Winter-Is-Coming",
        // else 10.0.x = "Magic Mesh" (the package id `magic-mesh` = the codename).
        let codename = if VERSION.starts_with("11.") {
            "MCNF 11.0 \"Winter-Is-Coming\""
        } else {
            "MCNF 10.0 \"Magic Mesh\""
        };
        let version = text(format!("{codename} · v{VERSION}"))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        // ── Project links ──────────────────────────────────────────
        let links = column![
            section_header("Project", palette, sizes),
            Space::new().height(Length::Fixed(6.0)),
            link_row("GitHub", REPO_URL, REPO_URL, palette, sizes),
            link_row("Releases", RELEASES_URL, RELEASES_URL, palette, sizes),
            link_row("Contact", CONTACT_EMAIL, CONTACT_MAILTO, palette, sizes),
        ]
        .spacing(2);

        // ── Changelog (embedded, single-source) ────────────────────
        let changelog = column![
            section_header("Changelog", palette, sizes),
            Space::new().height(Length::Fixed(6.0)),
            text(Self::changelog_text())
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(0);

        // ── Disclaimer (existing single-source block) ──────────────
        let disclaimer = column![
            section_header("Disclaimer", palette, sizes),
            Space::new().height(Length::Fixed(6.0)),
            text(Self::disclaimer_text())
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(0);

        let col = column![
            hero,
            Space::new().height(Length::Fixed(12.0)),
            title,
            Space::new().height(Length::Fixed(4.0)),
            version,
            Space::new().height(Length::Fixed(20.0)),
            links,
            Space::new().height(Length::Fixed(24.0)),
            changelog,
            Space::new().height(Length::Fixed(24.0)),
            disclaimer,
        ]
        .spacing(0);

        container(scrollable(col))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// A Carbon section header (Heading role, muted-bright text).
fn section_header<'a>(
    label: &'a str,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message, cosmic::Theme> {
    text(label)
        .size(TypeRole::Heading.size_in(sizes))
        .colr(palette.text.into_cosmic_color())
        .into()
}

/// A clickable link row: a fixed-width muted label + the accent-colored
/// value; pressing it opens `url` via `xdg-open`.
fn link_row<'a>(
    label: &'a str,
    display: &'a str,
    url: &'static str,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let lbl = container(
        text(label)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    )
    .width(Length::Fixed(96.0));
    let value = text(display)
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.accent.into_cosmic_color());
    let inner = row![lbl, value].spacing(8);
    let accent = palette.accent.into_cosmic_color();
    button(inner)
        .padding(Padding::from([4u16, 0u16]))
        .sty(
            move |_t: &cosmic::Theme, _status: button::Status| button::Style {
                snap: false,
                background: Some(Background::Color(Color::TRANSPARENT)),
                text_color: accent,
                icon_color: Some(accent),
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: 0.0.into(),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
            },
        )
        .on_press(crate::Message::OpenExternal(url))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_without_panic() {
        let _ = AboutPanel::view();
    }

    #[test]
    fn about_renders_the_single_source_disclaimer() {
        // E6.9 acceptance #2: the About surface pulls DISCLAIMER.md via
        // mde_disclaimer (the single source), never a copy-paste. Lock
        // that the panel's text IS mde_disclaimer::TEXT and is non-empty.
        assert_eq!(AboutPanel::disclaimer_text(), mde_disclaimer::TEXT);
        assert!(
            !AboutPanel::disclaimer_text().trim().is_empty(),
            "disclaimer text must be embedded + non-empty"
        );
    }

    #[test]
    fn about_embeds_the_single_source_changelog() {
        // The changelog is embedded from the repo-root CHANGELOG.md, not
        // copy-pasted — assert it's non-empty and looks like the file.
        assert_eq!(AboutPanel::changelog_text(), CHANGELOG);
        assert!(
            AboutPanel::changelog_text().contains("# Changelog"),
            "changelog must be the embedded CHANGELOG.md"
        );
    }

    #[test]
    fn about_surfaces_github_and_contact() {
        // The task: GitHub + contact must be present.
        assert!(REPO_URL.contains("github.com/matthewmackes/magic-mesh"));
        assert!(CONTACT_MAILTO.starts_with("mailto:"));
        assert!(CONTACT_EMAIL.contains('@'));
        assert!(!VERSION.is_empty());
    }
}
