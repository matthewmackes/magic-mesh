//! The grouped navigation authority for the Workers control center.
//!
//! Providers remain leaf renderers, but the shell presents them through a
//! Windows Settings-inspired category hierarchy.  This keeps scope visible and
//! separates ordinary node/device controls from the advanced worker runtime.

use crate::this_node_catalog::{page_index, PageEntry};
use crate::workbench::Plane;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkersDestination {
    Overview,
    ThisNode,
    Network,
    Fleet,
    Provisioning,
    MeshMap,
    Discovery,
    ActionConsole,
    ThisNodePage(PageEntry),
    Phones,
    PhoneFiles,
    PhoneServices,
    PhoneCommands,
    PhonePair,
    SafePower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CatalogGroup {
    Home,
    ThisNode,
    Mesh,
    Fleet,
    ConnectedDevices,
    Advanced,
    SafePower,
}

impl CatalogGroup {
    pub(crate) const ALL: [Self; 7] = [
        Self::Home,
        Self::ThisNode,
        Self::Mesh,
        Self::Fleet,
        Self::ConnectedDevices,
        Self::Advanced,
        Self::SafePower,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Home => "Overview",
            Self::ThisNode => "This Node",
            Self::Mesh => "Mesh",
            Self::Fleet => "Fleet",
            Self::ConnectedDevices => "Connected Devices",
            Self::Advanced => "Advanced Operations",
            Self::SafePower => "Safe Power Cycle",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Home => "Health, attention, and the main control areas.",
            Self::ThisNode => "Hardware, settings, accounts, and lifecycle for this workstation.",
            Self::Mesh => "Topology, connectivity, and discovery across the mesh.",
            Self::Fleet => "Nodes, fleet health, and provisioning.",
            Self::ConnectedDevices => "Phones, pairing, permissions, files, and services.",
            Self::Advanced => "Worker runtime, relations, history, and governed actions.",
            Self::SafePower => {
                "Restart, shut down, suspend, or end a session with explicit safeguards."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) section: Option<&'static str>,
    pub(crate) group: CatalogGroup,
    pub(crate) destination: WorkersDestination,
}

fn page_label(page: PageEntry) -> &'static str {
    match page.route {
        "this-node/network" => "Network & internet",
        "this-node/display-sound" => "Displays",
        _ => page.label,
    }
}

fn page_section(page: PageEntry) -> &'static str {
    match page.route {
        "this-node/overview" => "Status",
        "this-node/network" => "Connectivity",
        "this-node/display-sound"
        | "this-node/audio"
        | "this-node/input"
        | "this-node/accessibility" => "Input & output",
        "this-node/power-performance" => "Power",
        "this-node/hardware" | "this-node/storage" | "this-node/peripherals" => "Hardware",
        "this-node/personalization" => "Personalization",
        "this-node/users" | "this-node/security-privacy" => "Accounts & privacy",
        "this-node/updates" | "this-node/recovery-reset" | "this-node/backup-restore" => {
            "Lifecycle"
        }
        _ => "System",
    }
}

