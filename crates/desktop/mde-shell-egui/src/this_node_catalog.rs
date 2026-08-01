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

    /// Presentation parent in the Device Manager-style tree. These are visual
    /// branches only; the governed surface remains exactly the eight sections
    /// in [`Self::ALL`].
    pub(crate) const fn group(self) -> SectionGroup {
        match self {
            Self::Overview => SectionGroup::Status,
            Self::Connectivity
            | Self::DisplaySound
            | Self::Input
            | Self::PowerPerformance
            | Self::Hardware => SectionGroup::Devices,
            Self::Personalization | Self::MeshSystem => SectionGroup::System,
        }
    }

    /// Operator vocabulary used by the hardware center search.
    ///
    /// Keep this list descriptive: these are navigation aliases, not claims
    /// that a provider-backed control is available on this node.
    const KEYWORDS: [&'static [&'static str]; 8] = [
        &[
            "node",
            "this node",
            "status",
            "identity",
            "overview",
            "health",
            "health dashboard",
            "alerts",
            "critical alerts",
            "all systems operational",
        ],
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
            "interfaces",
            "topology",
            "network topology",
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
            "privacy",
            "camera",
            "microphone",
            "camera privacy",
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
            "accessibility",
            "gestures",
            "hotkeys",
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
            "fan",
            "lid",
            "idle",
        ],
        &[
            "device",
            "devices",
            "device manager",
            "hardware manager",
            "firmware",
            "storage",
            "disk",
            "drive",
            "capacity",
            "dock",
            "docking",
            "thunderbolt",
            "usb",
            "printer",
            "peripherals",
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
            "local preferences",
        ],
        &[
            "nebula",
            "mesh",
            "service",
            "services",
            "applications",
            "users",
            "accounts",
            "update",
            "updates",
            "recovery",
            "reset",
            "backup",
            "restore",
            "backup and restore",
            "diagnostics",
            "logs",
            "virtualization",
            "remote access",
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

/// Fixed presentation branches for the This Node hierarchy.
///
/// A branch is not a new settings route or provider domain. It only gives the
/// eight governed sections a stable tree shape for the health-dashboard landing
/// view, so adding a future provider cannot reorder the navigation implicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionGroup {
    Status,
    Devices,
    System,
}

impl SectionGroup {
    pub(crate) const ALL: [Self; 3] = [Self::Status, Self::Devices, Self::System];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Devices => "Devices & Peripherals",
            Self::System => "System",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Status => "Identity and node health",
            Self::Devices => "Connectivity, hardware, and performance",
            Self::System => "Personalization and mesh services",
        }
    }

    pub(crate) const fn sections(self) -> &'static [Section] {
        match self {
            Self::Status => &[Section::Overview],
            Self::Devices => &[
                Section::Connectivity,
                Section::DisplaySound,
                Section::Input,
                Section::PowerPerformance,
                Section::Hardware,
            ],
            Self::System => &[Section::Personalization, Section::MeshSystem],
        }
    }
}

/// A durable page entry in the This Node search/navigation index.
///
/// Page entries are routes, not provider claims.  A route can therefore be
/// indexed before its provider is connected; callers must surface
/// [`Self::unavailable_reason`] rather than turning an indexed route into a
/// successful control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageEntry {
    pub(crate) route: &'static str,
    pub(crate) section: Section,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    provider: PageProvider,
    keywords: &'static [&'static str],
}

/// Provider truth attached to each indexed page. This is intentionally not a
/// boolean: unavailable pages need a bounded operator-facing explanation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageProvider {
    Available,
    Unavailable(&'static str),
}

impl PageEntry {
    fn matches(self, query: &str) -> bool {
        let normalized = search_key(query);
        normalized.is_empty()
            || search_key(self.route).contains(&normalized)
            || search_key(self.label).contains(&normalized)
            || search_key(self.description).contains(&normalized)
            || self
                .keywords
                .iter()
                .any(|keyword| search_key(keyword).contains(&normalized))
    }

    /// Lower scores are more direct navigation matches. Ranking is kept in the
    /// catalog boundary so a broad parent page cannot hide an exact child page
    /// merely because the parent appears earlier in the durable index.
    fn match_rank(self, query: &str) -> u8 {
        let normalized = search_key(query);
        if normalized.is_empty()
            || search_key(self.route) == normalized
            || search_key(self.label) == normalized
        {
            return 0;
        }
        if self
            .keywords
            .iter()
            .any(|keyword| search_key(keyword) == normalized)
        {
            return 1;
        }
        2
    }

    pub(crate) const fn unavailable_reason(self) -> Option<&'static str> {
        match self.provider {
            PageProvider::Available => None,
            PageProvider::Unavailable(reason) => Some(reason),
        }
    }

    pub(crate) const fn is_available(self) -> bool {
        matches!(self.provider, PageProvider::Available)
    }
}

