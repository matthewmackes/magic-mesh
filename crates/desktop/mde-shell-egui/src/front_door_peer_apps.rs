//! Front Door peer-app discovery client.
//!
//! The daemon owns `action/apps/peer-list`; the shell mirrors only the JSON it
//! needs and never depends on the daemon crate. Requests are non-blocking Bus
//! RPCs, and replies are cached per peer so focusing a mesh node can reveal its
//! installed apps without polling the network from the render path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mackes_mesh_types::app_catalog::{FlatpakAppCatalog, FlatpakInstallState};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};
use serde::Deserialize;
use serde_json::json;

use crate::bus_reader::BusReader;
use crate::front_door::FrontDoorPeerApp;

const PEER_APPS_ACTION: &str = "action/apps/peer-list";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_REFRESH: Duration = Duration::from_secs(30);
const RETRY_BACKOFF: Duration = Duration::from_secs(1);
const MAX_RETRIES: u8 = 2;
const MAX_NODE_ID_BYTES: usize = 128;

#[derive(Debug, Clone)]
struct PendingPeerAppsRequest {
    node: String,
    ulid: String,
    sent: Instant,
}

#[derive(Debug, Clone)]
struct PeerAppsCache {
    apps: Vec<FrontDoorPeerApp>,
    refreshed: Instant,
    reconnecting: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct PeerAppsReply {
    ok: bool,
    node: String,
    entries: Vec<PeerAppEntry>,
    catalog: Option<FlatpakAppCatalog>,
    error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct PeerAppEntry {
    id: String,
    name: String,
    source: String,
    node: String,
    icon: String,
    health: String,
    state: String,
}

#[derive(Debug)]
pub(crate) struct FrontDoorPeerAppsState {
    bus_root: Option<PathBuf>,
    active_node: Option<String>,
    pending: Option<PendingPeerAppsRequest>,
    cache: HashMap<String, PeerAppsCache>,
    last_note: Option<String>,
    retry_not_before: Option<Instant>,
    retry_count: u8,
}

impl Default for FrontDoorPeerAppsState {
    fn default() -> Self {
        Self::new(mde_bus::client_data_dir())
    }
}

impl FrontDoorPeerAppsState {
    pub(crate) fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            bus_root,
            active_node: None,
            pending: None,
            cache: HashMap::new(),
            last_note: None,
            retry_not_before: None,
            retry_count: 0,
        }
    }

