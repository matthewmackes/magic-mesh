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

// ── "Built on open source" acknowledgements (ABOUT-OSS) ──────────────────────
//
// MCNF stands on the shoulders of the open-source projects below — they are not
// incidental dependencies, they ARE the substrate: the mesh fabric, the
// coordination layer, the file plane, the desktop the GUIs run on, the async
// engine of the daemon. This section names each, credits its
// authors/maintainers, links to its official home (opened via
// `OpenExternal`/`xdg-open`), and says thank you.
//
// LOGOS — each project's real brand mark, vendored + rendered (the operator
// asked for logos). We ship a simple monochrome SVG of the brand mark under
// `assets/oss/` for the seven projects with a cleanly-available one (etcd,
// Syncthing, COSMIC→System76, iced, Tokio, Podman, OpenTofu — from SimpleIcons /
// the projects' own marks), vendored as bytes so the airgapped build host needs
// no fetch. The SVG is rendered tinted to the tile's `ink` Carbon token, so the
// mark reads on the Carbon fill and stays §4 single-source colour (no raw hex)
// and light/dark-adaptive. Nebula, Navidrome and age have no simple SVG we can
// cleanly vendor, so they keep the Carbon *monogram tile* fallback — `logo_tile`
// renders whichever applies, and "never ship a broken image" is guaranteed.
// The marks remain their owners' trademarks; they appear here only to credit and
// thank their projects (nominative use), see NOTICE.

/// One acknowledged open-source project: its name, the people behind it, the
/// official link (opened via [`crate::Message::OpenExternal`]), the monogram +
/// the Carbon tokens that fill/ink its tile, and one line on why MCNF leans on
/// it — with thanks.
struct OssProject {
    /// Project name (also the visible link label).
    name: &'static str,
    /// Author / maintainer / steward we credit.
    author: &'static str,
    /// Official home — repo or project site. `&'static str` so it hands
    /// straight to `OpenExternal`.
    url: &'static str,
    /// 1–2 char monogram rendered on the tile (stands in for the logo).
    initials: &'static str,
    /// Tile fill — a Carbon ramp token (read from `mde_theme`, never raw hex).
    tile: mde_theme::Rgba,
    /// Monogram ink — a Carbon token chosen for contrast against `tile`.
    ink: mde_theme::Rgba,
    /// Why it matters to MCNF + a thank-you (1–2 lines).
    blurb: &'static str,
}

/// The heartfelt lead for the section — value + gratitude.
const OSS_LEAD: &str = "MCNF is not built alone. Almost everything you rely on here \
rests on open-source projects maintained by people who chose to share their craft \
with the world — the encrypted fabric the mesh rides on, the layer that keeps the \
fleet in agreement, the file plane, the desktop these windows are drawn in, the \
async engine of the daemon. We stand on their shoulders, and we are deeply grateful. \
These are the ten projects MCNF leans on most directly — please support them.";

/// The closing thanks to the wider community.
const OSS_CLOSING: &str = "And beyond these ten: thank you to the wider Rust \
community and to every maintainer, contributor, translator, and bug-reporter whose \
open-source work quietly carries this platform. We are standing on your shoulders.";