const fn provider_for(section: Section) -> PageProvider {
    match section {
        Section::Connectivity => PageProvider::Unavailable(
            "NetworkManager/ModemManager controls are not connected to This Node yet.",
        ),
        Section::DisplaySound => PageProvider::Unavailable(
            "Display and PipeWire mutation providers are not connected to This Node yet.",
        ),
        Section::Input => PageProvider::Unavailable(
            "Direct-seat input policy controls are not connected to This Node yet.",
        ),
        Section::Personalization => PageProvider::Unavailable(
            "This Node personalization persistence is not connected to this center yet.",
        ),
        _ => PageProvider::Available,
    }
}

const PAGE_INDEX: [PageEntry; 16] = [
    PageEntry {
        route: "this-node/overview",
        section: Section::Overview,
        provider: provider_for(Section::Overview),
        label: "Overview",
        description: "Health dashboard, identity, alerts, and recent events.",
        keywords: &["health", "status", "alerts", "health dashboard"],
    },
    PageEntry {
        route: "this-node/network",
        section: Section::Connectivity,
        provider: provider_for(Section::Connectivity),
        label: "Network",
        description: "Interfaces, connectivity providers, DNS, and topology.",
        keywords: &["wifi", "wi-fi", "ethernet", "cellular", "vpn", "topology"],
    },
    PageEntry {
        route: "this-node/display-sound",
        section: Section::DisplaySound,
        provider: provider_for(Section::DisplaySound),
        label: "Display & Sound",
        description: "Displays, brightness, audio routes, and privacy.",
        keywords: &[
            "display",
            "monitor",
            "brightness",
            "audio",
            "microphone",
            "camera",
        ],
    },
    PageEntry {
        route: "this-node/input",
        section: Section::Input,
        provider: provider_for(Section::Input),
        label: "Input & Accessibility",
        description: "Keyboard, pointer, touch, pen, and accessibility.",
        keywords: &["keyboard", "mouse", "touch", "stylus", "accessibility"],
    },
    PageEntry {
        route: "this-node/power-performance",
        section: Section::PowerPerformance,
        provider: provider_for(Section::PowerPerformance),
        label: "Power & Performance",
        description: "Battery, thermals, performance, and sleep policy.",
        keywords: &["battery", "thermal", "gpu", "performance", "sleep"],
    },
    PageEntry {
        route: "this-node/hardware",
        section: Section::Hardware,
        provider: provider_for(Section::Hardware),
        label: "Hardware",
        description: "Devices, firmware, storage, docks, and capabilities.",
        keywords: &["device", "firmware", "hardware", "capabilities"],
    },
    PageEntry {
        route: "this-node/storage",
        section: Section::Hardware,
        provider: provider_for(Section::Hardware),
        label: "Storage",
        description: "Drives, capacity, and storage health.",
        keywords: &["disk", "drive", "capacity", "storage"],
    },
    PageEntry {
        route: "this-node/peripherals",
        section: Section::Hardware,
        provider: provider_for(Section::Hardware),
        label: "Printers & Peripherals",
        description: "Printers, docks, USB, and attached peripherals.",
        keywords: &[
            "printer",
            "peripherals",
            "dock",
            "docking",
            "usb",
            "thunderbolt",
        ],
    },
    PageEntry {
        route: "this-node/personalization",
        section: Section::Personalization,
        provider: provider_for(Section::Personalization),
        label: "Personalization",
        description: "Appearance, layout, wallpaper, and local preferences.",
        keywords: &[
            "theme",
            "wallpaper",
            "background",
            "dark mode",
            "light mode",
        ],
    },
    PageEntry {
        route: "this-node/services",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Services & Applications",
        description: "Mesh services, applications, and service health.",
        keywords: &["services", "applications", "system"],
    },
    PageEntry {
        route: "this-node/users",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Users & Accounts",
        description: "Users, roles, accounts, and sign-in policy.",
        keywords: &["users", "accounts", "roles", "sign-in"],
    },
    PageEntry {
        route: "this-node/updates",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Updates & Lifecycle",
        description: "Updates, recovery, reset, and lifecycle state.",
        keywords: &["updates", "update", "recovery", "reset"],
    },
    PageEntry {
        route: "this-node/backup-restore",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Backup & Restore",
        description: "Backup, restore, and recovery evidence.",
        keywords: &["backup", "restore", "backup and restore"],
    },
    PageEntry {
        route: "this-node/diagnostics",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Diagnostics & Logs",
        description: "Diagnostics, logs, and operator-readable evidence.",
        keywords: &["diagnostics", "logs", "audit"],
    },
    PageEntry {
        route: "this-node/virtualization",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Virtualization & Remote Access",
        description: "Virtualization, trusted sessions, and remote access.",
        keywords: &["virtualization", "remote access", "trusted session"],
    },
    PageEntry {
        route: "this-node/system",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Mesh & System",
        description: "Nebula, mesh diagnostics, and system state.",
        keywords: &["nebula", "mesh", "system", "diagnostics"],
    },
];

