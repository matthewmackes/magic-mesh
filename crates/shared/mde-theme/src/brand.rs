//! Brand asset loader.
//!
//! Maps logical brand-asset slots (wordmark, monogram, app icon,
//! greeter art, logo lockup) to bytes. Every slot has a baked
//! fallback so the shell always renders something; an
//! `$MDE_BRAND_DIR` override and a `/usr/share/mde/brand/`
//! system layer let operators and end-users swap artwork
//! without rebuilding.
//!
//! The full slot table + production workflow lives in
//! `assets/brand/README.md`. Lock authority: the brand-pack
//! direction from the 2026-05-21 branding survey (BR-1..BR-5).
//!
//! ## Resolution order
//!
//! For each [`BrandSlot`], the loader probes every candidate file
//! extension at every layer in order, first hit wins:
//!
//! 1. `$MDE_BRAND_DIR/<basename>.<ext>` for each ext in
//!    [`BrandSlot::search_exts`] — dev / per-user override.
//! 2. `/usr/share/mde/brand/<basename>.<ext>` — system install
//!    (RPM-supplied).
//! 3. Baked fallback via `include_bytes!` from
//!    `assets/brand/baked/`.
//!
//! Vector formats (`svg`) win over raster (`png`) when both are
//! present in the same layer — SVG scales, can be tinted, and
//! generally renders better in UI contexts. Slots that are
//! intrinsically raster (greeter background) declare only `png`.

use std::env;
use std::path::{Path, PathBuf};

/// Default system-install path for the brand pack. The RPM lays
/// down `assets/brand/*` here so `/usr/bin/mde-*` binaries pick
/// them up without an env var. Kept `pub` so packagers can
/// verify the path matches what they ship.
pub const SYSTEM_BRAND_DIR: &str = "/usr/share/mde/brand";

/// Env var that lets a developer or end-user point the shell at
/// an alternate brand directory without reinstalling.
pub const BRAND_DIR_ENV: &str = "MDE_BRAND_DIR";

/// Logical brand-asset slots. Each variant maps to a single
/// basename ([`BrandSlot::basename`]), a set of acceptable file
/// extensions ([`BrandSlot::search_exts`]), and a baked fallback
/// ([`BrandSlot::fallback_bytes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrandSlot {
    /// "Mackes / MDE" wordmark, used in the sidebar header and the
    /// About panel header. 4:1 aspect.
    Wordmark,
    /// Hero-treatment wordmark for the About panel and greeter
    /// foreground. 4:1 aspect, visible at 600 px+ wide.
    WordmarkHero,
    /// "MDE" monogram for empty states and favicon-scale uses.
    /// 1:1 aspect.
    Monogram,
    /// Shipped window/taskbar app icon. Carries the brand palette
    /// baked-in (charcoal background, white monogram); **not**
    /// tintable, because it must render consistently across host
    /// taskbars regardless of their theme. 1:1 aspect.
    AppIcon,
    /// Greeter background image (raster). No baked fallback — the
    /// shell renders a flat charcoal background if this slot is
    /// missing. 16:9-ish landscape aspect.
    GreeterHero,
    /// Wordmark variant pinned for use over the greeter
    /// background. White (or contrasting) by design; do not tint.
    /// 4:1 aspect.
    GreeterWordmark,
    /// Stacked "Mackes / MDE" lockup — the square (1:1) brand mark
    /// that combines the parent brand name with the monogram into
    /// one composition. Used in the About panel hero, splash
    /// surfaces, and any context that wants the full brand
    /// identity in a square footprint.
    LogoLockup,
}

/// Detected file format of a loaded slot's bytes. Lets consumers
/// pick the right Iced widget (`svg::Handle` vs `image::Handle`)
/// without re-sniffing the bytes themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrandFormat {
    /// SVG (`image/svg+xml`).
    Svg,
    /// PNG (`image/png`).
    Png,
}

impl BrandFormat {
    /// File extension (no dot) for this format.
    #[must_use]
    pub const fn ext(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }

    fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "svg" => Some(Self::Svg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }
}

impl BrandSlot {
    /// The basename (no extension) used at every resolution layer.
    /// Pair with [`Self::search_exts`] to construct candidate
    /// filenames.
    #[must_use]
    pub const fn basename(self) -> &'static str {
        match self {
            Self::Wordmark => "wordmark",
            Self::WordmarkHero => "wordmark-hero",
            Self::Monogram => "monogram",
            Self::AppIcon => "app-icon",
            Self::GreeterHero => "greeter-hero",
            Self::GreeterWordmark => "greeter-wordmark",
            Self::LogoLockup => "logo-lockup",
        }
    }

    /// File extensions probed in priority order. SVG wins over
    /// PNG when both are present in the same directory because
    /// SVG scales cleanly and supports `currentColor` tinting.
    /// [`Self::GreeterHero`] declares only `png` — it is
    /// intrinsically raster (photographic / gradient background).
    #[must_use]
    pub const fn search_exts(self) -> &'static [&'static str] {
        match self {
            Self::GreeterHero => &["png"],
            _ => &["svg", "png"],
        }
    }

    /// Canonical filename for documentation, packaging manifests,
    /// and the baked fallback. This is the file the loader will
    /// find first in a freshly populated brand directory.
    #[must_use]
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Wordmark => "wordmark.png",
            Self::WordmarkHero => "wordmark-hero.png",
            Self::Monogram => "monogram.png",
            Self::AppIcon => "app-icon.png",
            Self::GreeterHero => "greeter-hero.png",
            Self::GreeterWordmark => "greeter-wordmark.png",
            Self::LogoLockup => "logo-lockup.png",
        }
    }

    /// Baked fallback bytes, used only when no override and no
    /// system file exist. The shipped assets in `assets/brand/`
    /// take precedence; these baked SVGs in `assets/brand/baked/`
    /// are the "the install is corrupted but render something"
    /// safety net.
    ///
    /// [`Self::GreeterHero`] and [`Self::LogoLockup`] return an
    /// empty slice — they have no baked default and the consumer
    /// must handle absence (greeter falls back to a flat charcoal
    /// background; lockup callers should fall back to
    /// [`Self::Monogram`]).
    #[must_use]
    pub fn fallback_bytes(self) -> &'static [u8] {
        match self {
            Self::Wordmark => {
                include_bytes!("../../../../assets/brand/baked/wordmark.svg")
            }
            Self::WordmarkHero => {
                include_bytes!("../../../../assets/brand/baked/wordmark-hero.svg")
            }
            Self::Monogram => {
                include_bytes!("../../../../assets/brand/baked/monogram.svg")
            }
            Self::AppIcon => {
                include_bytes!("../../../../assets/brand/baked/app-icon.svg")
            }
            Self::GreeterHero => &[],
            Self::GreeterWordmark => {
                include_bytes!("../../../../assets/brand/baked/greeter-wordmark.svg")
            }
            Self::LogoLockup => &[],
        }
    }

    /// Format of the baked fallback bytes. All current bakes are
    /// SVG; tracked separately so a future swap to a baked PNG
    /// fallback is one match-arm change.
    #[must_use]
    pub const fn fallback_format(self) -> BrandFormat {
        match self {
            // Every baked fallback is SVG today. The empty-slice
            // slots still report SVG so consumers have a single
            // typed path; the empty bytes signal absence.
            _ => BrandFormat::Svg,
        }
    }

    /// Whether this slot is safe to tint at render time.
    ///
    /// BRAND-11 (2026-06-19): the MCNF 11 brand is a **fixed-palette** mark
    /// (the blue windowed-constellation logo), so **no slot is tintable** — every
    /// baked asset now embeds the full-color logo and must render as supplied so
    /// it looks right outside any particular host theme (the same reason `AppIcon`
    /// was always fixed-palette). Previously Wordmark/WordmarkHero/Monogram shipped
    /// monochrome `currentColor` artwork; the rebrand replaced them with the logo.
    ///
    /// Note: tintability is a property of the *baked default*. A runtime PNG
    /// override can't be tinted regardless; consumers that tint should check the
    /// resolved [`BrandFormat`].
    #[must_use]
    pub const fn is_tintable(self) -> bool {
        false
    }
}