/// The ten projects, top-down by how directly MCNF leans on them. Tile colours
/// are Carbon ramp tokens only (§4 single-source); ink tokens are picked for
/// contrast so each monogram stays legible in light and dark.
const OSS_PROJECTS: [OssProject; 10] = [
    OssProject {
        name: "Nebula",
        author: "Slack / Defined Networking, and contributors",
        url: "https://github.com/slackhq/nebula",
        initials: "N",
        tile: mde_theme::carbon::BLUE_70,
        ink: mde_theme::carbon::GRAY_10,
        blurb: "The encrypted overlay that IS the mesh: every peer-to-peer tunnel \
between MCNF nodes rides Nebula's lighthouse-and-certificate fabric. Thank you for \
making private, scalable networking something we could simply build on.",
    },
    OssProject {
        name: "etcd",
        author: "CNCF (originally CoreOS), and contributors",
        url: "https://etcd.io",
        initials: "e",
        tile: mde_theme::carbon::BLUE_60,
        ink: mde_theme::carbon::WHITE,
        blurb: "Our strongly-consistent source of truth — leader election and the \
shared state the Datacenter plane reconciles against. Thank you for a coordination \
layer we can stake the fleet's correctness on.",
    },
    OssProject {
        name: "Syncthing",
        author: "Jakob Borg and the Syncthing community",
        url: "https://syncthing.net",
        initials: "S",
        tile: mde_theme::carbon::TEAL_30,
        ink: mde_theme::carbon::GRAY_100,
        blurb: "The continuous, peer-to-peer file plane behind Mesh-Sync: folders \
that quietly converge across the mesh with no server in the middle. Thank you for \
years of dependable, private sync.",
    },
    OssProject {
        name: "COSMIC / libcosmic",
        author: "System76, and contributors",
        url: "https://github.com/pop-os/libcosmic",
        initials: "C",
        tile: mde_theme::carbon::GRAY_70,
        ink: mde_theme::carbon::GRAY_10,
        blurb: "The Rust desktop environment and toolkit our GUIs are built on — \
the Workbench you're reading this in is a libcosmic app. Thank you for a modern, \
native Rust desktop to stand on.",
    },
    OssProject {
        name: "iced",
        author: "Héctor Ramón Jiménez / iced-rs, and contributors",
        url: "https://iced.rs",
        initials: "i",
        tile: mde_theme::carbon::BLUE_40,
        ink: mde_theme::carbon::GRAY_100,
        blurb: "The cross-platform Rust GUI library under every Carbon surface here \
— the widgets, layout, and runtime drawing this very page. Thank you for making \
Rust GUIs a genuine joy to write.",
    },
    OssProject {
        name: "Tokio",
        author: "The Tokio project, and contributors",
        url: "https://tokio.rs",
        initials: "T",
        tile: mde_theme::carbon::GRAY_100,
        ink: mde_theme::carbon::GRAY_10,
        blurb: "The async runtime powering the mackesd daemon and nearly every \
networked task across the mesh. Thank you for the foundation that lets MCNF do many \
things at once, reliably.",
    },
    OssProject {
        name: "Navidrome",
        author: "Deluan Quintão, and contributors",
        url: "https://www.navidrome.org",
        initials: "Nv",
        tile: mde_theme::carbon::BLUE_50,
        ink: mde_theme::carbon::WHITE,
        blurb: "The self-hosted, Subsonic-compatible server behind the mesh music \
service — your library, streamed across your own nodes. Thank you for self-hosted \
music done right.",
    },
    OssProject {
        name: "Podman",
        author: "Red Hat / the Containers project, and contributors",
        url: "https://podman.io",
        initials: "P",
        tile: mde_theme::carbon::GRAY_60,
        ink: mde_theme::carbon::GRAY_10,
        blurb: "The daemonless container engine that runs mesh workloads without a \
privileged background service. Thank you for rootless, daemonless containers we can \
trust on every node.",
    },
    OssProject {
        name: "OpenTofu",
        author: "The Linux Foundation, and contributors",
        url: "https://opentofu.org",
        initials: "oT",
        tile: mde_theme::carbon::YELLOW_30,
        ink: mde_theme::carbon::GRAY_100,
        blurb: "The open-source infrastructure-as-code engine the Datacenter plane \
provisions through — the fleet declared as code. Thank you for keeping \
infrastructure-as-code truly open.",
    },
    OssProject {
        name: "age",
        author: "Filippo Valsorda, and contributors",
        url: "https://age-encryption.org",
        initials: "a",
        tile: mde_theme::carbon::GREEN_60,
        ink: mde_theme::carbon::GRAY_10,
        blurb: "The modern, no-config file encryption securing the mesh secret \
store. Thank you for encryption that's simple enough to actually use correctly.",
    },
];

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

        // ── Built on open source (ABOUT-OSS) ───────────────────────
        // Heartfelt lead, then the ten projects (monogram + name link +
        // maintainer + a line of why-it-matters/thanks), then a closing line.
        let mut oss = column![
            section_header("Built on open source", palette, sizes),
            Space::new().height(Length::Fixed(6.0)),
            text(OSS_LEAD)
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            Space::new().height(Length::Fixed(18.0)),
        ]
        .spacing(0);
        for project in &OSS_PROJECTS {
            oss = oss.push(oss_entry(project, palette, sizes));
            oss = oss.push(Space::new().height(Length::Fixed(16.0)));
        }
        oss = oss.push(
            text(OSS_CLOSING)
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );

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
            Space::new().height(Length::Fixed(28.0)),
            oss,
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

/// One "Built on open source" entry: the monogram tile (left), then a column
/// with the name (a link button → [`crate::Message::OpenExternal`]), the
/// maintainer line, and the why-it-matters/thanks blurb.
fn oss_entry<'a>(
    project: &OssProject,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let tile = logo_tile(project, sizes);

    let name = oss_link_button(project.name, project.url, palette, sizes);
    let author = text(project.author)
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());
    let blurb = text(project.blurb)
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color());

    let body = column![
        name,
        author,
        Space::new().height(Length::Fixed(4.0)),
        blurb,
    ]
    .spacing(1)
    .width(Length::Fill);

    row![
        tile,
        Space::new().width(Length::Fixed(16.0)),
        body,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Top)
    .width(Length::Fill)
    .into()
}

