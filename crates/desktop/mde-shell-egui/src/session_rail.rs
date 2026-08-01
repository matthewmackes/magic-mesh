//! NAVBAR-U3 — local rail projection of the broker's public VDI session log.
//!
//! The authoritative shared session directory still lives behind the broker's
//! integration-gated `SessionStore`. Until that lands, the shell can still read the
//! same public Bus wire the broker drains (`action/vdi/session`) and render this
//! seat's non-closed sessions as compact rail entries. It deserialises the
//! shared [`mackes_mesh_types::vdi_session::SessionRequest`] (arch-2) off the JSON
//! boundary — a lightweight shared-types dependency, never a dependency on
//! `mackesd`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mackes_mesh_types::vdi_session::{AppVmLifecycleState, SessionRequest};
// arch-11: prod now opens via the BusReader seam; only the tests still name
// `Persist` (through `use super::*`), so the import is test-only.
#[cfg(test)]
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

use crate::surfaces::SessionRailEntry;

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
    app_id: Option<String>,
    app_state: Option<AppVmLifecycleState>,
    app_reason: Option<String>,
}

/// The app-specific half of a focused rail session. The shell consumes this
/// once and hands it to the existing VDI broker path; it never invents a
/// second app-launch transport or re-derives a session id from UI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppSessionHandoff {
    pub(crate) id: String,
    pub(crate) serving_peer: String,
    pub(crate) vm_id: String,
    pub(crate) app_id: String,
}

// The `SessionRequest` verbs read off `action/vdi/session` are the shared
// `mackes_mesh_types::vdi_session::SessionRequest` (arch-2) — imported above, not a
// local mirror. Only `Deserialize` is exercised here (this side reads the wire).

/// Shell-side projection of local VDI sessions for the bottom rail.
#[derive(Debug, Default)]
pub(crate) struct SessionRailState {
    bus_root: Option<PathBuf>,
    cursor: Option<String>,
    sessions: BTreeMap<String, RailSession>,
    pending_app_handoff: Option<AppSessionHandoff>,
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
                let entry = if app_session_can_focus(s) {
                    SessionRailEntry::with_session_id(&s.id, session_label(s), session_badge(s))
                } else {
                    // Keep lifecycle/recovery information visible, but omit the
                    // focus target while the app cannot honestly be opened.
                    SessionRailEntry::new(session_label(s), session_badge(s))
                };
                entry.with_app_status(
                    s.app_reason.clone(),
                    s.app_state.and_then(app_retry_guidance),
                )
            })
            .collect()
    }

    /// Focus a broker-visible session locally. This mirrors the broker lifecycle
    /// state for the shell's session selection without publishing a fake broker
    /// `Active` transition; the shared `SessionStore` remains the live multi-seat
    /// authority when it lands.
    pub(crate) fn focus_session(&mut self, id: &str) -> bool {
        self.poll();
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        if !app_session_can_focus(session) {
            return false;
        }
        if matches!(
            session.state,
            SessionState::Requested | SessionState::Active | SessionState::Disconnected
        ) {
            session.state = SessionState::Active;
            if let Some(app_id) = session.app_id.clone() {
                self.pending_app_handoff = Some(AppSessionHandoff {
                    id: session.id.clone(),
                    serving_peer: session.serving_peer.clone(),
                    vm_id: session.vm_id.clone(),
                    app_id,
                });
            }
            true
        } else {
            false
        }
    }

    /// Consume the one-shot app handoff raised by focusing an App VM session.
    /// A focus action is not allowed to retrigger a launch on every frame.
    pub(crate) fn take_app_handoff(&mut self) -> Option<AppSessionHandoff> {
        self.pending_app_handoff.take()
    }

    fn poll(&mut self) {
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
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
                        app_id: None,
                        app_state: None,
                        app_reason: None,
                    },
                );
            }
            SessionRequest::OpenApp {
                id,
                serving_peer,
                vm_id,
                client_peer,
                app_id,
                ..
            } => {
                self.sessions.insert(
                    id.clone(),
                    RailSession {
                        id,
                        serving_peer,
                        vm_id,
                        client_peer,
                        state: SessionState::Requested,
                        app_id: Some(app_id),
                        app_state: Some(AppVmLifecycleState::WaitingForPlacement),
                        app_reason: None,
                    },
                );
            }
            SessionRequest::Active { id } => self.set_state(&id, SessionState::Active),
            SessionRequest::AppState {
                id,
                generation: _,
                state,
                reason,
                ..
            } => {
                self.set_app_state(&id, state, reason)
            }
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

    fn set_app_state(&mut self, id: &str, state: AppVmLifecycleState, reason: Option<String>) {
        if let Some(session) = self.sessions.get_mut(id) {
            if session.app_id.is_some() {
                session.app_state = Some(state);
                session.app_reason = bound_app_reason(reason);
            }
        }
    }
}

