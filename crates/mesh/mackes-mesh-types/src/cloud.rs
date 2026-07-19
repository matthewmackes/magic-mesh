//! Provider-neutral Construct Cloud shared contracts.
//!
//! This module is the forward path for cloud consumers that should not bind to
//! an OpenStack module path.  The installed OpenStack adapter still owns the
//! first production parser, so the facade accepts both the generic Construct
//! Cloud wire shape and the legacy OpenStack/Keystone collection payloads.

use serde::{Deserialize, Serialize};

pub use crate::openstack::{
    absent_health, default_collection, hot_resource_type, reverse_generate_hot, shape_health,
    CatalogEndpoint as CloudEndpoint, CatalogParseError, CatalogService as CloudService,
    EndpointInterface as CloudEndpointInterface, HealthState as CloudHealthState,
    HeatEvent as CloudStackEvent, HeatOutput as CloudStackOutput, HeatPreview as CloudStackPreview,
    HeatResource as CloudStackResource, HeatStackDetail as CloudStackDetail, ProbeOutcome,
    ResourceParseError, ResourceRow as CloudResourceRow, ResourceTable as CloudResourceTable,
    ServiceCatalog as CloudServiceCatalog, ServiceHealth as CloudServiceHealth,
};

/// The cloud provider adapter that produced or will consume a shared contract.
///
/// `Openstack` remains a valid compatibility backend while the product-facing
/// cloud contract moves to this provider-neutral module path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudProviderAdapter {
    /// Generic Construct Cloud contract producer.
    ConstructCloud,
    /// Legacy installed OpenStack/Kolla adapter.
    Openstack,
    /// Simulator or fake provider used by tests and offline shell workflows.
    Simulator,
}

impl CloudProviderAdapter {
    /// Stable token used in persisted metadata and test fixtures.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConstructCloud => "construct_cloud",
            Self::Openstack => "openstack",
            Self::Simulator => "simulator",
        }
    }

    /// Product-facing label for UI and operator text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ConstructCloud => "Construct Cloud",
            Self::Openstack => "OpenStack adapter",
            Self::Simulator => "Cloud simulator",
        }
    }

    /// Whether this adapter is the compatibility OpenStack backend.
    #[must_use]
    pub const fn is_legacy_openstack(self) -> bool {
        matches!(self, Self::Openstack)
    }
}

/// Parse a provider-neutral Construct Cloud catalog, accepting the existing
/// OpenStack Keystone token response as a compatibility fallback.
///
/// Generic providers should publish the [`CloudServiceCatalog`] JSON shape
/// directly.  The fallback keeps the current OpenStack adapter and its contract
/// fixtures working while consumers migrate from `openstack::*` imports.
///
/// # Errors
/// Returns [`CatalogParseError`] when neither shape is recognized.
pub fn parse_service_catalog_json(body: &str) -> Result<CloudServiceCatalog, CatalogParseError> {
    let trimmed = body.trim();
    match serde_json::from_str::<CloudServiceCatalog>(trimmed) {
        Ok(catalog) => Ok(catalog),
        Err(generic_error) => crate::openstack::ServiceCatalog::from_keystone_token_json(trimmed)
            .map_err(|openstack_error| {
                CatalogParseError(format!(
                    "provider catalog JSON did not match Construct Cloud catalog \
                         ({generic_error}) or OpenStack Keystone catalog ({openstack_error})"
                ))
            }),
    }
}

/// Parse a provider-neutral resource table, accepting an existing OpenStack
/// collection response as a compatibility fallback.
///
/// Generic providers should publish [`CloudResourceTable`] JSON directly.  Empty
/// `service_type` or `collection` fields are filled from the requested context so
/// small simulator fixtures can stay concise.
///
/// # Errors
/// Returns [`ResourceParseError`] when neither shape is recognized.
pub fn parse_resource_table_json(
    service_type: &str,
    collection: &str,
    body: &str,
) -> Result<CloudResourceTable, ResourceParseError> {
    let trimmed = body.trim();
    match serde_json::from_str::<CloudResourceTable>(trimmed) {
        Ok(mut table) => {
            if table.service_type.trim().is_empty() {
                table.service_type = service_type.to_string();
            }
            if table.collection.trim().is_empty() {
                table.collection = collection.to_string();
            }
            Ok(table)
        }
        Err(generic_error) => {
            crate::openstack::ResourceTable::from_collection_json(service_type, collection, trimmed)
                .map_err(|openstack_error| {
                    ResourceParseError(format!(
                        "provider resource table JSON did not match Construct Cloud table \
                         ({generic_error}) or OpenStack collection ({openstack_error})"
                    ))
                })
        }
    }
}