/// The vendored monochrome SVG brand mark for `name`, or `None` for the three
/// projects (Nebula, Navidrome, age) that keep the Carbon monogram fallback.
/// Bytes are `include_bytes!`-embedded so the airgapped build host needs no fetch.
fn logo_bytes(name: &str) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match name {
        "etcd" => include_bytes!("../../assets/oss/etcd.svg"),
        "Syncthing" => include_bytes!("../../assets/oss/syncthing.svg"),
        "COSMIC / libcosmic" => include_bytes!("../../assets/oss/cosmic.svg"),
        "iced" => include_bytes!("../../assets/oss/iced.svg"),
        "Tokio" => include_bytes!("../../assets/oss/tokio.svg"),
        "Podman" => include_bytes!("../../assets/oss/podman.svg"),
        "OpenTofu" => include_bytes!("../../assets/oss/opentofu.svg"),
        _ => return None,
    };
    Some(bytes)
}

/// The logo slot: a rounded Carbon-token tile holding the project's real brand
/// mark — a vendored monochrome SVG tinted to the tile's `ink` Carbon token — when
/// one is available ([`logo_bytes`]), else the Carbon monogram. Both fill and ink
/// are `mde_theme` tokens, so it adapts in light/dark and never ships broken.
fn logo_tile<'a>(
    project: &OssProject,
    sizes: FontSize,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let fill = project.tile.into_cosmic_color();
    let ink = project.ink.into_cosmic_color();
    let inner: Element<'a, crate::Message, cosmic::Theme> =
        if let Some(bytes) = logo_bytes(project.name) {
            use cosmic::iced::widget::svg;
            svg(svg::Handle::from_memory(bytes))
                .sty(move |_t: &cosmic::Theme| svg::Style { color: Some(ink) })
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(28.0))
                .into()
        } else {
            text(project.initials)
                .size(TypeRole::Section.size_in(sizes))
                .colr(ink)
                .into()
        };
    container(inner)
        .center_x(Length::Fixed(48.0))
        .center_y(Length::Fixed(48.0))
        .style(move |_t: &cosmic::Theme| container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// An accent-colored, chromeless link button showing `label` and opening `url`
/// via `xdg-open` — the section-title link form used for each OSS project name
/// (mirrors [`link_row`]'s button styling).
fn oss_link_button<'a>(
    label: &'static str,
    url: &'static str,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let accent = palette.accent.into_cosmic_color();
    button(
        text(label)
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(accent),
    )
    .padding(Padding::from([0u16, 0u16]))
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
    fn oss_section_lists_exactly_ten_projects() {
        // ABOUT-OSS: the operator asked for the top ten projects MCNF leans on.
        assert_eq!(OSS_PROJECTS.len(), 10, "ten acknowledged projects");
    }

    #[test]
    fn oss_entries_are_well_formed() {
        // Each entry must carry a name, a maintainer credit, a clickable https
        // link (so OpenExternal/xdg-open opens something real), a 1–2 char
        // monogram for its tile, and a non-empty thank-you blurb.
        for p in &OSS_PROJECTS {
            assert!(!p.name.trim().is_empty(), "name present");
            assert!(!p.author.trim().is_empty(), "{}: maintainer present", p.name);
            assert!(
                p.url.starts_with("https://"),
                "{}: url must be an https link, got {:?}",
                p.name,
                p.url
            );
            let n = p.initials.chars().count();
            assert!(
                (1..=2).contains(&n),
                "{}: monogram must be 1–2 chars, got {:?}",
                p.name,
                p.initials
            );
            assert!(
                p.blurb.to_lowercase().contains("thank"),
                "{}: blurb must include a thank-you",
                p.name
            );
        }
    }

    #[test]
    fn oss_section_covers_the_named_substrate() {
        // The ten are repo-grounded substrate — lock that the headline projects
        // are actually named so the section can't silently lose one.
        let names: Vec<&str> = OSS_PROJECTS.iter().map(|p| p.name).collect();
        for required in [
            "Nebula",
            "etcd",
            "Syncthing",
            "iced",
            "Tokio",
            "Navidrome",
            "Podman",
            "OpenTofu",
            "age",
        ] {
            assert!(
                names.iter().any(|n| n.contains(required)),
                "missing acknowledged project: {required}"
            );
        }
        // libcosmic / COSMIC is named as the desktop substrate.
        assert!(
            names.iter().any(|n| n.contains("cosmic") || n.contains("COSMIC")),
            "missing libcosmic / COSMIC"
        );
        // The section prose leads with gratitude and closes on the wider community.
        assert!(OSS_LEAD.to_lowercase().contains("grateful"));
        assert!(OSS_CLOSING.to_lowercase().contains("thank you"));
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
