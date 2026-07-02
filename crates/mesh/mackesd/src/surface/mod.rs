//! SURFACE-2 — DMI detection + per-model Surface profile.
//!
//! The hardware-truth entry point for the Microsoft Surface enablement
//! epic (design: `docs/design/surface-tablet-enablement.md`, lock #3).
//! A node reads its DMI identity and folds it to a [`SurfaceModel`]:
//! a recognised Surface (Pro/Book/Laptop/Go/Studio + generation), an
//! honest [`SurfaceModel::UnknownSurface`] for a Microsoft product not
//! yet in the built-in table, or [`SurfaceModel::NotASurface`] for
//! everything else. A recognised model carries a [`SurfaceProfile`] —
//! the subsystem checklist that model *has* — so the day-2 verify
//! board (SURFACE-4) knows what to expect and the This-Node card
//! (SURFACE-6) renders the right list. Non-Surface nodes never see the
//! card.
//!
//! **The DMI read sits behind an injectable seam** ([`DmiSource`]) so
//! [`identify`] is a pure fold over a [`DmiInfo`] fixture — tests never
//! touch real `/sys`. The production seam ([`SysfsDmi`]) reads
//! `/sys/class/dmi/id/*` directly (§9 — no raw shell).
//!
//! This unit *detects*; it publishes nothing. SURFACE-3 (`surface_enable`)
//! and SURFACE-4 (verify + fleet publish) consume [`detect`]'s typed
//! result.

use std::path::Path;

/// SURFACE-3 — the day-2 activation half: the `surface_enable` verb/worker
/// (iptsd activate + per-model config) and the guided MOK enrollment state
/// machine, built on this module's [`SurfaceDevice`]/[`SurfaceProfile`].
pub mod enable;

/// The four DMI fields the Surface identity fold reads, already
/// trimmed. A field the firmware/kernel doesn't expose is the empty
/// string (never an error) — best-effort, like every other mackesd
/// probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DmiInfo {
    /// `/sys/class/dmi/id/sys_vendor` — `"Microsoft Corporation"` on a
    /// genuine Surface.
    pub sys_vendor: String,
    /// `/sys/class/dmi/id/product_name` — e.g. `"Surface Pro 7"`.
    pub product_name: String,
    /// `/sys/class/dmi/id/product_version` — sometimes carries the
    /// generation when `product_name` is bare (`"Surface_Pro_7"`).
    pub product_version: String,
    /// `/sys/class/dmi/id/chassis_type` — SMBIOS chassis code (30
    /// Tablet, 31 Convertible, 32 Detachable, 9/10 Laptop/Notebook,
    /// 13 All-in-One). A hint only; the vendor+product match is
    /// authoritative.
    pub chassis_type: String,
}

/// The injectable DMI seam. Production reads `/sys`; tests hand a
/// fixture [`DmiInfo`] straight to [`identify`].
pub trait DmiSource {
    /// Read the four DMI fields for this host. Best-effort: missing
    /// fields come back empty, never erroring.
    fn read(&self) -> DmiInfo;
}

/// The production seam — reads the SMBIOS/DMI fields the kernel exposes
/// under `/sys/class/dmi/id`. §9-clean: a direct filesystem read, no
/// `dmidecode` subprocess.
#[derive(Debug, Clone, Copy, Default)]
pub struct SysfsDmi;

impl SysfsDmi {
    const DMI_DIR: &'static str = "/sys/class/dmi/id";

    fn field(name: &str) -> String {
        let path = Path::new(Self::DMI_DIR).join(name);
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
}

impl DmiSource for SysfsDmi {
    fn read(&self) -> DmiInfo {
        DmiInfo {
            sys_vendor: Self::field("sys_vendor"),
            product_name: Self::field("product_name"),
            product_version: Self::field("product_version"),
            chassis_type: Self::field("chassis_type"),
        }
    }
}

/// The DMI `sys_vendor` string a genuine Microsoft Surface reports.
pub const MS_VENDOR: &str = "Microsoft Corporation";

/// One line-item subsystem the linux-surface matrix enables (design
/// lock #2). A [`SurfaceProfile`] declares which of these a given model
/// *has*; the verify board (SURFACE-4) probes exactly that set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Subsystem {
    /// Capacitive touchscreen (iptsd).
    Touch,
    /// Active pen / stylus pressure + tilt (iptsd).
    Pen,
    /// Detachable Type Cover keyboard + trackpad.
    TypeCover,
    /// Surface Aggregator Module — battery / thermal / perf profile.
    Sam,
    /// Accelerometer-driven auto-rotation.
    RotationAccel,
    /// Front / rear cameras.
    Cameras,
    /// Wi-Fi + Bluetooth (Surface quirks).
    WifiBt,
    /// S0ix modern-standby suspend residency.
    S0ix,
    /// Fingerprint reader (Windows-Hello-class), where fitted.
    Fingerprint,
}

