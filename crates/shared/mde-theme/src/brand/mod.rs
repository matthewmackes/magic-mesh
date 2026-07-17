//! `brand` — the Quazar branding submodule (QBRAND).
//!
//! The single source of truth for the platform's identity: the compile-time
//! build stamp ([`build`]), the monochrome Quazar icon set with its SVG→raster
//! loader ([`icons`], QBRAND-2), and the product-mark/wordmark logo lockup
//! ([`logo`], QBRAND-3). Every surface, the boot-splash, the About panel, the
//! RPM and `--version` read their brand data from here so the mark, the version
//! line and the build info never diverge.

pub mod build;
pub mod icons;
pub mod logo;