const fn app_session_can_focus(session: &RailSession) -> bool {
    match session.app_state {
        None => true,
        Some(AppVmLifecycleState::Connected | AppVmLifecycleState::Paused) => true,
        Some(
            AppVmLifecycleState::Installing
            | AppVmLifecycleState::WaitingForPlacement
            | AppVmLifecycleState::StartingGuest
            | AppVmLifecycleState::StartingApp
            | AppVmLifecycleState::Reconnecting
            | AppVmLifecycleState::Unavailable
            | AppVmLifecycleState::Denied
            | AppVmLifecycleState::StaleCatalog
            | AppVmLifecycleState::Failed,
        ) => false,
    }
}

fn bound_app_reason(reason: Option<String>) -> Option<String> {
    const MAX_CHARS: usize = 255;
    let reason = reason?;
    if reason.chars().any(char::is_control) {
        return None;
    }
    let mut bounded: String = reason.chars().take(MAX_CHARS).collect();
    if reason.chars().count() > MAX_CHARS {
        bounded.push_str("...");
    }
    Some(bounded)
}

const fn app_retry_guidance(state: AppVmLifecycleState) -> Option<&'static str> {
    match state {
        AppVmLifecycleState::Installing => Some("Installing guest application"),
        AppVmLifecycleState::WaitingForPlacement => Some("Waiting for placement"),
        AppVmLifecycleState::StartingGuest => Some("Starting guest"),
        AppVmLifecycleState::StartingApp => Some("Starting application"),
        AppVmLifecycleState::Paused => Some("Resume from Desktop"),
        AppVmLifecycleState::Reconnecting => Some("Waiting for connection"),
        AppVmLifecycleState::Unavailable | AppVmLifecycleState::Failed => {
            Some("Retry from Desktop")
        }
        AppVmLifecycleState::Denied => Some("Launch denied by policy"),
        AppVmLifecycleState::StaleCatalog => Some("Refresh catalog"),
        AppVmLifecycleState::Connected => Some("Open application"),
    }
}

fn session_label(session: &RailSession) -> String {
    if let Some(app_id) = &session.app_id {
        return format!("{} · {}", app_id, session.serving_peer);
    }
    if session.vm_id.is_empty() {
        session.serving_peer.clone()
    } else {
        format!("{} {}", session.serving_peer, session.vm_id)
    }
}

