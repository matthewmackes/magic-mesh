//! The one flat navigation authority for the Workers workspace.
//!
//! Entries are deliberately leaves: providers render content, but they do not
//! get to add a second rail, tab strip, or route menu.

use crate::this_node_catalog::{page_index, PageEntry};
use crate::workbench::Plane;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkersDestination {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) destination: WorkersDestination,
}

fn page_label(page: PageEntry) -> &'static str {
    match page.route {
        "this-node/network" => "Network (This Node)",
        _ => page.label,
    }
}

/// Construct the catalog at runtime so the existing governed This Node page
/// index remains the source of truth. The result is sorted by visible label.
pub(crate) fn catalog() -> Vec<CatalogEntry> {
    let mut entries = vec![
        CatalogEntry {
            id: "workers/action-console",
            label: "Action Console",
            destination: WorkersDestination::ActionConsole,
        },
        CatalogEntry {
            id: "workers/discovery",
            label: "Discovery",
            destination: WorkersDestination::Discovery,
        },
        CatalogEntry {
            id: "workers/fleet",
            label: "Fleet",
            destination: WorkersDestination::Fleet,
        },
        CatalogEntry {
            id: "workers/mesh-map",
            label: "Mesh Map",
            destination: WorkersDestination::MeshMap,
        },
        CatalogEntry {
            id: "workers/network",
            label: "Network",
            destination: WorkersDestination::Network,
        },
        CatalogEntry {
            id: "workers/phone-commands",
            label: "Commands",
            destination: WorkersDestination::PhoneCommands,
        },
        CatalogEntry {
            id: "workers/phone-files",
            label: "Files",
            destination: WorkersDestination::PhoneFiles,
        },
        CatalogEntry {
            id: "workers/phone-pair",
            label: "Pair",
            destination: WorkersDestination::PhonePair,
        },
        CatalogEntry {
            id: "workers/phone-services",
            label: "Services",
            destination: WorkersDestination::PhoneServices,
        },
        CatalogEntry {
            id: "workers/phones",
            label: "Phones",
            destination: WorkersDestination::Phones,
        },
        CatalogEntry {
            id: "workers/provisioning",
            label: "Provisioning",
            destination: WorkersDestination::Provisioning,
        },
    ];
    entries.extend(page_index().iter().map(|page| CatalogEntry {
        id: page.route,
        label: page_label(*page),
        destination: WorkersDestination::ThisNodePage(*page),
    }));
    entries.sort_by_key(|entry| (entry.label.to_ascii_lowercase(), entry.id));
    entries
}

pub(crate) fn default_destination() -> WorkersDestination {
    WorkersDestination::ThisNodePage(
        *page_index()
            .first()
            .expect("the governed This Node catalog must have an overview page"),
    )
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
    fn catalog_is_unique_and_deterministically_sorted() {
        let entries = catalog();
        assert!(!entries.is_empty());
        assert_eq!(entries[0].label, "Accessibility");
        assert_eq!(
            default_destination(),
            WorkersDestination::ThisNodePage(page_index()[0])
        );
        let ids: HashSet<_> = entries.iter().map(|entry| entry.id).collect();
        let labels: HashSet<_> = entries.iter().map(|entry| entry.label).collect();
        assert_eq!(ids.len(), entries.len());
        assert_eq!(labels.len(), entries.len());
        assert!(entries.windows(2).all(|pair| {
            (pair[0].label.to_ascii_lowercase(), pair[0].id)
                <= (pair[1].label.to_ascii_lowercase(), pair[1].id)
        }));
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
    }
}