/// Where a slot's bytes came from. Useful for diagnostic
/// surfaces (the About panel can show whether art is shipped,
/// overridden, or baked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrandSource {
    /// Loaded from `$MDE_BRAND_DIR/<file>`. Holds the resolved
    /// absolute path for display.
    Override(PathBuf),
    /// Loaded from `/usr/share/mde/brand/<file>`.
    System(PathBuf),
    /// Baked fallback via `include_bytes!`.
    Baked,
}

/// A fully resolved brand asset — bytes, format, and provenance.
#[derive(Debug, Clone)]
pub struct BrandAsset {
    /// The raw bytes. Empty when the slot has no baked default
    /// and no override / system file was found (currently only
    /// [`BrandSlot::GreeterHero`] and [`BrandSlot::LogoLockup`]).
    pub bytes: Vec<u8>,
    /// Detected format — lets consumers pick the right widget
    /// without re-sniffing the bytes.
    pub format: BrandFormat,
    /// Where the bytes came from.
    pub source: BrandSource,
}

impl BrandAsset {
    /// Whether this asset is empty (the slot has no baked default
    /// and nothing was found on disk).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Brand asset loader. Cheap to construct and clone; the only
/// state is two path roots and an optional override.
#[derive(Debug, Clone)]
pub struct Brand {
    override_dir: Option<PathBuf>,
    system_dir: PathBuf,
}

impl Brand {
    /// Build a loader using the standard resolution rules:
    /// `$MDE_BRAND_DIR` then `/usr/share/mde/brand/` then baked
    /// fallback.
    #[must_use]
    pub fn new() -> Self {
        Self {
            override_dir: env::var_os(BRAND_DIR_ENV).map(PathBuf::from),
            system_dir: PathBuf::from(SYSTEM_BRAND_DIR),
        }
    }

    /// Build a loader pointing at an explicit override directory.
    /// Use in tests, demos, and dev workflows where the env var
    /// would leak between processes.
    #[must_use]
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            override_dir: Some(dir.into()),
            system_dir: PathBuf::from(SYSTEM_BRAND_DIR),
        }
    }

    /// Build a loader that skips both override and system layers
    /// and always returns baked fallbacks. Tests that want a
    /// deterministic, env-independent answer should use this.
    #[must_use]
    pub fn baked_only() -> Self {
        Self {
            override_dir: None,
            system_dir: PathBuf::from("/nonexistent/mde/brand"),
        }
    }

    /// Resolve `slot` to a full [`BrandAsset`] (bytes + format +
    /// source). This is the canonical loader entry point.
    #[must_use]
    pub fn resolve(&self, slot: BrandSlot) -> BrandAsset {
        if let Some(dir) = &self.override_dir {
            if let Some(hit) = probe(dir, slot) {
                return BrandAsset {
                    bytes: hit.bytes,
                    format: hit.format,
                    source: BrandSource::Override(hit.path),
                };
            }
        }
        if let Some(hit) = probe(&self.system_dir, slot) {
            return BrandAsset {
                bytes: hit.bytes,
                format: hit.format,
                source: BrandSource::System(hit.path),
            };
        }
        BrandAsset {
            bytes: slot.fallback_bytes().to_vec(),
            format: slot.fallback_format(),
            source: BrandSource::Baked,
        }
    }

    /// Backwards-compatible shorthand: resolve a slot and return
    /// only `(bytes, source)`. Prefer [`Self::resolve`] when the
    /// format matters.
    #[must_use]
    pub fn load(&self, slot: BrandSlot) -> (Vec<u8>, BrandSource) {
        let asset = self.resolve(slot);
        (asset.bytes, asset.source)
    }

    /// Resolve `slot` to bytes only, discarding format and source.
    #[must_use]
    pub fn bytes(&self, slot: BrandSlot) -> Vec<u8> {
        self.resolve(slot).bytes
    }
}

