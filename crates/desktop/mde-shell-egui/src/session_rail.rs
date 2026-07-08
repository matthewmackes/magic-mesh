//! NAVBAR-U3 — local rail projection of the broker's public VDI session log.
//!
//! The authoritative shared session directory still lives behind the broker's
//! integration-gated `SessionStore`. Until that lands, the shell can still read the
//! same public Bus wire the broker drains (`action/vdi/session`) and render this
//! seat's non-closed sessions as taskbar-style rail entries. This is a mirror of
//! the JSON boundary only, not a dependency on `mackesd`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mde_bus::persist::Persist;
use serde::Deserialize;

use crate::dock::SessionRailEntry;

const ACTION_TOPIC: &str = "action/vdi/session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Requested,
    Active,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RailSession {
    id: String,
    serving_peer: String,
    vm_id: String,
    client_peer: String,
    state: SessionState,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum SessionRequest {
    Open {
        id: String,
        serving_peer: String,
        vm_id: String,
        client_peer: String,
    },
    Active {
        id: String,
    },
    Disconnect {
        id: String,
    },
    Close {
        id: String,
    },
}

/// Shell-side projection of local VDI sessions for the bottom rail.
#[derive(Debug, Default)]
pub(crate) struct SessionRailState {
    bus_root: Option<PathBuf>,
    cursor: Option<String>,
    sessions: BTreeMap<String, RailSession>,
}

impl SessionRailState {
    pub(crate) fn new() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bus_root(bus_root: PathBuf) -> Self {
        Self {
            bus_root: Some(bus_root),
            ..Self::default()
        }
    }

    /// Fold newly published broker requests and return this client's visible rail
    /// entries. Closed sessions disappear; requested/active/disconnected sessions
    /// stay visible so reconnect remains discoverable.
    pub(crate) fn entries(&mut self, client_peer: &str) -> Vec<SessionRailEntry> {
        self.poll();
        self.sessions
            .values()
            .filter(|s| s.client_peer == client_peer)
            .filter(|s| {
                matches!(
                    s.state,
                    SessionState::Requested | SessionState::Active | SessionState::Disconnected
                )
            })
            .map(|s| {
                SessionRailEntry::with_session_id(&s.id, session_label(s), session_badge(s.state))
            })
            .collect()
    }

    /// Focus a broker-visible session locally. This mirrors the broker lifecycle
    /// state for the shell's taskbar selection without publishing a fake broker
    /// `Active` transition; the shared `SessionStore` remains the live multi-seat
    /// authority when it lands.
    pub(crate) fn focus_session(&mut self, id: &str) -> bool {
        self.poll();
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        if matches!(
            session.state,
            SessionState::Requested | SessionState::Active | SessionState::Disconnected
        ) {
            session.state = SessionState::Active;
            true
        } else {
            false
        }
    }

    fn poll(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(msgs) = persist.list_since(ACTION_TOPIC, self.cursor.as_deref()) else {
            return;
        };
        for msg in msgs {
            self.cursor = Some(msg.ulid);
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            if let Ok(request) = serde_json::from_str::<SessionRequest>(body) {
                self.apply(request);
            }
        }
    }

    fn apply(&mut self, request: SessionRequest) {
        match request {
            SessionRequest::Open {
                id,
                serving_peer,
                vm_id,
                client_peer,
            } => {
                self.sessions.insert(
                    id.clone(),
                    RailSession {
                        id,
                        serving_peer,
                        vm_id,
                        client_peer,
                        state: SessionState::Requested,
                    },
                );
            }
            SessionRequest::Active { id } => self.set_state(&id, SessionState::Active),
            SessionRequest::Disconnect { id } => self.set_state(&id, SessionState::Disconnected),
            SessionRequest::Close { id } => {
                self.sessions.remove(&id);
            }
        }
    }

    fn set_state(&mut self, id: &str, state: SessionState) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.state = state;
        }
    }
}

fn session_label(session: &RailSession) -> String {
    if session.vm_id.is_empty() {
        session.serving_peer.clone()
    } else {
        format!("{} {}", session.serving_peer, session.vm_id)
    }
}

const fn session_badge(state: SessionState) -> &'static str {
    match state {
        SessionState::Requested => "VDI",
        SessionState::Active => "LIVE",
        SessionState::Disconnected => "DISC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;

    fn temp_bus(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("mde-session-rail-{tag}-{n}"));
        std::fs::create_dir_all(&root).expect("mkroot");
        root
    }

    fn publish(root: &PathBuf, body: &str) {
        Persist::open(root.clone())
            .expect("open bus")
            .write(ACTION_TOPIC, Priority::Default, None, Some(body))
            .expect("write session action");
    }

    #[test]
    fn broker_session_actions_fold_into_local_rail_entries() {
        let root = temp_bus("fold");
        publish(
            &root,
            r#"{"op":"open","id":"s1","serving_peer":"oak","vm_id":"win11","client_peer":"eagle"}"#,
        );
        publish(
            &root,
            r#"{"op":"open","id":"s2","serving_peer":"ash","vm_id":"build","client_peer":"other"}"#,
        );

        let mut state = SessionRailState::with_bus_root(root.clone());
        let entries = state.entries("eagle");
        assert_eq!(
            entries,
            vec![SessionRailEntry::with_session_id("s1", "oak win11", "VDI")]
        );

        publish(&root, r#"{"op":"active","id":"s1"}"#);
        let entries = state.entries("eagle");
        assert_eq!(
            entries,
            vec![SessionRailEntry::with_session_id("s1", "oak win11", "LIVE")]
        );

        publish(&root, r#"{"op":"close","id":"s1"}"#);
        assert!(state.entries("eagle").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn focused_session_entry_marks_the_local_rail_entry_live() {
        let root = temp_bus("focus");
        publish(
            &root,
            r#"{"op":"open","id":"s1","serving_peer":"oak","vm_id":"win11","client_peer":"eagle"}"#,
        );

        let mut state = SessionRailState::with_bus_root(root.clone());
        assert_eq!(
            state.entries("eagle"),
            vec![SessionRailEntry::with_session_id("s1", "oak win11", "VDI")]
        );
        assert!(state.focus_session("s1"));
        assert_eq!(
            state.entries("eagle"),
            vec![SessionRailEntry::with_session_id("s1", "oak win11", "LIVE")]
        );
        assert!(
            !state.focus_session("missing"),
            "unknown session ids do not fabricate rail entries"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
