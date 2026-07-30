//! Typed This Node section catalog.
//!
//! This is the durable navigation authority for the node-local hardware center.
//! Providers are intentionally separate: a section can be discoverable before a
//! node has the provider needed to render it, but it must then remain visibly
//! unavailable rather than pretending that a control is live.

/// The governed This Node hierarchy (WL-UX-011).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Section {
    #[default]
    Overview,
    Connectivity,
    DisplaySound,
    Input,
    PowerPerformance,
    Hardware,
    Personalization,
    MeshSystem,
}

impl Section {
    pub(crate) const ALL: [Self; 8] = [
        Self::Overview,
        Self::Connectivity,
        Self::DisplaySound,
        Self::Input,
        Self::PowerPerformance,
        Self::Hardware,
        Self::Personalization,
        Self::MeshSystem,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Connectivity => "Connectivity",
            Self::DisplaySound => "Display & Sound",
            Self::Input => "Input",
            Self::PowerPerformance => "Power & Performance",
            Self::Hardware => "Hardware",
            Self::Personalization => "Personalization",
            Self::MeshSystem => "Mesh & System",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Overview => "Node identity, status, and available controls.",
            Self::Connectivity => "Wi-Fi, Ethernet, cellular, VPN, DNS, and proxy.",
            Self::DisplaySound => "Displays, brightness, audio routes, and privacy.",
            Self::Input => "Keyboard, pointer, touch, pen, and seat policy.",
            Self::PowerPerformance => "Battery, thermals, performance, and sleep policy.",
            Self::Hardware => "Devices, firmware, storage, docks, and capabilities.",
            Self::Personalization => "Appearance, layout, wallpaper, and local preferences.",
            Self::MeshSystem => "Nebula, services, updates, and mesh diagnostics.",
        }
    }

    /// Operator vocabulary used by the hardware center search.
    ///
    /// Keep this list descriptive: these are navigation aliases, not claims
    /// that a provider-backed control is available on this node.
    const KEYWORDS: [&'static [&'static str]; 8] = [
        &["node", "this node", "status", "identity", "overview"],
        &[
            "wi-fi",
            "wifi",
            "wireless",
            "bluetooth",
            "bt",
            "hotspot",
            "hot spot",
            "tethering",
            "network",
            "networking",
            "ethernet",
            "cellular",
            "vpn",
            "dns",
            "proxy",
        ],
        &[
            "audio",
            "sound",
            "speaker",
            "microphone",
            "mic",
            "display",
            "monitor",
            "screen",
            "brightness",
            "volume",
        ],
        &[
            "keyboard",
            "pointer",
            "mouse",
            "touch",
            "touchscreen",
            "touch screen",
            "pen",
            "stylus",
            "trackpad",
            "touchpad",
        ],
        &[
            "battery",
            "charging",
            "charger",
            "thermal",
            "thermals",
            "temperature",
            "gpu",
            "performance",
            "sleep",
            "power",
        ],
        &[
            "device",
            "devices",
            "firmware",
            "storage",
            "disk",
            "drive",
            "dock",
            "docking",
            "usb",
            "capabilities",
        ],
        &[
            "theme",
            "themes",
            "wallpaper",
            "background",
            "appearance",
            "layout",
            "dark mode",
            "light mode",
        ],
        &[
            "nebula",
            "mesh",
            "service",
            "services",
            "update",
            "updates",
            "diagnostics",
            "system",
        ],
    ];

    /// Search the label and description while treating punctuation and spacing
    /// as presentation details (`wifi` matches `Wi-Fi`).
    pub(crate) fn matches(self, query: &str) -> bool {
        let normalized = search_key(query);
        normalized.is_empty()
            || search_key(self.label()).contains(&normalized)
            || search_key(self.description()).contains(&normalized)
            || Self::KEYWORDS[self as usize]
                .iter()
                .any(|keyword| search_key(keyword).contains(&normalized))
    }

    pub(crate) const fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            Self::Connectivity => {
                Some("NetworkManager/ModemManager controls are not connected to This Node yet.")
            }
            Self::DisplaySound => {
                Some("Display and PipeWire mutation providers are not connected to This Node yet.")
            }
            Self::Input => {
                Some("Direct-seat input policy controls are not connected to This Node yet.")
            }
            Self::Personalization => {
                Some("This Node personalization persistence is not connected to this center yet.")
            }
            _ => None,
        }
    }
}

fn search_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

pub(crate) fn search(query: &str) -> Vec<Section> {
    Section::ALL
        .into_iter()
        .filter(|section| section.matches(query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{search, Section};

    #[test]
    fn catalog_has_the_governed_eight_sections() {
        assert_eq!(Section::ALL.len(), 8);
        assert_eq!(Section::ALL[0], Section::Overview);
        assert_eq!(Section::ALL[7], Section::MeshSystem);
    }

    #[test]
    fn search_is_case_insensitive_and_keeps_unavailable_sections_discoverable() {
        assert_eq!(search("SOUND"), vec![Section::DisplaySound]);
        assert_eq!(search("wifi"), vec![Section::Connectivity]);
        assert_eq!(search(""), Section::ALL.to_vec());
        assert!(Section::Connectivity.unavailable_reason().is_some());
    }

    #[test]
    fn search_covers_hardware_center_operator_aliases() {
        let aliases = [
            ("bluetooth", Section::Connectivity),
            ("hotspot", Section::Connectivity),
            ("brightness", Section::DisplaySound),
            ("audio", Section::DisplaySound),
            ("touchscreen", Section::Input),
            ("stylus", Section::Input),
            ("thermal", Section::PowerPerformance),
            ("gpu", Section::PowerPerformance),
            ("firmware", Section::Hardware),
            ("dock", Section::Hardware),
            ("wallpaper", Section::Personalization),
            ("theme", Section::Personalization),
            ("nebula", Section::MeshSystem),
            ("updates", Section::MeshSystem),
        ];

        for (query, section) in aliases {
            assert_eq!(search(query), vec![section], "query: {query}");
        }
    }

    #[test]
    fn search_normalizes_alias_punctuation_and_spacing() {
        assert_eq!(search("Wi-Fi"), vec![Section::Connectivity]);
        assert_eq!(search("wi fi"), vec![Section::Connectivity]);
        assert_eq!(search("Touch-Screen"), vec![Section::Input]);
        assert_eq!(search("dark-mode"), vec![Section::Personalization]);
    }
}
