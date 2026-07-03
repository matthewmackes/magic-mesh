//! `Surface::About` — the canonical "about this platform" panel (QBRAND-6,
//! design `docs/design/quasar-branding.md`, placement lock #13).
//!
//! One honest identity screen, folded entirely from the single brand source of
//! truth in [`mde_theme::brand`] (§6 — nothing is re-embedded or re-derived here):
//!
//! * the **horizontal lockup** ([`brand::logo::lockup_horizontal`], the official
//!   `MDE-QUAZAR-MAIN.png`), decoded + uploaded through the shell's shared
//!   [`decode_png_rgba`] path — the very path the boot-splash and the dock use — and
//!   cached once in egui memory so the ≈1-shot decode never repeats per frame;
//! * the user-facing **product name + tagline** ([`brand::logo::PRODUCT_NAME`] /
//!   [`brand::logo::PRODUCT_TAGLINE`], lock #10);
//! * the complete **build identity** ([`brand::build::full`] — version · codename ·
//!   git hash · date · channel), the exact string `mde-shell-egui --version` prints,
//!   single-sourced so the About panel and `--version` can never drift;
//! * the shipped **legal docs** (LICENSE / NOTICE / DISCLAIMER) referenced at their
//!   packaged runtime paths, plus the project source URL.
//!
//! Every colour + metric is a Carbon [`Style`] token (§4); the panel holds no live
//! state and drives no worker — it is a pure renderer of compile-time constants.
//!
//! **Honest degradation (§7):** an undecodable lockup falls back to the text name
//! alone (never a broken-image box), and a legal doc that is not present at its
//! runtime path is named without a dead link (a dev / non-RPM build ships the docs
//! at the repo root instead).

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, …); the shell body in main.rs consumes them"
)]

use mde_egui::egui::{self, RichText, TextureHandle, TextureOptions};
use mde_egui::{field, muted_note, Style};
use mde_theme::brand;

use crate::chooser::decode_png_rgba;

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

/// The widest the lockup letterboxes into (logical points) — a comfortable
/// header band that never spans an ultrawide seat edge-to-edge.
const LOCKUP_MAX_W: f32 = Style::SP_XL * 14.0;
/// The tallest the lockup letterboxes into (logical points).
const LOCKUP_MAX_H: f32 = Style::SP_XL * 4.0;

/// Render the About surface into `ui` — the lockup header, the product name +
/// tagline, the full build identity, and the shipped legal docs + source URL.
///
/// A pure renderer over the [`mde_theme::brand`] constants: it takes only `ui`
/// (no shell state), mirroring the free-function surfaces (`workbench::show`, the
/// Music/Media panels) the shell body drives.
pub(crate) fn about_panel(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(Style::SP_L);

            // ── Header — the brand lockup, then the product name + tagline ──
            ui.vertical_centered(|ui| {
                // The official horizontal lockup, decoded + uploaded through the
                // shared path and cached once. A decode failure draws nothing here
                // and falls through to the text name below (§7).
                if let Some(tex) = lockup_texture(ui.ctx()) {
                    let size = lockup_size(ui.available_width(), tex.size());
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        size,
                    )));
                    ui.add_space(Style::SP_M);
                }
                // The user-facing product name + tagline (lock #10) — always drawn,
                // so an undecodable lockup still names the platform honestly.
                ui.label(
                    RichText::new(brand::logo::PRODUCT_NAME)
                        .color(Style::TEXT)
                        .size(Style::HEADING)
                        .strong(),
                );
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(brand::logo::PRODUCT_TAGLINE)
                        .color(Style::TEXT_DIM)
                        .size(Style::BODY),
                );
            });

            ui.add_space(Style::SP_L);
            build_section(ui);
            ui.add_space(Style::SP_M);
            legal_section(ui);
            ui.add_space(Style::SP_L);
        });
}

/// A titled section: a dim caption over a grouped card — the shared surface
/// idiom (mirrors `system::section`).
fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(
        RichText::new(title)
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.group(body);
}