    pub(crate) fn drive_for_focus(&mut self, focused_node: Option<&str>) {
        let now = Instant::now();
        self.resolve_pending(now);

        let Some(node) = focused_node.and_then(clean_node) else {
            return;
        };
        if self.active_node.as_deref() != Some(node) {
            self.active_node = Some(node.to_owned());
            if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.node != node)
            {
                self.pending = None;
                self.retry_not_before = None;
                self.retry_count = 0;
            }
        }
        if self.pending.is_some()
            || self
                .retry_not_before
                .is_some_and(|not_before| now < not_before)
            || !self.cache_stale(node, now)
        {
            return;
        }
        self.publish_request_for(node, now);
    }

    pub(crate) fn items(&self) -> Vec<FrontDoorPeerApp> {
        let Some(node) = self.active_node.as_deref() else {
            return Vec::new();
        };
        self.cache
            .get(node)
            .map(|cache| {
                if !cache.reconnecting {
                    return cache.apps.clone();
                }
                cache
                    .apps
                    .iter()
                    .cloned()
                    .map(|mut app| {
                        // Preserve permission and identity metadata while a
                        // formerly launchable row is reconnecting.
                        if app.state.trim().eq_ignore_ascii_case("installed")
                            && app.health.trim().eq_ignore_ascii_case("ready")
                        {
                            app.health = "reconnecting".to_owned();
                            app.state = "reconnecting".to_owned();
                        }
                        app
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn cache_stale(&self, node: &str, now: Instant) -> bool {
        self.cache
            .get(node)
            .is_none_or(|cache| now.duration_since(cache.refreshed) >= CACHE_REFRESH)
    }

    fn resolve_pending(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if let Some(reply) = self.read_reply(&pending.ulid) {
            let (apps, note) = fold_peer_apps_reply(&pending.node, reply);
            self.pending = None;
            if let Some(note) = note {
                self.mark_reconnecting(&pending.node);
                if self.retry_count < MAX_RETRIES {
                    self.retry_count = self.retry_count.saturating_add(1);
                    self.retry_not_before = Some(now + RETRY_BACKOFF);
                    self.last_note = Some(note);
                } else {
                    self.mark_discovery_failed(&pending.node, now, note);
                }
            } else {
                self.cache.insert(
                    pending.node.clone(),
                    PeerAppsCache {
                        apps,
                        refreshed: now,
                        reconnecting: false,
                    },
                );
                self.last_note = None;
                self.retry_not_before = None;
                self.retry_count = 0;
            }
        } else if now.duration_since(pending.sent) >= REQUEST_TIMEOUT {
            self.pending = None;
            self.mark_reconnecting(&pending.node);
            if self.retry_count < MAX_RETRIES {
                self.retry_count = self.retry_count.saturating_add(1);
                self.retry_not_before = Some(now + RETRY_BACKOFF);
                self.last_note = Some(format!(
                    "reconnecting to {} for app discovery (retry {}/{})",
                    pending.node, self.retry_count, MAX_RETRIES
                ));
            } else {
                self.mark_discovery_failed(
                    &pending.node,
                    now,
                    format!("{} did not answer app discovery", pending.node),
                );
            }
        }
    }

    fn mark_reconnecting(&mut self, node: &str) {
        if let Some(cache) = self.cache.get_mut(node) {
            cache.reconnecting = true;
        }
    }

    fn mark_discovery_failed(&mut self, node: &str, now: Instant, note: String) {
        // Preserve the last admitted projection while the peer is unavailable;
        // replacing it with an empty list would make a transient reconnect
        // look like an uninstall and erase the user's permission/lifecycle
        // context from Front Door.
        let apps = self
            .cache
            .get(node)
            .map(|cache| cache.apps.clone())
            .unwrap_or_default();
        self.cache.insert(
            node.to_owned(),
            PeerAppsCache {
                apps,
                refreshed: now,
                reconnecting: true,
            },
        );
        self.retry_not_before = None;
        self.last_note = Some(note);
    }

    fn publish_request_for(&mut self, node: &str, now: Instant) {
        let body = json!({ "node": node }).to_string();
        let Some(persist) = self.persist() else {
            self.mark_reconnecting(node);
            if self.retry_count < MAX_RETRIES {
                self.retry_count = self.retry_count.saturating_add(1);
                self.retry_not_before = Some(now + RETRY_BACKOFF);
                self.last_note = Some(format!(
                    "reconnecting to {node} for app discovery (retry {}/{})",
                    self.retry_count, MAX_RETRIES
                ));
            } else {
                self.mark_discovery_failed(
                    node,
                    now,
                    "the local mesh Bus is unavailable".to_owned(),
                );
            }
            return;
        };
        match publish_request(
            &persist,
            PEER_APPS_ACTION,
            Priority::Default,
            None,
            Some(&body),
        ) {
            Ok(ulid) => {
                self.pending = Some(PendingPeerAppsRequest {
                    node: node.to_owned(),
                    ulid,
                    sent: now,
                });
                self.last_note = None;
            }
            Err(err) => {
                self.mark_reconnecting(node);
                if self.retry_count < MAX_RETRIES {
                    self.retry_count = self.retry_count.saturating_add(1);
                    self.retry_not_before = Some(now + RETRY_BACKOFF);
                    self.last_note = Some(format!(
                        "reconnecting to {node} for app discovery (retry {}/{})",
                        self.retry_count, MAX_RETRIES
                    ));
                } else {
                    self.mark_discovery_failed(
                        node,
                        now,
                        format!("could not ask {node} for apps: {err}"),
                    );
                }
            }
        }
    }

    fn read_reply(&self, ulid: &str) -> Option<PeerAppsReply> {
        let persist = self.persist()?;
        let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
        let body = msgs.first()?.body.as_deref()?;
        serde_json::from_str(body).ok()
    }

    fn persist(&self) -> Option<Persist> {
        BusReader::new(self.bus_root.clone()).open()
    }

    #[cfg(test)]
    pub(crate) fn pending_ulid(&self) -> Option<&str> {
        self.pending.as_ref().map(|pending| pending.ulid.as_str())
    }
}

fn fold_peer_apps_reply(
    requested_node: &str,
    mut reply: PeerAppsReply,
) -> (Vec<FrontDoorPeerApp>, Option<String>) {
    if !reply.ok {
        return (
            Vec::new(),
            Some(
                reply
                    .error
                    .unwrap_or_else(|| format!("{requested_node} app discovery failed")),
            ),
        );
    }
    // A reply is correlated by ULID, but the node field is still part of the
    // application identity contract.  Never cache a valid catalog or legacy
    // row from a different peer under the node that was requested.
    let Some(reply_node) = clean_node(&reply.node).map(str::to_owned) else {
        return (
            Vec::new(),
            Some(format!(
                "{requested_node} app discovery reply omitted its node"
            )),
        );
    };
    if reply_node != requested_node {
        return (
            Vec::new(),
            Some(format!(
                "app discovery reply was for {reply_node}, not {requested_node}"
            )),
        );
    }
    if let Some(catalog) = reply.catalog.take() {
        return match catalog.admitted() {
            Ok(catalog) => {
                let catalog_revision = catalog.revision.clone();
                (
                    catalog
                        .entries
                        .into_iter()
                        .map(|entry| {
                            let launchable = entry.is_launchable()
                                && entry
                                    .supported_actions
                                    .iter()
                                    .any(|action| action.trim().eq_ignore_ascii_case("launch"));
                            let state =
                                if !launchable && entry.state == FlatpakInstallState::Installed {
                                    if entry.provenance.signature.is_some() {
                                        "unavailable".to_owned()
                                    } else {
                                        "unsigned".to_owned()
                                    }
                                } else {
                                    flatpak_state_label(entry.state).to_owned()
                                };
                            FrontDoorPeerApp {
                                id: entry.app_id,
                                name: entry.display_name,
                                node: reply_node.clone(),
                                source: "flatpak".to_owned(),
                                icon: entry.icon_reference,
                                health: if launchable {
                                    "ready".to_owned()
                                } else {
                                    "unavailable".to_owned()
                                },
                                state,
                                catalog_revision: Some(catalog_revision.clone()),
                                guest_profile: Some(entry.guest_profile),
                                requested_capabilities: entry.declared_capabilities,
                            }
                        })
                        .collect(),
                    None,
                )
            }
            Err(error) => (
                Vec::new(),
                Some(format!(
                    "{reply_node} sent an invalid Flatpak catalog: {error:?}"
                )),
            ),
        };
    }
    let apps = reply
        .entries
        .into_iter()
        .filter_map(|entry| {
            let node = match clean_node(&entry.node) {
                Some(node) if node == reply_node => node,
                Some(_) => return None,
                None => reply_node.as_str(),
            };
            let id = entry.id.trim();
            let name = entry.name.trim();
            if node.is_empty() || id.is_empty() || name.is_empty() {
                return None;
            }
            let app = FrontDoorPeerApp {
                id: id.to_owned(),
                name: name.to_owned(),
                node: node.to_owned(),
                source: entry.source,
                icon: entry.icon,
                health: entry.health,
                state: entry.state,
                catalog_revision: None,
                guest_profile: None,
                requested_capabilities: Vec::new(),
            };
            // Reject malformed Flatpak identities at the untrusted reply
            // boundary, before they enter the cache or Front Door ranker.
            app.flatpak_id().ok()?;
            Some(app)
        })
        .collect();
    (apps, None)
}

fn flatpak_state_label(state: FlatpakInstallState) -> &'static str {
    match state {
        FlatpakInstallState::Installed => "installed",
        FlatpakInstallState::Available => "available",
        FlatpakInstallState::Stale => "stale",
        FlatpakInstallState::Unsigned => "unsigned",
        FlatpakInstallState::Unavailable => "unavailable",
    }
}

fn clean_node(node: &str) -> Option<&str> {
    let node = node.trim();
    if node.is_empty()
        || node.len() > MAX_NODE_ID_BYTES
        || matches!(node, "." | "..")
        || node
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front_door::peer_app_search_items;

    fn catalog() -> FlatpakAppCatalog {
        FlatpakAppCatalog {
            schema_version: mackes_mesh_types::app_catalog::FLATPAK_CATALOG_SCHEMA_VERSION,
            revision: "catalog-42".into(),
            entries: vec![mackes_mesh_types::app_catalog::FlatpakCatalogEntry {
                app_id: "org.example.Editor".into(),
                display_name: "Editor".into(),
                summary: "Guest editor".into(),
                icon_reference: "icon:editor".into(),
                source_revision: "flathub-42".into(),
                declared_capabilities: vec!["audio".into()],
                guest_profile: "wayland-standard".into(),
                supported_actions: vec!["launch".into()],
                provenance: mackes_mesh_types::app_catalog::FlatpakCatalogProvenance {
                    source: "curated".into(),
                    signature: Some("sig-42".into()),
                },
                state: FlatpakInstallState::Installed,
            }],
        }
    }

    #[test]
    fn validated_catalog_projects_into_launchable_front_door_row() {
        let reply = PeerAppsReply {
            ok: true,
            node: "oak".into(),
            entries: Vec::new(),
            catalog: Some(catalog()),
            error: None,
        };
        let (apps, note) = fold_peer_apps_reply("oak", reply);
        assert!(note.is_none());
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].source, "flatpak");
        assert_eq!(apps[0].state, "installed");
        assert_eq!(apps[0].health, "ready");
        assert_eq!(apps[0].catalog_revision.as_deref(), Some("catalog-42"));
        assert_eq!(apps[0].guest_profile.as_deref(), Some("wayland-standard"));
        assert_eq!(apps[0].requested_capabilities, vec!["audio"]);
    }

    #[test]
    fn catalog_reply_from_another_node_is_rejected_before_caching() {
        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "pine".into(),
                entries: Vec::new(),
                catalog: Some(catalog()),
                error: None,
            },
        );

        assert!(apps.is_empty());
        assert_eq!(
            note.as_deref(),
            Some("app discovery reply was for pine, not oak")
        );
    }

    #[test]
    fn unsafe_peer_identity_never_reaches_discovery_or_reply_cache_authority() {
        let dir = tempfile::tempdir().expect("temp bus");
        let root = dir.path().to_path_buf();
        let persist = Persist::open(root.clone()).expect("open bus");
        let mut state = FrontDoorPeerAppsState::new(Some(root));

        for unsafe_node in [
            "../oak",
            "oak/guest",
            "oak\nadmin",
            ".",
            "..",
            &"n".repeat(MAX_NODE_ID_BYTES + 1),
        ] {
            state.drive_for_focus(Some(unsafe_node));
        }

        assert!(state.pending_ulid().is_none());
        assert!(persist
            .list_since(PEER_APPS_ACTION, None)
            .expect("discovery requests")
            .is_empty());

        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "oak/guest".into(),
                entries: Vec::new(),
                catalog: Some(catalog()),
                error: None,
            },
        );
        assert!(apps.is_empty());
        assert_eq!(
            note.as_deref(),
            Some("oak app discovery reply omitted its node")
        );
    }

    #[test]
    fn legacy_reply_from_another_node_is_rejected_before_caching() {
        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "pine".into(),
                entries: vec![PeerAppEntry {
                    id: "org.example.Editor".into(),
                    name: "Editor".into(),
                    source: "flatpak".into(),
                    node: "pine".into(),
                    icon: String::new(),
                    health: "online".into(),
                    state: "installed".into(),
                }],
                catalog: None,
                error: None,
            },
        );

        assert!(apps.is_empty());
        assert_eq!(
            note.as_deref(),
            Some("app discovery reply was for pine, not oak")
        );
    }

    #[test]
    fn legacy_cross_peer_row_is_rejected_instead_of_becoming_a_launch_target() {
        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "oak".into(),
                entries: vec![
                    PeerAppEntry {
                        id: "org.example.Safe".into(),
                        name: "Safe".into(),
                        source: "flatpak".into(),
                        node: String::new(),
                        icon: String::new(),
                        health: "ready".into(),
                        state: "installed".into(),
                    },
                    PeerAppEntry {
                        id: "org.example.Substituted".into(),
                        name: "Substituted".into(),
                        source: "flatpak".into(),
                        node: "pine".into(),
                        icon: String::new(),
                        health: "ready".into(),
                        state: "installed".into(),
                    },
                ],
                catalog: None,
                error: None,
            },
        );

        assert!(note.is_none());
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "org.example.Safe");
        assert_eq!(apps[0].node, "oak");
        assert!(apps[0].flatpak_id().is_ok_and(|id| id.is_some()));
    }

    #[test]
    fn unsigned_catalog_row_is_preserved_but_never_launchable() {
        let mut catalog = catalog();
        catalog.entries[0].provenance.signature = None;
        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "oak".into(),
                entries: Vec::new(),
                catalog: Some(catalog),
                error: None,
            },
        );
        assert!(note.is_none());
        assert_eq!(apps[0].state, "unsigned");
        assert_eq!(apps[0].health, "unavailable");
        assert_eq!(peer_app_search_items(apps, 0).len(), 1);
    }

    #[test]
    fn catalog_preserves_not_installed_and_unavailable_rows_without_promoting_them() {
        let mut catalog = catalog();
        catalog
            .entries
            .push(mackes_mesh_types::app_catalog::FlatpakCatalogEntry {
                app_id: "org.example.NotInstalled".into(),
                display_name: "Not installed".into(),
                summary: "Guest app not installed".into(),
                icon_reference: "icon:not-installed".into(),
                source_revision: "flathub-42".into(),
                declared_capabilities: Vec::new(),
                guest_profile: "wayland-standard".into(),
                supported_actions: vec!["launch".into()],
                provenance: mackes_mesh_types::app_catalog::FlatpakCatalogProvenance {
                    source: "curated".into(),
                    signature: Some("sig-42".into()),
                },
                state: FlatpakInstallState::Available,
            });
        catalog
            .entries
            .push(mackes_mesh_types::app_catalog::FlatpakCatalogEntry {
                app_id: "org.example.Unavailable".into(),
                display_name: "Unavailable".into(),
                summary: "Guest app unavailable".into(),
                icon_reference: "icon:unavailable".into(),
                source_revision: "flathub-42".into(),
                declared_capabilities: Vec::new(),
                guest_profile: "wayland-standard".into(),
                supported_actions: vec!["launch".into()],
                provenance: mackes_mesh_types::app_catalog::FlatpakCatalogProvenance {
                    source: "curated".into(),
                    signature: Some("sig-42".into()),
                },
                state: FlatpakInstallState::Unavailable,
            });

        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "oak".into(),
                entries: Vec::new(),
                catalog: Some(catalog),
                error: None,
            },
        );

        assert!(note.is_none());
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[1].id, "org.example.NotInstalled");
        assert_eq!(apps[1].state, "available");
        assert_eq!(apps[1].health, "unavailable");
        assert_eq!(apps[2].id, "org.example.Unavailable");
        assert_eq!(apps[2].state, "unavailable");
        assert_eq!(apps[2].health, "unavailable");
        assert_eq!(peer_app_search_items(apps, 0).len(), 3);
    }

    #[test]
    fn installed_catalog_without_launch_action_is_unavailable() {
        let mut catalog = catalog();
        catalog.entries[0].supported_actions = vec!["resume".into()];
        let (apps, note) = fold_peer_apps_reply(
            "oak",
            PeerAppsReply {
                ok: true,
                node: "oak".into(),
                entries: Vec::new(),
                catalog: Some(catalog),
                error: None,
            },
        );

        assert!(note.is_none());
        assert_eq!(apps[0].id, "org.example.Editor");
        assert_eq!(apps[0].state, "unavailable");
        assert_eq!(apps[0].health, "unavailable");
        assert_eq!(peer_app_search_items(apps, 0).len(), 1);
    }

    #[test]
    fn peer_apps_publish_peer_list_and_fold_reply_into_front_door_rows() {
        let dir = tempfile::tempdir().expect("temp bus");
        let root = dir.path().to_path_buf();
        let mut state = FrontDoorPeerAppsState::new(Some(root.clone()));

        state.drive_for_focus(Some("oak"));

        let persist = Persist::open(root.clone()).expect("open bus");
        let requests = persist
            .list_since(PEER_APPS_ACTION, None)
            .expect("requests");
        assert_eq!(requests.len(), 1);
        let request_body: serde_json::Value =
            serde_json::from_str(requests[0].body.as_deref().expect("request body"))
                .expect("request json");
        assert_eq!(request_body["node"], "oak");

        let ulid = state.pending_ulid().expect("pending request").to_owned();
        let reply = json!({
            "ok": true,
            "node": "oak",
            "entries": [
                {
                    "id": "org.mozilla.Firefox.desktop",
                    "name": "Firefox",
                    "source": "flatpak",
                    "icon": "firefox",
                    "health": "online",
                    "state": "installed"
                },
                {
                    "id": "",
                    "name": "bad",
                    "source": "xdg"
                },
                {
                    "id": "org.example.Bad/Path",
                    "name": "bad flatpak",
                    "source": "flatpak"
                }
            ]
        })
        .to_string();
        persist
            .write(&reply_topic(&ulid), Priority::Default, None, Some(&reply))
            .expect("write reply");

        state.drive_for_focus(Some("oak"));

        let apps = state.items();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].node, "oak");
        assert_eq!(apps[0].id, "org.mozilla.Firefox.desktop");
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].source, "flatpak");
        assert!(state.pending_ulid().is_none());
    }

    #[test]
    fn peer_apps_missing_bus_degrades_to_empty_cached_rows() {
        let mut state = FrontDoorPeerAppsState::new(None);

        state.drive_for_focus(Some("oak"));

        assert!(state.items().is_empty());
        assert!(state.pending_ulid().is_none());
        assert_eq!(
            state.last_note.as_deref(),
            Some("reconnecting to oak for app discovery (retry 1/2)")
        );
    }

    #[test]
    fn timed_out_peer_apps_retry_with_backoff_and_keep_the_last_projection() {
        let dir = tempfile::tempdir().expect("temp bus");
        let root = dir.path().to_path_buf();
        let mut state = FrontDoorPeerAppsState::new(Some(root.clone()));
        state.cache.insert(
            "oak".into(),
            PeerAppsCache {
                apps: vec![FrontDoorPeerApp {
                    id: "org.example.Editor".into(),
                    name: "Editor".into(),
                    node: "oak".into(),
                    source: "flatpak".into(),
                    icon: String::new(),
                    health: "ready".into(),
                    state: "installed".into(),
                    catalog_revision: Some("catalog-42".into()),
                    guest_profile: Some("wayland-standard".into()),
                    requested_capabilities: vec!["audio".into()],
                }],
                refreshed: Instant::now() - CACHE_REFRESH,
                reconnecting: false,
            },
        );

        state.drive_for_focus(Some("oak"));
        state.pending.as_mut().expect("initial request").sent = Instant::now() - REQUEST_TIMEOUT;
        state.drive_for_focus(Some("oak"));

        assert!(state.pending.is_none());
        assert_eq!(state.items()[0].id, "org.example.Editor");
        assert_eq!(state.items()[0].state, "reconnecting");
        assert_eq!(state.items()[0].health, "reconnecting");
        assert_eq!(state.items()[0].requested_capabilities, vec!["audio"]);
        assert_eq!(
            state.last_note.as_deref(),
            Some("reconnecting to oak for app discovery (retry 1/2)")
        );

        state.retry_not_before = Some(Instant::now() - RETRY_BACKOFF);
        state.drive_for_focus(Some("oak"));
        assert!(state.pending.is_some(), "backoff should permit a retry");
        assert_eq!(state.items()[0].requested_capabilities, vec!["audio"]);
    }

    #[test]
    fn failed_peer_reply_preserves_permissions_and_enters_reconnecting_state() {
        let dir = tempfile::tempdir().expect("temp bus");
        let root = dir.path().to_path_buf();
        let mut state = FrontDoorPeerAppsState::new(Some(root.clone()));
        state.cache.insert(
            "oak".into(),
            PeerAppsCache {
                apps: vec![FrontDoorPeerApp {
                    id: "org.example.Editor".into(),
                    name: "Editor".into(),
                    node: "oak".into(),
                    source: "flatpak".into(),
                    icon: "icon:editor".into(),
                    health: "ready".into(),
                    state: "installed".into(),
                    catalog_revision: Some("catalog-42".into()),
                    guest_profile: Some("wayland-standard".into()),
                    requested_capabilities: vec!["audio".into(), "clipboard".into()],
                }],
                refreshed: Instant::now() - CACHE_REFRESH,
                reconnecting: false,
            },
        );

        state.drive_for_focus(Some("oak"));
        let ulid = state.pending_ulid().expect("discovery request").to_owned();
        Persist::open(root)
            .expect("open bus")
            .write(
                &reply_topic(&ulid),
                Priority::Default,
                None,
                Some(
                    &json!({
                        "ok": false,
                        "node": "oak",
                        "error": "peer is temporarily unavailable"
                    })
                    .to_string(),
                ),
            )
            .expect("write failed reply");

        state.drive_for_focus(Some("oak"));

        let app = &state.items()[0];
        assert_eq!(app.state, "reconnecting");
        assert_eq!(app.health, "reconnecting");
        assert_eq!(app.catalog_revision.as_deref(), Some("catalog-42"));
        assert_eq!(app.guest_profile.as_deref(), Some("wayland-standard"));
        assert_eq!(app.requested_capabilities, vec!["audio", "clipboard"]);
        assert_eq!(
            state.last_note.as_deref(),
            Some("peer is temporarily unavailable")
        );
        assert_eq!(state.retry_count, 1);
        assert!(state.retry_not_before.is_some());
    }

    #[test]
    fn successful_reconnect_clears_retry_state_and_refreshes_rows() {
        let dir = tempfile::tempdir().expect("temp bus");
        let root = dir.path().to_path_buf();
        let mut state = FrontDoorPeerAppsState::new(Some(root.clone()));

        state.drive_for_focus(Some("oak"));
        state.pending.as_mut().expect("initial request").sent = Instant::now() - REQUEST_TIMEOUT;
        state.drive_for_focus(Some("oak"));
        state.retry_not_before = Some(Instant::now() - RETRY_BACKOFF);
        state.drive_for_focus(Some("oak"));
        let ulid = state.pending_ulid().expect("retry request").to_owned();

        let persist = Persist::open(root).expect("open bus");
        let reply = json!({
            "ok": true,
            "node": "oak",
            "catalog": serde_json::to_value(catalog()).expect("catalog json")
        })
        .to_string();
        persist
            .write(&reply_topic(&ulid), Priority::Default, None, Some(&reply))
            .expect("write reply");

        state.drive_for_focus(Some("oak"));

        assert_eq!(state.items().len(), 1);
        assert_eq!(state.items()[0].id, "org.example.Editor");
        assert_eq!(state.retry_count, 0);
        assert!(state.retry_not_before.is_none());
        assert!(state.last_note.is_none());
    }
}
