//! GUI-6 (Q46/47) — the Magic Mesh cosmic-applet's logic layer.
//!
//! The applet is a libcosmic panel widget that subscribes to
//! `mde-bus`, shows a mesh-health pip, offers quick actions
//! (join/leave, DnD, transfers), and deep-links into the Workbench.
//! libcosmic only renders inside a live Cosmic session, so this
//! module is the **render-agnostic, fully-tested core** — pip-state
//! derivation, the quick-action → Bus-verb table, and the Workbench
//! deep-link URIs. The libcosmic `applet::run` shell that draws this
//! state into the Cosmic panel is the hardware-gated render target
//! (it needs a Cosmic session to build + verify); it consumes this
//! crate so its surface is thin glue, not logic.

/// Mesh health, as the pip renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pip {
    /// mackesd reachable + every probed peer healthy.
    Healthy,
    /// mackesd reachable but a peer is degraded/critical.
    Degraded,
    /// mackesd unreachable — the mesh service is down or unenrolled.
    Down,
}

impl Pip {
    /// Carbon semantic token name the libcosmic shell maps to a color.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Pip::Healthy => "success",
            Pip::Degraded => "warning",
            Pip::Down => "danger",
        }
    }

    /// One-line tooltip.
    #[must_use]
    pub fn tooltip(self) -> &'static str {
        match self {
            Pip::Healthy => "Mesh healthy",
            Pip::Degraded => "Mesh degraded — a peer needs attention",
            Pip::Down => "Mesh service down",
        }
    }
}

/// Derive the pip from the `action/mesh/directory` reply (PD-1) — the
/// same record the Front Door reads, so the applet and the panel
/// never disagree. `None`/unparseable/not-ok ⇒ Down.
#[must_use]
pub fn pip_from_directory(reply: &str) -> Pip {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply.trim()) else {
        return Pip::Down;
    };
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Pip::Down;
    }
    let peers = v.get("peers").and_then(|p| p.as_array());
    let Some(peers) = peers else {
        return Pip::Down;
    };
    // Any peer critical/degraded/unreachable ⇒ degraded pip.
    let any_unhealthy = peers.iter().any(|p| {
        !matches!(
            p.get("health").and_then(|h| h.as_str()),
            Some("healthy") | None
        )
    });
    if any_unhealthy {
        Pip::Degraded
    } else {
        Pip::Healthy
    }
}

/// A quick action the applet offers in its popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    /// Toggle Do-Not-Disturb (notifications).
    ToggleDnd,
    /// Open the Workbench Peers Front Door.
    OpenPeers,
    /// Open the Workbench Files transfers view.
    OpenTransfers,
    /// Open the Registration panel to join/leave the mesh.
    OpenRegistration,
}

/// The Bus topic (action) a quick action publishes, if any. `None`
/// for actions that only deep-link into a Workbench surface.
#[must_use]
pub fn action_bus_topic(action: QuickAction) -> Option<&'static str> {
    match action {
        QuickAction::ToggleDnd => Some("action/dnd/toggle"),
        QuickAction::OpenPeers | QuickAction::OpenTransfers | QuickAction::OpenRegistration => None,
    }
}

/// The Workbench deep-link an action opens, if any — the
/// `<group>.<panel>` focus slug passed as `mde-workbench --focus`.
#[must_use]
pub fn action_deep_link(action: QuickAction) -> Option<&'static str> {
    match action {
        // PLANES-1 — Peers is its own Front Door plane; Registration
        // re-homed to the This Node plane (slug "node").
        QuickAction::OpenPeers => Some("peers"),
        QuickAction::OpenTransfers => Some("files.transfers"),
        QuickAction::OpenRegistration => Some("node.registration"),
        QuickAction::ToggleDnd => None,
    }
}

/// The launch argv for a deep-link action (what the libcosmic shell
/// spawns). `None` for pure Bus actions.
#[must_use]
pub fn launch_argv(action: QuickAction) -> Option<Vec<String>> {
    action_deep_link(action).map(|slug| {
        vec![
            "mde-workbench".to_string(),
            "--focus".to_string(),
            slug.to_string(),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pip_reads_the_directory_record() {
        let healthy = r#"{"ok":true,"peers":[{"health":"healthy"},{"health":"healthy"}]}"#;
        assert_eq!(pip_from_directory(healthy), Pip::Healthy);
        let degraded = r#"{"ok":true,"peers":[{"health":"healthy"},{"health":"degraded"}]}"#;
        assert_eq!(pip_from_directory(degraded), Pip::Degraded);
        let critical = r#"{"ok":true,"peers":[{"health":"critical"}]}"#;
        assert_eq!(pip_from_directory(critical), Pip::Degraded);
    }

    #[test]
    fn pip_is_down_on_unreachable_or_garbage() {
        assert_eq!(pip_from_directory(r#"{"ok":false}"#), Pip::Down);
        assert_eq!(pip_from_directory("not json"), Pip::Down);
        assert_eq!(pip_from_directory(r#"{"ok":true}"#), Pip::Down);
    }

    #[test]
    fn empty_mesh_is_healthy_not_degraded() {
        assert_eq!(
            pip_from_directory(r#"{"ok":true,"peers":[]}"#),
            Pip::Healthy
        );
    }

    #[test]
    fn pip_tokens_are_carbon_semantic_names() {
        assert_eq!(Pip::Healthy.token(), "success");
        assert_eq!(Pip::Degraded.token(), "warning");
        assert_eq!(Pip::Down.token(), "danger");
    }

    #[test]
    fn quick_actions_map_to_bus_or_deep_link_never_both() {
        for action in [
            QuickAction::ToggleDnd,
            QuickAction::OpenPeers,
            QuickAction::OpenTransfers,
            QuickAction::OpenRegistration,
        ] {
            let bus = action_bus_topic(action).is_some();
            let link = action_deep_link(action).is_some();
            assert!(
                bus ^ link,
                "{action:?} must be exactly one of bus/deep-link"
            );
        }
    }

    #[test]
    fn deep_links_launch_the_workbench_at_the_right_slug() {
        let argv = launch_argv(QuickAction::OpenPeers).unwrap();
        assert_eq!(argv, ["mde-workbench", "--focus", "peers"]);
        assert!(launch_argv(QuickAction::ToggleDnd).is_none());
    }
}
