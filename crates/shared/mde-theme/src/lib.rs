//! `mde-theme` — the MCNF **Construct** brand & identity crate.
//!
//! QBRAND locked decision #4: the branding mechanism lives in ONE crate — the
//! single source of truth every egui surface, the RPM and `--version` read from.
//! The E12 pivot deleted the Cosmic-era Carbon-token `mde-theme` (the look now
//! lives in [`mde_egui::Style`]); this crate revives the name for the *brand*
//! layer that sits alongside that look.
//!
//! Today it carries [`brand::build`] — the compile-time build identity stamped
//! by `build.rs` — and [`brand::icons`] — the Construct product marks plus
//! YAMIS-backed platform glyphs with their tintable SVG→RGBA rasterizer
//! (QBRAND-2). The logo lockup ([`brand`] `::logo`, QBRAND-3) extends the same
//! submodule so the mark, the version line and the build stamp all resolve
//! through `mde_theme::brand`.

pub mod brand;