/// Provider-neutral name for the currently supported default collection mapper.
#[must_use]
pub fn default_resource_collection(service_type: &str) -> Option<&'static str> {
    default_collection(service_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_neutral_catalog_json_parses_without_keystone_wrapper() {
        let body = r#"{
            "region": "vehicle-lab",
            "services": [{
                "type": "compute",
                "name": "generic-compute",
                "endpoints": [{
                    "interface": "public",
                    "url": "https://cloud.mesh/compute",
                    "region": "vehicle-lab"
                }]
            }]
        }"#;

        let catalog = parse_service_catalog_json(body).expect("generic catalog parses");

        assert_eq!(catalog.region.as_deref(), Some("vehicle-lab"));
        assert_eq!(catalog.services[0].service_type, "compute");
        assert_eq!(
            catalog.services[0].primary_url(),
            Some("https://cloud.mesh/compute")
        );
    }

    #[test]
    fn openstack_catalog_payload_still_parses_through_the_cloud_facade() {
        let body = r#"{"token":{"catalog":[{
            "type":"compute",
            "name":"nova",
            "endpoints":[{
                "interface":"public",
                "url":"https://openstack.mesh/compute/v2.1",
                "region":"RegionOne"
            }]
        }]}}"#;

        let catalog = parse_service_catalog_json(body).expect("keystone fallback parses");

        assert_eq!(catalog.region.as_deref(), Some("RegionOne"));
        assert_eq!(catalog.services[0].name.as_deref(), Some("nova"));
    }

    #[test]
    fn provider_neutral_resource_table_json_parses_without_collection_wrapper() {
        let body = r#"{
            "service_type": "compute",
            "collection": "instances",
            "columns": ["name", "status"],
            "rows": [{"id": "inst-1", "cells": ["Dispatch", "running"]}]
        }"#;

        let table =
            parse_resource_table_json("compute", "instances", body).expect("generic table parses");

        assert_eq!(table.service_type, "compute");
        assert_eq!(table.collection, "instances");
        assert_eq!(table.row_label(&table.rows[0]), "Dispatch");
    }

    #[test]
    fn openstack_collection_payload_still_parses_through_the_cloud_facade() {
        let body = r#"{"servers":[{
            "id":"server-1",
            "name":"legacy-worker",
            "status":"ACTIVE"
        }]}"#;

        let table = parse_resource_table_json("compute", "servers/detail", body)
            .expect("openstack collection fallback parses");

        assert_eq!(table.service_type, "compute");
        assert_eq!(table.collection, "servers/detail");
        assert_eq!(table.row_label(&table.rows[0]), "legacy-worker");
    }

    #[test]
    fn adapter_labels_keep_openstack_as_compatibility_not_product_default() {
        assert_eq!(
            CloudProviderAdapter::ConstructCloud.as_str(),
            "construct_cloud"
        );
        assert_eq!(
            CloudProviderAdapter::ConstructCloud.label(),
            "Construct Cloud"
        );
        assert_eq!(CloudProviderAdapter::Openstack.label(), "OpenStack adapter");
        assert!(CloudProviderAdapter::Openstack.is_legacy_openstack());
        assert!(!CloudProviderAdapter::ConstructCloud.is_legacy_openstack());
    }

    #[test]
    fn default_resource_collection_is_the_provider_neutral_import_path() {
        assert_eq!(
            default_resource_collection("compute"),
            Some("servers/detail")
        );
        assert_eq!(default_resource_collection("identity"), None);
    }
}