impl Subsystem {
    /// Stable identifier for state keys / logs.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Touch => "touch",
            Self::Pen => "pen",
            Self::TypeCover => "type_cover",
            Self::Sam => "sam",
            Self::RotationAccel => "rotation_accel",
            Self::Cameras => "cameras",
            Self::WifiBt => "wifi_bt",
            Self::S0ix => "s0ix",
            Self::Fingerprint => "fingerprint",
        }
    }

    /// Human label for the card checklist.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Touch => "Touchscreen",
            Self::Pen => "Pen / stylus",
            Self::TypeCover => "Type Cover",
            Self::Sam => "Surface Aggregator (battery/thermal)",
            Self::RotationAccel => "Auto-rotation (accelerometer)",
            Self::Cameras => "Cameras",
            Self::WifiBt => "Wi-Fi / Bluetooth",
            Self::S0ix => "S0ix suspend",
            Self::Fingerprint => "Fingerprint reader",
        }
    }
}

/// The subsystem checklist a recognised model carries.
///
/// Each field says whether the model *has* that line item, so the verify
/// board should expect it green. A `false` means the hardware is absent
/// (a Surface Laptop has no detachable Type Cover; a Go lacks a
/// fingerprint reader) — verify neither probes nor faults it.
// A subsystem checklist is inherently a set of presence flags; the bools
// are the honest shape (one per line item), not conflatable state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceProfile {
    /// Capacitive touchscreen.
    pub touch: bool,
    /// Active pen.
    pub pen: bool,
    /// Detachable Type Cover.
    pub type_cover: bool,
    /// Surface Aggregator Module.
    pub sam: bool,
    /// Accelerometer auto-rotation.
    pub rotation_accel: bool,
    /// Cameras.
    pub cameras: bool,
    /// Wi-Fi + Bluetooth.
    pub wifi_bt: bool,
    /// S0ix suspend.
    pub s0ix: bool,
    /// Fingerprint reader.
    pub fingerprint: bool,
}

impl SurfaceProfile {
    /// The subsystems this profile claims the model *has*, in board
    /// order — the checklist SURFACE-4 verifies and SURFACE-6 renders.
    #[must_use]
    pub fn expected(&self) -> Vec<Subsystem> {
        [
            (self.touch, Subsystem::Touch),
            (self.pen, Subsystem::Pen),
            (self.type_cover, Subsystem::TypeCover),
            (self.sam, Subsystem::Sam),
            (self.rotation_accel, Subsystem::RotationAccel),
            (self.cameras, Subsystem::Cameras),
            (self.wifi_bt, Subsystem::WifiBt),
            (self.s0ix, Subsystem::S0ix),
            (self.fingerprint, Subsystem::Fingerprint),
        ]
        .into_iter()
        .filter_map(|(has, s)| has.then_some(s))
        .collect()
    }

    /// Does this model have `subsystem`?
    #[must_use]
    pub const fn has(&self, subsystem: Subsystem) -> bool {
        match subsystem {
            Subsystem::Touch => self.touch,
            Subsystem::Pen => self.pen,
            Subsystem::TypeCover => self.type_cover,
            Subsystem::Sam => self.sam,
            Subsystem::RotationAccel => self.rotation_accel,
            Subsystem::Cameras => self.cameras,
            Subsystem::WifiBt => self.wifi_bt,
            Subsystem::S0ix => self.s0ix,
            Subsystem::Fingerprint => self.fingerprint,
        }
    }
}

/// The Surface product families the built-in table recognises (design
/// lock #3: Pro/Book/Laptop/Go — Studio folded in for the desktop `AiO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceFamily {
    /// Surface Pro — detachable 2-in-1 tablet.
    Pro,
    /// Surface Book — detachable-clipboard clamshell.
    Book,
    /// Surface Laptop — traditional clamshell (built-in keyboard).
    Laptop,
    /// Surface Go — smaller detachable 2-in-1.
    Go,
    /// Surface Studio — desktop all-in-one with pen display.
    Studio,
}