/// Construct the catalog in presentation order.  Provider indexes remain the
/// source of truth for This Node routes, while scope/category order is explicit.
pub(crate) fn catalog() -> Vec<CatalogEntry> {
    let mut entries = vec![CatalogEntry {
        id: "workers/overview",
        label: "Overview",
        section: None,
        group: CatalogGroup::Home,
        destination: WorkersDestination::Overview,
    }];

    const THIS_NODE_SECTIONS: [&str; 9] = [
        "Status",
        "Connectivity",
        "Input & output",
        "Power",
        "Hardware",
        "Personalization",
        "Accounts & privacy",
        "Lifecycle",
        "System",
    ];
    for section in THIS_NODE_SECTIONS {
        entries.extend(
            page_index()
                .iter()
                .filter(|page| page_section(**page) == section)
                .map(|page| CatalogEntry {
                    id: page.route,
                    label: page_label(*page),
                    section: Some(section),
                    group: CatalogGroup::ThisNode,
                    destination: WorkersDestination::ThisNodePage(*page),
                }),
        );
    }

    entries.extend([
        CatalogEntry {
            id: "workers/mesh-map",
            label: "Topology",
            section: None,
            group: CatalogGroup::Mesh,
            destination: WorkersDestination::MeshMap,
        },
        CatalogEntry {
            id: "workers/network",
            label: "Mesh network",
            section: None,
            group: CatalogGroup::Mesh,
            destination: WorkersDestination::Network,
        },
        CatalogEntry {
            id: "workers/discovery",
            label: "Discovery",
            section: None,
            group: CatalogGroup::Mesh,
            destination: WorkersDestination::Discovery,
        },
        CatalogEntry {
            id: "workers/fleet",
            label: "Nodes",
            section: None,
            group: CatalogGroup::Fleet,
            destination: WorkersDestination::Fleet,
        },
        CatalogEntry {
            id: "workers/provisioning",
            label: "Provisioning",
            section: None,
            group: CatalogGroup::Fleet,
            destination: WorkersDestination::Provisioning,
        },
        CatalogEntry {
            id: "workers/phones",
            label: "Phones",
            section: None,
            group: CatalogGroup::ConnectedDevices,
            destination: WorkersDestination::Phones,
        },
        CatalogEntry {
            id: "workers/phone-pair",
            label: "Pair a phone",
            section: None,
            group: CatalogGroup::ConnectedDevices,
            destination: WorkersDestination::PhonePair,
        },
        CatalogEntry {
            id: "workers/phone-files",
            label: "Phone files",
            section: None,
            group: CatalogGroup::ConnectedDevices,
            destination: WorkersDestination::PhoneFiles,
        },
        CatalogEntry {
            id: "workers/phone-services",
            label: "Phone services",
            section: None,
            group: CatalogGroup::ConnectedDevices,
            destination: WorkersDestination::PhoneServices,
        },
        CatalogEntry {
            id: "workers/phone-commands",
            label: "Phone commands",
            section: None,
            group: CatalogGroup::ConnectedDevices,
            destination: WorkersDestination::PhoneCommands,
        },
        CatalogEntry {
            id: "workers/action-console",
            label: "Worker runtime",
            section: None,
            group: CatalogGroup::Advanced,
            destination: WorkersDestination::ActionConsole,
        },
        CatalogEntry {
            id: "workers/safe-power",
            label: "Safe Power Cycle Controls",
            section: None,
            group: CatalogGroup::SafePower,
            destination: WorkersDestination::SafePower,
        },
    ]);
    entries
}

pub(crate) const fn default_destination() -> WorkersDestination {
    WorkersDestination::Overview
}

pub(crate) fn group(destination: WorkersDestination) -> CatalogGroup {
    catalog()
        .into_iter()
        .find(|entry| entry.destination == destination)
        .map_or(CatalogGroup::ThisNode, |entry| entry.group)
}

pub(crate) fn landing_destination(group: CatalogGroup) -> WorkersDestination {
    catalog()
        .into_iter()
        .find(|entry| entry.group == group)
        .map_or(WorkersDestination::Overview, |entry| entry.destination)
}

pub(crate) fn plane(destination: WorkersDestination) -> Option<Plane> {
    match destination {
        WorkersDestination::ThisNode => Some(Plane::ThisNode),
        WorkersDestination::Network => Some(Plane::Network),
        WorkersDestination::Fleet => Some(Plane::Fleet),
        WorkersDestination::Provisioning => Some(Plane::Provisioning),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_is_unique_and_grouped_in_control_center_order() {
        let entries = catalog();
        assert!(!entries.is_empty());
        assert_eq!(entries[0].destination, WorkersDestination::Overview);
        assert_eq!(default_destination(), WorkersDestination::Overview);
        let ids: HashSet<_> = entries.iter().map(|entry| entry.id).collect();
        assert_eq!(ids.len(), entries.len());
        assert!(entries
            .windows(2)
            .all(|pair| pair[0].group <= pair[1].group));
        assert!(entries
            .iter()
            .all(|entry| entry.destination != WorkersDestination::ThisNode));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry.destination, WorkersDestination::ThisNodePage(_)))
                .count(),
            page_index().len()
        );
        for group in CatalogGroup::ALL {
            assert!(entries.iter().any(|entry| entry.group == group));
        }
        assert_eq!(
            entries.last().map(|entry| entry.destination),
            Some(WorkersDestination::SafePower)
        );
    }
}
