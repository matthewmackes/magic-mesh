//! The terminal grid's bundled monospace font.
//!
//! **Intel One Mono** (SIL OFL-1.1 — `assets/fonts/IntelOneMono-NOTICE.txt`)
//! is embedded in the crate so the terminal grid renders a crisp, clean monospace
//! face on the immutable bootc image, with no dependency on a system-installed
//! font. This is the platform default (the shared `Style` installs the same face);
//! it renders a plain monospace with **no programming ligatures** — the family's
//! 1.4 ligatures live behind the opt-in `ss01` stylistic set, which egui never
//! activates, so the terminal still trades away the earlier Fira Code ligature
//! face (TERM-13). The grid paints through `FontId::monospace` (see
//! [`crate::widget`]); [`install`] registers the embedded face at the **highest
//! priority** of egui's `Monospace` family, so the grid — and the terminal's own
//! monospace chrome — draw with the bundled font.
//!
//! [`install`] uses `Context::add_font`, which is **additive**: it layers the
//! face onto whatever the shared `Style::install` already set (§4), rather than
//! replacing the font set, so the Proportional family and the emoji/CJK
//! fallbacks stay intact.

use mde_egui::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use mde_egui::egui::{Context, FontData, FontFamily};

/// The embedded Intel One Mono face (SIL OFL-1.1) — a clean monospace font
/// (no programming ligatures without the `ss01` stylistic set, which egui
/// never activates). An OpenType/CFF `.otf`; egui's `ttf-parser` backend
/// reads CFF outlines natively.
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");

/// Key for the bundled terminal face in egui's font map.
const TERM_FONT_KEY: &str = "mde-term-mono";

/// Register the bundled monospace font as the terminal grid's monospace face.
///
/// Called once from the surface binary's setup (after `Style::install`); the
/// call is idempotent — egui skips the reload once the face is present — so an
/// embedder may safely call it again.
pub fn install(ctx: &Context) {
    ctx.add_font(FontInsert::new(
        TERM_FONT_KEY,
        FontData::from_static(INTEL_ONE_MONO),
        // Highest in Monospace: the grid's `FontId::monospace` glyphs resolve to
        // the bundled face first, egui's built-ins staying as fallback.
        vec![InsertFontFamily {
            family: FontFamily::Monospace,
            priority: FontPriority::Highest,
        }],
    ));
}

#[cfg(test)]
mod tests {
    use super::{install, INTEL_ONE_MONO};

    #[test]
    fn intel_one_mono_face_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty OpenType/CFF face (magic
        // `OTTO`) — not a stray/missing asset.
        assert!(
            INTEL_ONE_MONO.len() > 50_000,
            "the bundled monospace OTF looks too small ({} bytes)",
            INTEL_ONE_MONO.len()
        );
        assert_eq!(&INTEL_ONE_MONO[0..4], b"OTTO");
    }

    #[test]
    fn install_registers_the_grid_font_headless() {
        // Registering the terminal face must work without a GPU (CPU-only
        // Context), and a frame that lays out monospace text must succeed —
        // this forces egui to actually parse the embedded CFF face.
        let ctx = mde_egui::egui::Context::default();
        install(&ctx);
        let _ = ctx.run(mde_egui::egui::RawInput::default(), |ctx| {
            mde_egui::egui::CentralPanel::default().show(ctx, |ui| {
                ui.monospace("grid glyphs");
            });
        });
    }
}