impl SurfaceFamily {
    /// Display name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pro => "Surface Pro",
            Self::Book => "Surface Book",
            Self::Laptop => "Surface Laptop",
            Self::Go => "Surface Go",
            Self::Studio => "Surface Studio",
        }
    }

    /// The DMI-`product_name` prefix that identifies this family. `"Surface
    /// Studio"` (the desktop `AiO`) is a disjoint prefix from `"Surface
    /// Laptop"`; the newer clamshell-convertible `"Surface Laptop Studio"`
    /// falls under [`SurfaceFamily::Laptop`] (built-in keyboard, no
    /// accelerometer rotation) — a defensible bucket its verify board can
    /// refine.
    #[must_use]
    pub const fn dmi_prefix(self) -> &'static str {
        match self {
            Self::Pro => "Surface Pro",
            Self::Book => "Surface Book",
            Self::Laptop => "Surface Laptop",
            Self::Go => "Surface Go",
            Self::Studio => "Surface Studio",
        }
    }

    /// This family's built-in subsystem profile. Model-aware per the
    /// physical hardware: clamshells have no detachable Type Cover and
    /// no auto-rotation; the desktop Studio has neither and no S0ix; a
    /// Go ships without a fingerprint reader.
    #[must_use]
    pub const fn profile(self) -> SurfaceProfile {
        match self {
            // The 2-in-1s (Pro tablet, Book detachable-clipboard, Go) share
            // the full linux-surface matrix: touch/pen/detachable Type
            // Cover/SAM/auto-rotation/cameras/Wi-Fi-BT/S0ix. Hello is
            // IR-face, not a fingerprint reader, so fingerprint stays false.
            Self::Pro | Self::Book | Self::Go => SurfaceProfile {
                touch: true,
                pen: true,
                type_cover: true,
                sam: true,
                rotation_accel: true,
                cameras: true,
                wifi_bt: true,
                s0ix: true,
                fingerprint: false,
            },
            // Traditional clamshell — built-in keyboard (no detachable
            // Type Cover), no auto-rotation; power-button fingerprint on
            // several SKUs.
            Self::Laptop => SurfaceProfile {
                touch: true,
                pen: true,
                type_cover: false,
                sam: true,
                rotation_accel: false,
                cameras: true,
                wifi_bt: true,
                s0ix: true,
                fingerprint: true,
            },
            // Desktop all-in-one — pen display, no Type Cover, no
            // rotation, mains-powered (no S0ix), no fingerprint.
            Self::Studio => SurfaceProfile {
                touch: true,
                pen: true,
                type_cover: false,
                sam: true,
                rotation_accel: false,
                cameras: true,
                wifi_bt: true,
                s0ix: false,
                fingerprint: false,
            },
        }
    }
}

/// Family-prefix match order. `"Surface Studio"` (desktop `AiO`) sits ahead
/// of the others so it can't be shadowed; the prefixes are otherwise
/// disjoint. A product like `"Surface Laptop Studio"` matches the
/// `"Surface Laptop"` prefix (its true form factor).
const FAMILY_MATCH: [SurfaceFamily; 5] = [
    SurfaceFamily::Studio,
    SurfaceFamily::Book,
    SurfaceFamily::Go,
    SurfaceFamily::Pro,
    SurfaceFamily::Laptop,
];

/// A recognised Surface device — its family, best-effort generation,
/// the exact DMI product string, and the subsystem profile downstream
/// verify/card consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDevice {
    /// Which Surface product family.
    pub family: SurfaceFamily,
    /// Generation number parsed from the product string (`7` for
    /// `"Surface Pro 7"`), if present.
    pub generation: Option<u32>,
    /// The raw DMI product string it matched (`"Surface Pro 7"`).
    pub product: String,
    /// The subsystem checklist this model carries.
    pub profile: SurfaceProfile,
}

/// The identity fold's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceModel {
    /// Not a Microsoft Surface (vendor mismatch) — the card never
    /// appears.
    NotASurface,
    /// A genuine Microsoft product whose model isn't in the built-in
    /// table yet — honest (§7), not a panic. Carries the product string
    /// so the card can say "unrecognised Surface: <product>" and an
    /// operator can file it.
    UnknownSurface {
        /// The DMI `product_name` (falling back to `product_version`).
        product: String,
    },
    /// A recognised Surface with its per-model profile.
    Known(SurfaceDevice),
}

impl SurfaceModel {
    /// `true` for a recognised Surface — the card-visibility gate
    /// (design lock #3/#7 — non-Surface nodes never see the card).
    #[must_use]
    pub const fn is_surface(&self) -> bool {
        matches!(self, Self::Known(_))
    }

    /// The subsystem profile for a recognised model.
    #[must_use]
    pub const fn profile(&self) -> Option<&SurfaceProfile> {
        match self {
            Self::Known(dev) => Some(&dev.profile),
            _ => None,
        }
    }
}