const fn session_badge(session: &RailSession) -> &'static str {
    if let Some(state) = session.app_state {
        return match state {
            AppVmLifecycleState::Installing => "INSTALL",
            AppVmLifecycleState::WaitingForPlacement => "PLACE",
            AppVmLifecycleState::StartingGuest => "BOOT",
            AppVmLifecycleState::StartingApp => "START",
            AppVmLifecycleState::Connected => "LIVE",
            AppVmLifecycleState::Paused => "PAUSE",
            AppVmLifecycleState::Reconnecting => "RECON",
            AppVmLifecycleState::Unavailable => "OFFLINE",
            AppVmLifecycleState::Denied => "DENIED",
            AppVmLifecycleState::StaleCatalog => "STALE",
            AppVmLifecycleState::Failed => "FAILED",
        };
    }
    match session.state {
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

    #[test]
    fn app_vm_readiness_updates_are_visible_without_faking_transport_state() {
        let root = temp_bus("app-state");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":["audio"],"resume":true}"#,
        );
        publish(
            &root,
            r#"{"op":"app_state","id":"app-session-1","state":"starting_guest","reason":"guest boot accepted"}"#,
        );

        let mut state = SessionRailState::with_bus_root(root.clone());
        assert_eq!(
            state.entries("eagle"),
            vec![SessionRailEntry::new("org.example.Writer · oak", "BOOT").with_app_status(
                Some("guest boot accepted".to_owned()),
                Some("Starting guest"),
            )]
        );
        publish(
            &root,
            r#"{"op":"app_state","id":"app-session-1","state":"connected","reason":"surface ready"}"#,
        );
        assert_eq!(state.entries("eagle")[0].protocol(), "LIVE");
        assert_eq!(state.entries("eagle")[0].reason(), Some("surface ready"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_states_show_guidance_without_claiming_transport_recovery() {
        let root = temp_bus("app-recovery-guidance");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":[],"resume":true}"#,
        );
        let mut state = SessionRailState::with_bus_root(root.clone());

        for (wire_state, badge, reason, guidance) in [
            ("failed", "FAILED", "flatpak install failed", "Retry from Desktop"),
            ("unavailable", "OFFLINE", "guest is unreachable", "Retry from Desktop"),
            ("reconnecting", "RECON", "surface connection dropped", "Waiting for connection"),
        ] {
            publish(
                &root,
                &format!(
                    "{{\"op\":\"app_state\",\"id\":\"app-session-1\",\"state\":\"{wire_state}\",\"reason\":\"{reason}\"}}"
                ),
            );
            let entry = &state.entries("eagle")[0];
            assert_eq!(entry.protocol(), badge);
            assert_eq!(entry.reason(), Some(reason));
            assert_eq!(entry.retry_guidance(), Some(guidance));
            assert_eq!(
                entry.session_id(),
                None,
                "{wire_state} must not be presented as a focusable App VM"
            );
            assert!(
                !state.focus_session("app-session-1"),
                "{wire_state} must not emit an app launch handoff"
            );
            assert!(state.take_app_handoff().is_none());
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_non_transport_app_state_has_honest_next_step_guidance() {
        let root = temp_bus("app-lifecycle-guidance");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":[],"resume":true}"#,
        );
        let mut state = SessionRailState::with_bus_root(root.clone());
        for (wire_state, guidance) in [
            ("installing", "Installing guest application"),
            ("waiting_for_placement", "Waiting for placement"),
            ("starting_guest", "Starting guest"),
            ("starting_app", "Starting application"),
            ("paused", "Resume from Desktop"),
            ("denied", "Launch denied by policy"),
            ("stale_catalog", "Refresh catalog"),
            ("connected", "Open application"),
        ] {
            publish(
                &root,
                &format!(
                    "{{\"op\":\"app_state\",\"id\":\"app-session-1\",\"state\":\"{wire_state}\"}}"
                ),
            );
            assert_eq!(state.entries("eagle")[0].retry_guidance(), Some(guidance));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn app_state_reason_is_bounded_and_control_free_at_shell_boundary() {
        let root = temp_bus("app-reason-bound");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":[],"resume":true}"#,
        );
        publish(
            &root,
            &format!(
                "{{\"op\":\"app_state\",\"id\":\"app-session-1\",\"state\":\"failed\",\"reason\":\"{}\"}}",
                "x".repeat(300)
            ),
        );
        let mut state = SessionRailState::with_bus_root(root.clone());
        let entries = state.entries("eagle");
        let reason = entries[0].reason().expect("bounded reason");
        assert_eq!(reason.chars().count(), 258);
        assert!(reason.ends_with("..."));

        publish(
            &root,
            "{\"op\":\"app_state\",\"id\":\"app-session-1\",\"state\":\"failed\",\"reason\":\"bad\\nreason\"}",
        );
        assert!(state.entries("eagle")[0].reason().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn focusing_an_app_vm_emits_one_typed_vdi_handoff() {
        let root = temp_bus("app-focus");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":["audio"],"resume":true}"#,
        );
        publish(
            &root,
            r#"{"op":"app_state","id":"app-session-1","state":"connected"}"#,
        );

        let mut state = SessionRailState::with_bus_root(root.clone());
        assert!(state.focus_session("app-session-1"));
        assert_eq!(
            state.take_app_handoff(),
            Some(AppSessionHandoff {
                id: "app-session-1".to_owned(),
                serving_peer: "oak".to_owned(),
                vm_id: "appvm-writer".to_owned(),
                app_id: "org.example.Writer".to_owned(),
            })
        );
        assert!(
            state.take_app_handoff().is_none(),
            "focus is consumed once; it must not retrigger a VDI launch"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn paused_app_vm_is_presented_as_resumable_and_emits_the_existing_handoff() {
        let root = temp_bus("app-resume");
        publish(
            &root,
            r#"{"op":"open_app","id":"app-session-1","serving_peer":"oak","vm_id":"appvm-writer","client_peer":"eagle","app_id":"org.example.Writer","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":[],"resume":true}"#,
        );
        publish(
            &root,
            r#"{"op":"app_state","id":"app-session-1","state":"paused","reason":"guest suspended"}"#,
        );

        let mut state = SessionRailState::with_bus_root(root.clone());
        let entries = state.entries("eagle");
        assert_eq!(entries[0].protocol(), "PAUSE");
        assert_eq!(entries[0].retry_guidance(), Some("Resume from Desktop"));
        assert_eq!(entries[0].session_id(), Some("app-session-1"));
        assert!(state.focus_session("app-session-1"));
        assert!(state.take_app_handoff().is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