/// The immutable index order is the navigation order.  Provider state never
/// inserts, removes, or reorders routes; it only changes their availability.
pub(crate) const fn page_index() -> &'static [PageEntry] {
    &PAGE_INDEX
}

pub(crate) fn search_pages(query: &str) -> Vec<PageEntry> {
    let mut matches: Vec<_> = PAGE_INDEX
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            page.matches(query)
                .then_some((page.match_rank(query), index, page))
        })
        .collect();
    matches.sort_by_key(|(rank, index, _)| (*rank, *index));
    matches
        .into_iter()
        .map(|(_, _, page)| page)
        .collect()
}

pub(crate) fn page_for_route(route: &str) -> Option<PageEntry> {
    PAGE_INDEX.into_iter().find(|page| page.route == route)
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
        .filter(|section| {
            section.matches(query)
                || PAGE_INDEX
                    .iter()
                    .any(|page| page.section == *section && page.matches(query))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        page_for_route, page_index, search, search_pages, PageProvider, Section, SectionGroup,
    };

    #[test]
    fn catalog_has_the_governed_eight_sections() {
        assert_eq!(Section::ALL.len(), 8);
        assert_eq!(Section::ALL[0], Section::Overview);
        assert_eq!(Section::ALL[7], Section::MeshSystem);

        let flattened: Vec<_> = SectionGroup::ALL
            .into_iter()
            .flat_map(SectionGroup::sections)
            .copied()
            .collect();
        assert_eq!(flattened, Section::ALL);
        for section in Section::ALL {
            assert!(section.group().sections().contains(&section));
        }
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
            ("fan", Section::PowerPerformance),
            ("health dashboard", Section::Overview),
            ("critical alerts", Section::Overview),
            ("interfaces", Section::Connectivity),
            ("topology", Section::Connectivity),
            ("privacy", Section::DisplaySound),
            ("camera", Section::DisplaySound),
            ("accessibility", Section::Input),
            ("firmware", Section::Hardware),
            ("device manager", Section::Hardware),
            ("capacity", Section::Hardware),
            ("dock", Section::Hardware),
            ("thunderbolt", Section::Hardware),
            ("peripherals", Section::Hardware),
            ("wallpaper", Section::Personalization),
            ("theme", Section::Personalization),
            ("nebula", Section::MeshSystem),
            ("updates", Section::MeshSystem),
            ("users", Section::MeshSystem),
            ("recovery", Section::MeshSystem),
            ("backup", Section::MeshSystem),
            ("logs", Section::MeshSystem),
        ];

        for (query, section) in aliases {
            assert_eq!(search(query), vec![section], "query: {query}");
        }
    }

    #[test]
    fn search_covers_the_surveyed_device_manager_vocabulary() {
        let aliases = [
            ("health dashboard", Section::Overview),
            ("critical alerts", Section::Overview),
            ("network topology", Section::Connectivity),
            ("camera privacy", Section::DisplaySound),
            ("accessibility", Section::Input),
            ("device manager", Section::Hardware),
            ("thunderbolt", Section::Hardware),
            ("users", Section::MeshSystem),
            ("recovery", Section::MeshSystem),
            ("backup and restore", Section::MeshSystem),
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

    #[test]
    fn page_index_has_stable_routes_and_preserves_section_order() {
        let pages = page_index();
        assert_eq!(
            pages.first().map(|page| page.route),
            Some("this-node/overview")
        );
        assert_eq!(
            pages.last().map(|page| page.route),
            Some("this-node/system")
        );
        assert!(pages.windows(2).all(|pair| pair[0].route != pair[1].route));
        assert!(pages
            .iter()
            .all(|page| Section::ALL.contains(&page.section)));
        assert!(pages.iter().any(|page| page.route == "this-node/network"));
        assert!(pages.iter().any(|page| page.route == "this-node/storage"));
    }

    #[test]
    fn page_search_reaches_provider_pages_without_hiding_unavailable_sections() {
        assert_eq!(search_pages("storage")[0].route, "this-node/storage");
        assert_eq!(search("storage"), vec![Section::Hardware]);
        assert_eq!(search("network topology"), vec![Section::Connectivity]);

        let display = page_for_route("this-node/display-sound").expect("stable route");
        assert_eq!(display.section, Section::DisplaySound);
        assert!(display.unavailable_reason().is_some());
        assert_eq!(page_for_route("this-node/not-a-page"), None);
    }

    #[test]
    fn every_indexed_page_has_explicit_provider_truth() {
        for page in page_index() {
            match page.section.unavailable_reason() {
                Some(reason) => assert_eq!(
                    page.provider,
                    PageProvider::Unavailable(reason),
                    "provider state for {} must follow section authority",
                    page.route
                ),
                None => {
                    assert_eq!(page.provider, PageProvider::Available, "{}", page.route);
                    assert!(page.is_available());
                    assert_eq!(page.unavailable_reason(), None);
                }
            }
        }
    }
}
