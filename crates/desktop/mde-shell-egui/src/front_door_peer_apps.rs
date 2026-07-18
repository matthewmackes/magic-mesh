//! Front Door peer-app discovery client.
//!
//! The daemon owns `action/apps/peer-list`; the shell mirrors only the JSON it
//! needs and never depends on the daemon crate. Requests are non-blocking Bus
//! RPCs, and replies are cached per peer so focusing a mesh node can reveal its
//! installed apps without polling the network from the render path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct PeerAppsReply {
    ok: bool,
    node: String,
    entries: Vec<PeerAppEntry>,
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
            }
        }
        if self.pending.is_some() || !self.cache_stale(node, now) {
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
            .map(|cache| cache.apps.clone())
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
            self.cache.insert(
                pending.node.clone(),
                PeerAppsCache {
                    apps,
                    refreshed: now,
                },
            );
            self.last_note = note;
            self.pending = None;
        } else if now.duration_since(pending.sent) >= REQUEST_TIMEOUT {
            self.cache.insert(
                pending.node.clone(),
                PeerAppsCache {
                    apps: Vec::new(),
                    refreshed: now,
                },
            );
            self.last_note = Some(format!("{} did not answer app discovery", pending.node));
            self.pending = None;
        }
    }

    fn publish_request_for(&mut self, node: &str, now: Instant) {
        let body = json!({ "node": node }).to_string();
        let Some(persist) = self.persist() else {
            self.cache.insert(
                node.to_owned(),
                PeerAppsCache {
                    apps: Vec::new(),
                    refreshed: now,
                },
            );
            self.last_note = Some("the local mesh Bus is unavailable".to_owned());
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
                self.cache.insert(
                    node.to_owned(),
                    PeerAppsCache {
                        apps: Vec::new(),
                        refreshed: now,
                    },
                );
                self.last_note = Some(format!("could not ask {node} for apps: {err}"));
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
    reply: PeerAppsReply,
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
    let reply_node = clean_node(&reply.node).unwrap_or(requested_node);
    let apps = reply
        .entries
        .into_iter()
        .filter_map(|entry| {
            let node = clean_node(&entry.node).unwrap_or(reply_node);
            let id = entry.id.trim();
            let name = entry.name.trim();
            if node.is_empty() || id.is_empty() || name.is_empty() {
                return None;
            }
            Some(FrontDoorPeerApp {
                id: id.to_owned(),
                name: name.to_owned(),
                node: node.to_owned(),
                source: entry.source,
                icon: entry.icon,
                health: entry.health,
                state: entry.state,
            })
        })
        .collect();
    (apps, None)
}

fn clean_node(node: &str) -> Option<&str> {
    let node = node.trim();
    (!node.is_empty()).then_some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Some("the local mesh Bus is unavailable")
        );
    }
}