/// The typed detection result the downstream units consume: the model
/// verdict plus the raw DMI it was folded from (for the card's detail
/// line + logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDetection {
    /// The identity verdict.
    pub model: SurfaceModel,
    /// The DMI fields the verdict was folded from.
    pub dmi: DmiInfo,
}

/// The pure identity fold: DMI → [`SurfaceModel`]. No I/O — tests drive
/// it with [`DmiInfo`] fixtures.
///
/// * Vendor ≠ `"Microsoft Corporation"` ⇒ [`SurfaceModel::NotASurface`].
/// * A Microsoft product matching the built-in family table ⇒
///   [`SurfaceModel::Known`] with that family's profile.
/// * A Microsoft product not in the table ⇒
///   [`SurfaceModel::UnknownSurface`] (honest).
#[must_use]
pub fn identify(dmi: &DmiInfo) -> SurfaceModel {
    if dmi.sys_vendor.trim() != MS_VENDOR {
        return SurfaceModel::NotASurface;
    }

    // Prefer product_name; some firmware leaves it bare and carries the
    // real string (underscored) in product_version.
    let product = pick_product(dmi);
    let normalised = product.replace('_', " ");

    for family in FAMILY_MATCH {
        if let Some(rest) = normalised.strip_prefix(family.dmi_prefix()) {
            // Guard against a false prefix hit ("Surface Prodigy"): the
            // char after the prefix must be a boundary (end or space).
            if !rest.is_empty() && !rest.starts_with(' ') {
                continue;
            }
            return SurfaceModel::Known(SurfaceDevice {
                family,
                generation: parse_generation(rest),
                product: normalised,
                profile: family.profile(),
            });
        }
    }

    SurfaceModel::UnknownSurface {
        product: normalised,
    }
}

/// Choose the product string to match on — `product_name` unless it's
/// blank, then `product_version`.
fn pick_product(dmi: &DmiInfo) -> String {
    let name = dmi.product_name.trim();
    if name.is_empty() {
        dmi.product_version.trim().to_string()
    } else {
        name.to_string()
    }
}

/// Pull the leading integer out of the post-family remainder
/// (`" 7"` → `Some(7)`, `" X"`/`""` → `None`).
fn parse_generation(rest: &str) -> Option<u32> {
    rest.split_whitespace()
        .find_map(|tok| tok.parse::<u32>().ok())
}

/// Detect this host's Surface identity via `src`. Pairs the injectable
/// DMI read with the pure [`identify`] fold.
#[must_use]
pub fn detect_with(src: &impl DmiSource) -> SurfaceDetection {
    let dmi = src.read();
    let model = identify(&dmi);
    SurfaceDetection { model, dmi }
}

