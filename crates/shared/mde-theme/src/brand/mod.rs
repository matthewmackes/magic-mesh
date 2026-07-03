//! `brand` — the Quazar branding submodule (QBRAND).
//!
//! The single source of truth for the platform's identity: the compile-time
//! build stamp today ([`build`]), the monochrome Carbon icon set and the
//! product-mark/wordmark logo lockup to follow (QBRAND-2/3). Every surface, the
//! boot-splash, the About panel, the RPM and `--version` read their brand data
//! from here so the mark, the version line and the build info never diverge.

pub mod build;
