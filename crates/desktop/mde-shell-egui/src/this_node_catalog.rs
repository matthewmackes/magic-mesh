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
            Self::DisplaySound => "Display & Audio",
            Self::Input => "Input",
            Self::PowerPerformance => "Power & Performance",
            Self::Hardware => "Hardware",
            Self::Personalization => "Personalization",
            Self::MeshSystem => "Mesh & System",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Overview => "Node identity, inventory, and available controls.",
            Self::Connectivity => "Wi-Fi, Ethernet, cellular, VPN, DNS, and proxy.",
            Self::DisplaySound => "Displays, brightness, audio routes, and privacy controls.",
            Self::Input => "Keyboard, pointer, touch, pen, and seat policy.",
            Self::PowerPerformance => "Battery, thermals, performance, and sleep policy.",
            Self::Hardware => "Devices, firmware, storage, docks, and capabilities.",
            Self::Personalization => "Appearance, layout, wallpaper, and local preferences.",
            Self::MeshSystem => "Nebula, services, updates, and system configuration.",
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
            "virtualization",
            "remote access",
            "time",
            "language",
            "region",
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
            // NetworkManager observation plus the bounded Wi-Fi radio action
            // now reach the typed seat provider. Profile/credential workflows
            // remain separately unavailable until SecretAgent contracts land.
            Self::Connectivity => None,
            // Display and input now have read-only inventory providers. Their
            // mutation boundaries are rendered by the detail views instead of
            // hiding the observed page behind an unavailable route.
            Self::DisplaySound | Self::Input => None,
            Self::Personalization => None,
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
    pub(crate) fn matches(self, query: &str) -> bool {
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
        // NetworkManager observation plus the bounded Wi-Fi radio action now
        // reach the typed seat provider. Profile/credential workflows remain
        // separately unavailable until SecretAgent contracts land.
        Section::Connectivity => PageProvider::Available,
        // These pages now have bounded read-only observations. Mutation
        // controls remain separately fail-closed behind typed seat providers.
        Section::DisplaySound | Section::Input => PageProvider::Available,
        Section::Personalization => PageProvider::Available,
        _ => PageProvider::Available,
    }
}