/// Detect this host's Surface identity from real `/sys/class/dmi/id`.
/// The production entry point SURFACE-3/4 call.
#[must_use]
pub fn detect() -> SurfaceDetection {
    detect_with(&SysfsDmi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dmi(vendor: &str, product: &str) -> DmiInfo {
        DmiInfo {
            sys_vendor: vendor.to_string(),
            product_name: product.to_string(),
            product_version: String::new(),
            chassis_type: String::new(),
        }
    }

    #[test]
    fn surface_pro_maps_to_pro_profile_with_generation() {
        let m = identify(&dmi(MS_VENDOR, "Surface Pro 7"));
        let SurfaceModel::Known(dev) = m else {
            panic!("expected Known, got {m:?}");
        };
        assert_eq!(dev.family, SurfaceFamily::Pro);
        assert_eq!(dev.generation, Some(7));
        assert_eq!(dev.product, "Surface Pro 7");
        // Pro carries the full matrix, IR-face (no fingerprint reader).
        assert!(dev.profile.touch && dev.profile.pen && dev.profile.type_cover);
        assert!(dev.profile.rotation_accel && dev.profile.s0ix);
        assert!(!dev.profile.fingerprint);
        assert_eq!(
            dev.profile.expected(),
            vec![
                Subsystem::Touch,
                Subsystem::Pen,
                Subsystem::TypeCover,
                Subsystem::Sam,
                Subsystem::RotationAccel,
                Subsystem::Cameras,
                Subsystem::WifiBt,
                Subsystem::S0ix,
            ]
        );
    }

    #[test]
    fn surface_pro_x_has_no_numeric_generation() {
        let m = identify(&dmi(MS_VENDOR, "Surface Pro X"));
        let SurfaceModel::Known(dev) = m else {
            panic!("expected Known");
        };
        assert_eq!(dev.family, SurfaceFamily::Pro);
        assert_eq!(dev.generation, None);
    }

    #[test]
    fn surface_laptop_has_no_type_cover_or_rotation_but_fingerprint() {
        let m = identify(&dmi(MS_VENDOR, "Surface Laptop 3"));
        let p = m.profile().expect("laptop is a known Surface");
        assert!(!p.type_cover, "clamshell has a built-in keyboard");
        assert!(!p.rotation_accel, "clamshell doesn't auto-rotate");
        assert!(p.fingerprint, "laptop has a power-button fingerprint");
        assert!(!p.expected().contains(&Subsystem::TypeCover));
    }

    #[test]
    fn surface_go_lacks_fingerprint() {
        let m = identify(&dmi(MS_VENDOR, "Surface Go 2"));
        let SurfaceModel::Known(dev) = m else {
            panic!("expected Known");
        };
        assert_eq!(dev.family, SurfaceFamily::Go);
        assert_eq!(dev.generation, Some(2));
        assert!(!dev.profile.fingerprint);
        assert!(dev.profile.type_cover);
    }

    #[test]
    fn surface_book_is_detachable_full_matrix() {
        let p = identify(&dmi(MS_VENDOR, "Surface Book 3"))
            .profile()
            .copied()
            .expect("book is known");
        assert!(p.type_cover && p.rotation_accel);
    }

    #[test]
    fn surface_studio_is_desktop_no_rotation_no_s0ix() {
        let m = identify(&dmi(MS_VENDOR, "Surface Studio 2"));
        let p = m.profile().expect("studio is known");
        assert!(!p.type_cover);
        assert!(!p.rotation_accel);
        assert!(!p.s0ix, "mains-powered desktop, no modern standby");
        assert!(p.touch && p.pen);
    }

    #[test]
    fn laptop_studio_is_a_laptop_not_the_desktop_studio() {
        // The clamshell-convertible Laptop Studio is a laptop form factor,
        // distinct from the desktop "Surface Studio" AiO.
        let m = identify(&dmi(MS_VENDOR, "Surface Laptop Studio"));
        let SurfaceModel::Known(dev) = m else {
            panic!("expected Known");
        };
        assert_eq!(dev.family, SurfaceFamily::Laptop);
        assert_eq!(dev.generation, None);
    }

    #[test]
    fn desktop_studio_still_resolves_to_studio() {
        let m = identify(&dmi(MS_VENDOR, "Surface Studio 2"));
        let SurfaceModel::Known(dev) = m else {
            panic!("expected Known");
        };
        assert_eq!(dev.family, SurfaceFamily::Studio);
    }

    #[test]
    fn non_microsoft_vendor_is_not_a_surface() {
        assert_eq!(
            identify(&dmi("Dell Inc.", "XPS 13 9310")),
            SurfaceModel::NotASurface
        );
        assert!(!identify(&dmi("LENOVO", "ThinkPad X1")).is_surface());
    }

    #[test]
    fn microsoft_non_surface_product_is_unknown_not_panic() {
        let m = identify(&dmi(MS_VENDOR, "Surface Duo"));
        assert_eq!(
            m,
            SurfaceModel::UnknownSurface {
                product: "Surface Duo".to_string()
            }
        );
        // Honest: not-a-panic, still flagged as a Surface-vendor box.
        assert!(!m.is_surface());
    }

    #[test]
    fn false_prefix_surface_prodigy_is_unknown() {
        // "Surface Prodigy" must not be read as a Surface Pro.
        let m = identify(&dmi(MS_VENDOR, "Surface Prodigy"));
        assert!(matches!(m, SurfaceModel::UnknownSurface { .. }));
    }

    #[test]
    fn product_falls_back_to_version_when_name_blank() {
        let d = DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: String::new(),
            product_version: "Surface_Pro_9".to_string(),
            chassis_type: "31".to_string(),
        };
        let SurfaceModel::Known(dev) = identify(&d) else {
            panic!("expected Known via product_version");
        };
        assert_eq!(dev.family, SurfaceFamily::Pro);
        assert_eq!(dev.generation, Some(9));
        assert_eq!(dev.product, "Surface Pro 9");
    }

    #[test]
    fn detect_with_folds_the_seam() {
        struct Fixture;
        impl DmiSource for Fixture {
            fn read(&self) -> DmiInfo {
                dmi(MS_VENDOR, "Surface Pro 8")
            }
        }
        let det = detect_with(&Fixture);
        assert!(det.model.is_surface());
        assert_eq!(det.dmi.product_name, "Surface Pro 8");
    }

    #[test]
    fn profile_has_matches_expected_list() {
        let p = SurfaceFamily::Laptop.profile();
        for s in p.expected() {
            assert!(p.has(s));
        }
        assert!(!p.has(Subsystem::TypeCover));
    }
}