impl Default for Brand {
    fn default() -> Self {
        Self::new()
    }
}

struct ProbeHit {
    path: PathBuf,
    bytes: Vec<u8>,
    format: BrandFormat,
}

fn probe(dir: &Path, slot: BrandSlot) -> Option<ProbeHit> {
    for ext in slot.search_exts() {
        let path = dir.join(format!("{}.{ext}", slot.basename()));
        if let Some(bytes) = read_if_exists(&path) {
            return Some(ProbeHit {
                path,
                bytes,
                format: BrandFormat::from_ext(ext)
                    .expect("search_exts must contain only known extensions"),
            });
        }
    }
    None
}

fn read_if_exists(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn baked_fallback_returns_nonempty_svg_for_each_default_slot() {
        let brand = Brand::baked_only();
        for slot in [
            BrandSlot::Wordmark,
            BrandSlot::WordmarkHero,
            BrandSlot::Monogram,
            BrandSlot::AppIcon,
            BrandSlot::GreeterWordmark,
        ] {
            let asset = brand.resolve(slot);
            assert_eq!(asset.source, BrandSource::Baked, "{slot:?}");
            assert_eq!(asset.format, BrandFormat::Svg, "{slot:?}");
            assert!(!asset.bytes.is_empty(), "{slot:?} baked bytes were empty");
            let head = std::str::from_utf8(&asset.bytes[..asset.bytes.len().min(64)])
                .expect("svg should be valid utf-8");
            assert!(head.contains("<svg"), "{slot:?} missing <svg> root");
        }
    }

    #[test]
    fn greeter_hero_and_logo_lockup_have_no_baked_default() {
        let brand = Brand::baked_only();
        for slot in [BrandSlot::GreeterHero, BrandSlot::LogoLockup] {
            let asset = brand.resolve(slot);
            assert_eq!(asset.source, BrandSource::Baked, "{slot:?}");
            assert!(asset.is_empty(), "{slot:?} should have empty fallback");
        }
    }

    #[test]
    fn override_png_wins_over_baked_svg() {
        let tmp = tempdir();
        let custom = b"\x89PNG\r\n\x1a\nfake png bytes";
        fs::write(tmp.path().join("monogram.png"), custom).unwrap();

        let brand = Brand::with_dir(tmp.path());
        let asset = brand.resolve(BrandSlot::Monogram);

        assert_eq!(asset.bytes, custom);
        assert_eq!(asset.format, BrandFormat::Png);
        match asset.source {
            BrandSource::Override(p) => {
                assert_eq!(p, tmp.path().join("monogram.png"));
            }
            other => panic!("expected Override, got {other:?}"),
        }
    }

    #[test]
    fn override_svg_wins_over_override_png_in_same_dir() {
        // When the user supplies both, SVG should be preferred —
        // it's the more capable format.
        let tmp = tempdir();
        fs::write(tmp.path().join("monogram.svg"), b"<svg>winner</svg>").unwrap();
        fs::write(tmp.path().join("monogram.png"), b"\x89PNGloser").unwrap();

        let brand = Brand::with_dir(tmp.path());
        let asset = brand.resolve(BrandSlot::Monogram);

        assert_eq!(asset.format, BrandFormat::Svg);
        assert_eq!(asset.bytes, b"<svg>winner</svg>");
    }

    #[test]
    fn greeter_hero_only_probes_png() {
        // GreeterHero declares only png; an svg in the override
        // dir must be ignored.
        let tmp = tempdir();
        fs::write(
            tmp.path().join("greeter-hero.svg"),
            b"<svg>should be ignored</svg>",
        )
        .unwrap();
        let brand = Brand::with_dir(tmp.path());
        let asset = brand.resolve(BrandSlot::GreeterHero);
        assert!(asset.is_empty(), "greeter-hero.svg must not be picked up");
        assert_eq!(asset.source, BrandSource::Baked);
    }

    #[test]
    fn missing_override_falls_through_to_baked() {
        // Isolate BOTH dirs to empty temps so the result is the baked fallback
        // regardless of whether this host has the RPM-installed
        // /usr/share/mde/brand (which would otherwise intercept as `System`).
        let over = tempdir();
        let sys = tempdir();
        let brand = Brand {
            override_dir: Some(over.path().to_path_buf()),
            system_dir: sys.path().to_path_buf(),
        };
        let asset = brand.resolve(BrandSlot::Monogram);
        assert!(!asset.bytes.is_empty());
        assert_eq!(asset.source, BrandSource::Baked);
        assert_eq!(asset.format, BrandFormat::Svg);
    }

    #[test]
    fn basenames_match_filenames() {
        for slot in [
            BrandSlot::Wordmark,
            BrandSlot::WordmarkHero,
            BrandSlot::Monogram,
            BrandSlot::AppIcon,
            BrandSlot::GreeterHero,
            BrandSlot::GreeterWordmark,
            BrandSlot::LogoLockup,
        ] {
            // The canonical filename should equal basename + the
            // first search extension. Catches drift between the
            // filename docs and the actual probe order.
            let first_ext = slot
                .search_exts()
                .first()
                .copied()
                .expect("every slot must have at least one search extension");
            // Wordmark/etc. declare svg first but filename() now
            // reports png because that's what we ship in
            // assets/brand/. The contract here: filename() is the
            // shipped artifact; basename + first ext is the
            // preferred form. Both must reference the same slot.
            let shipped = slot.filename();
            assert!(
                shipped.starts_with(slot.basename()),
                "{slot:?} filename {shipped} does not match basename {}",
                slot.basename()
            );
            assert!(
                shipped.ends_with(&format!(".{first_ext}")) || shipped.ends_with(".png"),
                "{slot:?} filename {shipped} extension not in search_exts"
            );
        }
    }

    #[test]
    fn brand_is_fixed_palette_not_tintable() {
        // BRAND-11: the MCNF 11 logo is a fixed-palette mark — NO slot is
        // tintable, and every baked fallback embeds the full-color logo (not a
        // monochrome `currentColor` glyph).
        for slot in [
            BrandSlot::Wordmark,
            BrandSlot::WordmarkHero,
            BrandSlot::Monogram,
            BrandSlot::AppIcon,
            BrandSlot::GreeterWordmark,
            BrandSlot::GreeterHero,
            BrandSlot::LogoLockup,
        ] {
            assert!(!slot.is_tintable(), "{slot:?} must be fixed-palette");
        }
        // The baked logo slots embed the raster mark (base64 PNG <image>), so
        // they must NOT carry currentColor any more.
        for slot in [
            BrandSlot::Wordmark,
            BrandSlot::WordmarkHero,
            BrandSlot::Monogram,
            BrandSlot::AppIcon,
        ] {
            let text = std::str::from_utf8(slot.fallback_bytes()).unwrap();
            assert!(
                !text.contains("currentColor"),
                "{slot:?} is fixed-palette but baked bytes still use currentColor"
            );
            assert!(
                text.contains("image"),
                "{slot:?} baked fallback should embed the logo image"
            );
        }
    }

    #[test]
    fn brand_format_ext_round_trips() {
        for fmt in [BrandFormat::Svg, BrandFormat::Png] {
            assert_eq!(BrandFormat::from_ext(fmt.ext()), Some(fmt));
        }
        assert_eq!(BrandFormat::from_ext("jpg"), None);
    }

    // Minimal temp-dir helper — avoids pulling in the `tempfile`
    // crate for a few tests. Creates a unique directory under
    // `std::env::temp_dir()` and cleans it up on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("mde-brand-test-{pid}-{n}"));
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