const PAGE_INDEX: [PageEntry; 20] = [
    PageEntry {
        route: "this-node/overview",
        section: Section::Overview,
        provider: provider_for(Section::Overview),
        label: "Overview",
        description: "Node identity, inventory, and recent events.",
        keywords: &["identity", "inventory", "overview"],
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
        label: "Displays",
        description: "Display arrangement, brightness, resolution, and refresh rate.",
        keywords: &[
            "display",
            "monitor",
            "brightness",
            "resolution",
            "refresh rate",
            "arrangement",
        ],
    },
    PageEntry {
        route: "this-node/audio",
        section: Section::DisplaySound,
        provider: provider_for(Section::DisplaySound),
        label: "Sound",
        description: "Volume, output and input devices, routes, and mute state.",
        keywords: &["audio", "sound", "speaker", "microphone", "volume", "mute"],
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
        provider: PageProvider::Available,
        label: "Users & Accounts",
        description: "Users, roles, accounts, and sign-in policy.",
        keywords: &["users", "accounts", "roles", "sign-in"],
    },
    PageEntry {
        route: "this-node/updates",
        section: Section::MeshSystem,
        provider: provider_for(Section::MeshSystem),
        label: "Updates & Lifecycle",
        description:
            "Shared ONBOARD session for updates and lifecycle; mutation stays with mackesd.",
        keywords: &[
            "updates",
            "update",
            "recovery",
            "reset",
            "onboard",
            "offboard",
            "lifecycle",
        ],
    },
    PageEntry {
        route: "this-node/recovery-reset",
        section: Section::MeshSystem,
        provider: PageProvider::Unavailable(
            "Recovery and reset remain behind a privileged node provider.",
        ),
        label: "Recovery & Reset",
        description: "Shared ONBOARD session for recovery and reset; mutation stays with mackesd.",
        keywords: &["recovery", "reset", "restore", "safe recovery", "lifecycle"],
    },
    PageEntry {
        route: "this-node/backup-restore",
        section: Section::MeshSystem,
        provider: PageProvider::Available,
        label: "Backup & Restore",
        description: "Backup evidence only. Restore and wipe stay with mackesd.",
        keywords: &["backup", "restore", "backup and restore"],
    },
    PageEntry {
        route: "this-node/virtualization",
        section: Section::MeshSystem,
        provider: PageProvider::Available,
        label: "Virtualization & Remote Access",
        description: "Virtualization, trusted sessions, and remote access.",
        keywords: &["virtualization", "remote access", "trusted session"],
    },
    PageEntry {
        route: "this-node/security-privacy",
        section: Section::DisplaySound,
        provider: PageProvider::Available,
        label: "Security & Privacy",
        description: "Encryption posture, privacy controls, and security state.",
        keywords: &["security", "privacy", "encryption", "camera privacy"],
    },
    PageEntry {
        route: "this-node/accessibility",
        section: Section::Input,
        provider: PageProvider::Available,
        label: "Accessibility",
        description: "Assistive input, display, and interaction preferences.",
        keywords: &["accessibility", "assistive", "screen reader", "contrast"],
    },
    PageEntry {
        route: "this-node/time-language-region",
        section: Section::MeshSystem,
        provider: PageProvider::Available,
        label: "Time, Language & Region",
        description: "Clock, locale, language, keyboard region, and time zone.",
        keywords: &["time", "language", "locale", "region", "time zone"],
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
    matches.into_iter().map(|(_, _, page)| page).collect()
}

pub(crate) fn page_for_route(route: &str) -> Option<PageEntry> {
    PAGE_INDEX.into_iter().find(|page| page.route == route)
}

/// The stable landing page for a governed section. Child pages retain their
/// catalog order so selecting a parent from the hierarchy never leaves the
/// detail pane pointing at an unrelated route.
pub(crate) fn first_page_for_section(section: Section) -> Option<PageEntry> {
    PAGE_INDEX.into_iter().find(|page| page.section == section)
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
    fn search_is_case_insensitive_and_keeps_provider_sections_discoverable() {
        assert_eq!(search("SOUND"), vec![Section::DisplaySound]);
        assert_eq!(search("wifi"), vec![Section::Connectivity]);
        assert_eq!(search(""), Section::ALL.to_vec());
        assert!(Section::Connectivity.unavailable_reason().is_none());
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
        ];

        for (query, section) in aliases {
            assert_eq!(search(query), vec![section], "query: {query}");
        }
    }

    #[test]
    fn search_covers_the_surveyed_device_manager_vocabulary() {
        let aliases = [
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
    fn personalization_route_matches_the_live_system_provider() {
        assert!(Section::Personalization.unavailable_reason().is_none());
        let page = page_for_route("this-node/personalization").expect("personalization route");
        assert!(page.is_available());
        assert!(page.unavailable_reason().is_none());
    }

    #[test]
    fn page_search_reaches_provider_pages_without_hiding_unavailable_sections() {
        assert_eq!(search_pages("storage")[0].route, "this-node/storage");
        assert_eq!(search("storage"), vec![Section::Hardware]);
        assert_eq!(search("network topology"), vec![Section::Connectivity]);

        let display = page_for_route("this-node/display-sound").expect("stable route");
        assert_eq!(display.section, Section::DisplaySound);
        assert!(display.is_available());
        let audio = page_for_route("this-node/audio").expect("sound has its own stable route");
        assert_eq!(audio.section, Section::DisplaySound);
        assert_eq!(search_pages("volume")[0], audio);
        assert_eq!(page_for_route("this-node/not-a-page"), None);
    }

    #[test]
    fn every_indexed_page_has_explicit_provider_truth() {
        for page in page_index() {
            if let Some(reason) = page.section.unavailable_reason() {
                assert_eq!(
                    page.provider,
                    PageProvider::Unavailable(reason),
                    "provider state for {} must follow section authority",
                    page.route
                );
            } else if let Some(reason) = page.unavailable_reason() {
                assert!(
                    !reason.trim().is_empty(),
                    "page-specific provider reason for {} must be readable",
                    page.route
                );
                assert!(!page.is_available());
            } else {
                assert!(page.is_available(), "{}", page.route);
            }
        }
        let users = page_for_route("this-node/users").expect("users page route");
        assert!(
            users.is_available(),
            "aggregate users observation is available"
        );
        assert!(users.unavailable_reason().is_none());

        assert!(page_for_route("this-node/virtualization")
            .expect("remote proofing route")
            .is_available());
        assert!(page_for_route("this-node/backup-restore")
            .expect("backup route")
            .is_available());
        let recovery = page_for_route("this-node/recovery-reset").expect("recovery route");
        assert!(!recovery.is_available());
        assert!(recovery
            .unavailable_reason()
            .is_some_and(|reason| reason.contains("privileged")));
        assert!(
            recovery.description.contains("ONBOARD session"),
            "recovery-reset must name the shared lifecycle session"
        );
        let updates = page_for_route("this-node/updates").expect("updates route");
        assert!(
            updates.description.contains("ONBOARD session"),
            "updates must name the same shared lifecycle session"
        );
        let backup = page_for_route("this-node/backup-restore").expect("backup route");
        assert!(
            backup.description.contains("stay with mackesd"),
            "backup-restore must not claim dest wipe"
        );
        let privacy = page_for_route("this-node/security-privacy").expect("privacy page route");
        assert!(
            privacy.is_available(),
            "bounded privacy observations are available"
        );
        assert!(privacy.unavailable_reason().is_none());
        for route in ["this-node/accessibility", "this-node/time-language-region"] {
            assert!(
                page_for_route(route)
                    .expect("partial provider route")
                    .is_available(),
                "durable System continuity should keep {route} reachable"
            );
        }
        assert_eq!(
            search_pages("security")[0].route,
            "this-node/security-privacy"
        );
        assert_eq!(
            search_pages("time zone")[0].route,
            "this-node/time-language-region"
        );
        assert!(page_for_route("this-node/diagnostics").is_none());
    }
}
