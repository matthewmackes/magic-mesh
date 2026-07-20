//! `Surface::About` - the canonical "about this platform" panel
//! (Construct brand design `docs/design/construct-branding.md`, placement lock #13).
//!
//! One honest identity screen, folded entirely from the single brand source of
//! truth in [`mde_theme::brand`] (§6 — nothing is re-embedded or re-derived here):
//!
//! * the user-facing **Construct identity block**, rendered from text constants
//!   instead of legacy raster wordmark art;
//! * the user-facing **Construct identity** ([`brand::logo::PRODUCT_NAME`],
//!   [`brand::logo::SOFTWARE_STUDIO`], and [`brand::logo::PRODUCT_RELEASE`],
//!   lock #10);
//! * the complete **build identity** ([`brand::build::full`] — version · codename ·
//!   git hash · date · channel), the exact string `mde-shell-egui --version` prints,
//!   single-sourced so the About panel and `--version` can never drift;
//! * the shipped **legal docs** (LICENSE / NOTICE / DISCLAIMER) referenced at their
//!   packaged runtime paths, plus the project source URL.
//!
//! Every colour + metric is a Carbon [`Style`] token (§4); the panel holds no live
//! state and drives no worker — it is a pure renderer of compile-time constants.
//!
//! **Honest degradation (§7):** the identity block is text-first, and a legal
//! doc that is not present at its runtime path is named without a dead link (a
//! dev / non-RPM build ships the docs at the repo root instead).

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the shell body in main.rs consumes them"
)]

use mde_egui::egui::{self, RichText};
use mde_egui::{field, muted_note, Style};
use mde_theme::brand;

/// The canonical project source URL, single-sourced from the workspace
/// `Cargo.toml` `repository` field (inherited by this crate) — never a
/// hand-copied string that could drift.
const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// The shipped legal docs and their packaged (RPM) runtime paths — where the
/// `cargo generate-rpm` assets install them (`crates/mesh/mackesd/Cargo.toml`).
/// Referenced, not embedded: the About panel names each doc and, when the
/// package is installed, shows the on-disk path (§7 — no dead link otherwise).
const LEGAL_DOCS: [(&str, &str); 3] = [
    ("LICENSE", "/usr/share/licenses/magic-mesh/LICENSE"),
    ("NOTICE", "/usr/share/licenses/magic-mesh/NOTICE"),
    ("DISCLAIMER", "/usr/share/magic-mesh/DISCLAIMER.md"),
];

/// Render the About surface into `ui` — the product name, tagline, full build
/// identity, and shipped legal docs + source URL.
///
/// A pure renderer over the [`mde_theme::brand`] constants: it takes only `ui`
/// (no shell state), mirroring the free-function surfaces (`workbench::show`, the
/// Music/Media panels) the shell body drives.
pub(crate) fn about_panel(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(Style::SP_L);

            // ── Header — the visible Construct product identity ──
            ui.vertical_centered(|ui| {
                // The user-facing product identity is always text,
                // avoiding legacy raster wordmarks during the Construct rename.
                ui.label(
                    RichText::new(brand::logo::PRODUCT_NAME)
                        .color(Style::TEXT)
                        .size(Style::HEADING)
                        .strong(),
                );
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(brand::logo::SOFTWARE_STUDIO)
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .strong(),
                );
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(brand::logo::PRODUCT_RELEASE)
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .strong(),
                );
            });

            ui.add_space(Style::SP_L);
            build_section(ui);
            ui.add_space(Style::SP_M);
            legal_section(ui);
            ui.add_space(Style::SP_L);
        });
}

/// A titled section: a dim caption over the shared raised **card** primitive
/// ([`mde_egui::card`] — the single source of the surface fill, hairline, mid
/// radius, comfortable padding, and the `Elevation::Raised` soft shadow). The
/// About build / legal cards now read the whole look from the foundation instead
/// of a hand-rolled group frame plus a per-surface shadow cast, so the surface
/// mints nothing of its own (§4, design lock #2).
fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(
        RichText::new(title)
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    mde_egui::card().show(ui, body);
}