/// The build-identity card — the version fields broken out for scanning, then
/// the complete [`brand::build::full`] line (the exact `--version` string,
/// single-sourced so the two can never drift).
fn build_section(ui: &mut egui::Ui) {
    let info = brand::build::info();
    section(ui, "BUILD", |ui| {
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

/// Decode + upload the official horizontal lockup once, cached in egui memory so
/// the decode + GPU upload happen on the first paint only and every later frame
/// takes a cheap ref-counted [`TextureHandle`] clone (the `dock::icon_texture`
/// pattern). A failed decode caches `None`, so a broken asset fails soft to the
/// text name (§7) without retrying every frame.
fn lockup_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let key = egui::Id::new("qbrand6-about-lockup");
    // Fast path: the resolved texture (or a cached `None`) is already in memory.
    if let Some(cached) = ctx.data_mut(|d| d.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    // Slow path (first paint): decode the embedded PNG through the shared path
    // and upload it, then cache the handle.
    let texture = decode_png_rgba(brand::logo::lockup_horizontal())
        .map(|img| ctx.load_texture("qbrand6-about-lockup", img, TextureOptions::LINEAR));
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    texture
}

/// Letterbox the `[w, h]` lockup into the header band: scale to fit within
/// [`LOCKUP_MAX_W`]×[`LOCKUP_MAX_H`] (and the available width), aspect preserved,
/// never upscaled past the box. A degenerate size yields [`egui::Vec2::ZERO`]
/// (no NaN, no panic).
#[allow(
    clippy::cast_precision_loss,
    reason = "lockup pixel dimensions are far below f32's exact-integer range"
)]
fn lockup_size(available_width: f32, tex: [usize; 2]) -> egui::Vec2 {
    let (tw, th) = (tex[0] as f32, tex[1] as f32);
    if tw <= 0.0 || th <= 0.0 {
        return egui::Vec2::ZERO;
    }
    let box_w = available_width.min(LOCKUP_MAX_W);
    let scale = (box_w / tw).min(LOCKUP_MAX_H / th);
    egui::vec2(tw * scale, th * scale)
}

#[cfg(test)]
mod tests {
    use super::{
        about_panel, lockup_size, lockup_texture, named_or_dash, LEGAL_DOCS, LOCKUP_MAX_H,
        LOCKUP_MAX_W, REPO_URL,
    };
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_theme::brand;

    #[test]
    fn the_about_panel_renders_headless_and_uploads_the_lockup() {
        // Drive one headless frame (the same Context::run → tessellate path the DRM
        // runner uses, minus the GPU): it must draw primitives without panicking,
        // and the embedded lockup must decode + upload through the shared path.
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
        assert!(
            lockup_texture(&ctx).is_some(),
            "the About lockup failed to decode + upload"
        );
    }

    #[test]
    fn the_panel_surfaces_the_product_identity_and_full_build_line() {
        // The About panel folds the single brand source of truth: the locked
        // name/tagline (lock #10) and the complete build stamp (`--version`).
        assert_eq!(brand::logo::PRODUCT_NAME, "MDE Quazar");
        assert_eq!(brand::logo::PRODUCT_TAGLINE, "Mackes Display Environment");
        let full = brand::build::full();
        let info = brand::build::info();
        assert!(full.contains(info.version), "build line missing version");
        assert!(full.contains(info.git_hash), "build line missing git hash");
        assert!(full.contains(info.build_date), "build line missing date");
        assert!(full.contains(info.channel), "build line missing channel");
    }

    #[test]
    fn lockup_size_letterboxes_within_the_box_and_preserves_aspect() {
        // A wide lockup into a generous box: bounded by the box, aspect preserved.
        let s = lockup_size(4000.0, [1000, 250]);
        assert!(
            s.x <= LOCKUP_MAX_W + 0.5 && s.y <= LOCKUP_MAX_H + 0.5,
            "lockup exceeds the box: {s:?}"
        );
        let (aspect_in, aspect_out) = (1000.0 / 250.0, s.x / s.y);
        assert!(
            (aspect_in - aspect_out).abs() < 0.01,
            "aspect not preserved: in {aspect_in} out {aspect_out}"
        );
        // A narrow available width clamps the drawn width to it.
        let narrow = lockup_size(100.0, [1000, 250]);
        assert!(
            narrow.x <= 100.0 + 0.5,
            "narrow width not clamped: {narrow:?}"
        );
        // Degenerate size → zero (no NaN / panic).
        assert_eq!(lockup_size(200.0, [0, 0]), egui::Vec2::ZERO);
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
        assert_eq!(named_or_dash("Quazar"), "Quazar");
        assert_eq!(named_or_dash(""), "—");
    }
}
