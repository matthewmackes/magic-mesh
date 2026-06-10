//! PLANES-2 (H1/H2/H5/H6/H7) — monochrome **hero** line-art for the
//! primary-service Workbench panels.
//!
//! Each [`Hero`] is an *original* monochrome line-art glyph (NOT the
//! upstream project logo — H2/H5) baked in at compile time, drawn with
//! `stroke="currentColor"` so the renderer tints it with the single
//! [`HERO_STROKE`] Carbon token (§4). A panel for a primary service
//! (Ansible / LizardFS / Nebula / Fedora / …) shows its hero in the
//! header band at 96–128 px (H3/H4) with the service NAME + live version
//! caption (H8); the art always renders, even when the service isn't
//! installed (H10 — the panel says so in text, the hero stays).

use crate::carbon;
use crate::color::Rgba;

/// The single-sourced stroke colour for all hero line-art (§4): Carbon
/// Gray 50 — a calm mid-stroke that reads cleanly on the Gray-100 dark
/// surface without competing with content. Change it only here.
pub const HERO_STROKE: Rgba = carbon::GRAY_50;

/// A primary-service hero glyph. Order is stable (drives any indexed
/// rendering); add new services at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hero {
    Ansible,
    LizardFs,
    Nebula,
    Fedora,
    Netdata,
    Podman,
    Libvirt,
    Cosmic,
    Systemd,
    Remmina,
    PipeWire,
    Rustls,
    Vpn,
}

impl Hero {
    /// Every hero, in stable order.
    #[must_use]
    pub const fn all() -> [Self; 13] {
        [
            Self::Ansible,
            Self::LizardFs,
            Self::Nebula,
            Self::Fedora,
            Self::Netdata,
            Self::Podman,
            Self::Libvirt,
            Self::Cosmic,
            Self::Systemd,
            Self::Remmina,
            Self::PipeWire,
            Self::Rustls,
            Self::Vpn,
        ]
    }

    /// Operator-facing service name shown in the hero caption (H8).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ansible => "Ansible",
            Self::LizardFs => "LizardFS",
            Self::Nebula => "Nebula",
            Self::Fedora => "Fedora",
            Self::Netdata => "Netdata",
            Self::Podman => "Podman",
            Self::Libvirt => "libvirt",
            Self::Cosmic => "COSMIC",
            Self::Systemd => "systemd",
            Self::Remmina => "Remmina",
            Self::PipeWire => "PipeWire",
            Self::Rustls => "rustls",
            Self::Vpn => "VPN",
        }
    }

    /// The baked monochrome line-art SVG (`stroke="currentColor"`).
    #[must_use]
    pub const fn svg_bytes(self) -> &'static [u8] {
        match self {
            Self::Ansible => include_bytes!("../../../../assets/heroes/ansible.svg"),
            Self::LizardFs => include_bytes!("../../../../assets/heroes/lizardfs.svg"),
            Self::Nebula => include_bytes!("../../../../assets/heroes/nebula.svg"),
            Self::Fedora => include_bytes!("../../../../assets/heroes/fedora.svg"),
            Self::Netdata => include_bytes!("../../../../assets/heroes/netdata.svg"),
            Self::Podman => include_bytes!("../../../../assets/heroes/podman.svg"),
            Self::Libvirt => include_bytes!("../../../../assets/heroes/libvirt.svg"),
            Self::Cosmic => include_bytes!("../../../../assets/heroes/cosmic.svg"),
            Self::Systemd => include_bytes!("../../../../assets/heroes/systemd.svg"),
            Self::Remmina => include_bytes!("../../../../assets/heroes/remmina.svg"),
            Self::PipeWire => include_bytes!("../../../../assets/heroes/pipewire.svg"),
            Self::Rustls => include_bytes!("../../../../assets/heroes/rustls.svg"),
            Self::Vpn => include_bytes!("../../../../assets/heroes/vpn.svg"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_stroke_is_carbon_gray_50() {
        // §4 — the hero stroke is single-sourced to a Carbon ramp value,
        // not an ad-hoc literal. Pin it so a drift is a test failure.
        assert_eq!(HERO_STROKE, carbon::GRAY_50);
    }

    #[test]
    fn every_hero_bakes_a_nonempty_currentcolor_svg() {
        for h in Hero::all() {
            let svg = std::str::from_utf8(h.svg_bytes()).expect("utf-8 svg");
            assert!(svg.starts_with("<svg"), "{h:?} not an svg");
            assert!(svg.contains("</svg>"), "{h:?} svg truncated");
            // currentColor so HERO_STROKE tints it (no baked-in colour).
            assert!(
                svg.contains("currentColor"),
                "{h:?} must stroke currentColor"
            );
            // line-art, not a filled logo (H2/H5).
            assert!(
                svg.contains("fill=\"none\""),
                "{h:?} must be line-art (fill none)"
            );
        }
    }

    #[test]
    fn all_thirteen_services_present_with_names() {
        let names: Vec<&str> = Hero::all().iter().map(|h| h.name()).collect();
        assert_eq!(names.len(), 13);
        for expected in [
            "Ansible", "LizardFS", "Nebula", "Fedora", "COSMIC", "rustls", "VPN",
        ] {
            assert!(names.contains(&expected), "missing hero name {expected}");
        }
        // names are distinct.
        let uniq: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(uniq.len(), names.len(), "duplicate hero name");
    }
}