/// The build-identity card — the version fields broken out for scanning, then
/// the complete [`brand::build::full`] line (the exact `--version` string,
/// single-sourced so the two can never drift).
fn build_section(ui: &mut egui::Ui) {
    let info = brand::build::info();
    section(ui, "BUILD", |ui| {
        field(ui, "Release", brand::logo::PRODUCT_RELEASE, Style::TEXT);
        field(ui, "Version", &brand::build::version_line(), Style::TEXT);
        field(ui, "Codename", named_or_dash(info.codename), Style::TEXT);
        field(ui, "Git hash", info.git_hash, Style::TEXT);
        field(ui, "Build date", info.build_date, Style::TEXT);
        field(ui, "Channel", info.channel, Style::TEXT);
        ui.add_space(Style::SP_XS);
        // The full build stamp verbatim — the string `--version` prints.
        ui.label(
            RichText::new(brand::build::full())
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .monospace(),
        );
    });
}

/// The legal card — each shipped doc at its packaged runtime path (or named
/// alone when absent, §7), plus the project source URL.
fn legal_section(ui: &mut egui::Ui) {
    section(ui, "LEGAL", |ui| {
        muted_note(ui, "Shipped with the package:");
        ui.add_space(Style::SP_XS);
        for (name, path) in LEGAL_DOCS {
            legal_row(ui, name, path);
        }
        ui.add_space(Style::SP_XS);
        field(ui, "Source", REPO_URL, Style::TEXT_DIM);
    });
}

/// One legal-doc row. If the packaged doc is present at its runtime path we show
/// that path; otherwise (a dev / non-RPM build) we name the doc alone — never a
/// dead link to a file that isn't there (§7).
fn legal_row(ui: &mut egui::Ui, name: &str, path: &str) {
    if std::path::Path::new(path).exists() {
        field(ui, name, path, Style::TEXT);
    } else {
        field(ui, name, "bundled with the package", Style::TEXT_DIM);
    }
}

/// A codename for display, or an em-dash placeholder when the epoch has no name
/// (so the field is never blank).
const fn named_or_dash(codename: &str) -> &str {
    if codename.is_empty() {
        "—"
    } else {
        codename
    }
}

#[cfg(test)]
mod tests {
    use super::{about_panel, named_or_dash, LEGAL_DOCS, REPO_URL};
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_theme::brand;

    #[test]
    fn the_about_panel_renders_headless_with_construct_identity() {
        // Drive one headless frame (the same Context::run → tessellate path the DRM
        // runner uses, minus the GPU): it must draw primitives without panicking.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(720.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, about_panel);
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the About panel drew nothing");
    }

    #[test]
    fn the_panel_surfaces_the_product_identity_and_full_build_line() {
        // The About panel folds the single brand source of truth: the locked
        // Construct identity (lock #10) and the complete build stamp (`--version`).
        assert_eq!(brand::logo::PRODUCT_NAME, "Construct");
        assert_eq!(brand::logo::PRODUCT_TAGLINE, brand::logo::SOFTWARE_STUDIO);
        assert_eq!(brand::logo::PRODUCT_RELEASE, "Release 1.0 BETA");
        assert_eq!(brand::logo::SOFTWARE_STUDIO, "Software Studio: MDE");
        let full = brand::build::full();
        let info = brand::build::info();
        assert!(full.contains(info.version), "build line missing version");
        assert!(full.contains(info.git_hash), "build line missing git hash");
        assert!(full.contains(info.build_date), "build line missing date");
        assert!(full.contains(info.channel), "build line missing channel");
    }

    #[test]
    fn legal_docs_name_the_shipped_files_at_absolute_paths() {
        let names: Vec<&str> = LEGAL_DOCS.iter().map(|(n, _)| *n).collect();
        for want in ["LICENSE", "NOTICE", "DISCLAIMER"] {
            assert!(
                names.contains(&want),
                "{want} missing from the About legal list"
            );
        }
        // Each reference is an absolute packaged path (the RPM install dest), never
        // a relative link that would dead-end.
        for (_, path) in LEGAL_DOCS {
            assert!(
                path.starts_with('/'),
                "{path} is not an absolute packaged path"
            );
        }
    }

    #[test]
    fn repo_url_is_the_single_sourced_project_url() {
        assert!(
            REPO_URL.starts_with("https://"),
            "repo url not a url: {REPO_URL}"
        );
        assert!(
            REPO_URL.contains("magic-mesh"),
            "repo url not single-sourced: {REPO_URL}"
        );
    }

    #[test]
    fn codename_placeholder_when_the_epoch_is_unnamed() {
        assert_eq!(named_or_dash("Construct"), "Construct");
        assert_eq!(named_or_dash(""), "—");
    }
}
