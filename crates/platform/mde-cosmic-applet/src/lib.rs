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
    /// Open the MDE-Notification-Hub center (NOTIFY-7).
    OpenNotifications,
}

/// The Bus topic (action) a quick action publishes, if any. `None`
/// for actions that only deep-link into a Workbench surface.
#[must_use]
pub fn action_bus_topic(action: QuickAction) -> Option<&'static str> {
    match action {
        QuickAction::ToggleDnd => Some("action/dnd/toggle"),
        QuickAction::OpenPeers
        | QuickAction::OpenTransfers
        | QuickAction::OpenRegistration
        | QuickAction::OpenNotifications => None,
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
        // Notifications launches its own layer-shell binary, not a Workbench
        // panel — see launch_argv.
        QuickAction::ToggleDnd | QuickAction::OpenNotifications => None,
    }
}

/// The launch argv for a deep-link action (what the libcosmic shell
/// spawns). `None` for pure Bus actions.
#[must_use]
pub fn launch_argv(action: QuickAction) -> Option<Vec<String>> {
    // The notification center is its own layer-shell binary (NOTIFY-7), not a
    // Workbench deep-link.
    if matches!(action, QuickAction::OpenNotifications) {
        return Some(vec!["mde-notify-center".to_string()]);
    }
    action_deep_link(action).map(|slug| {
        vec![
            "mde-workbench".to_string(),
            "--focus".to_string(),
            slug.to_string(),
        ]
    })
}

// ───────────────────────── APPS-2 — launcher model ─────────────────────────
//
// The Applications Panel launcher (docs/design/apps-launcher.md). The mackesd
// `apps_aggregator` (APPS-1) serves the unified entry list on `action/apps/list`;
// this render-agnostic layer parses that reply + filters it per tab/search so the
// libcosmic dropdown shell stays thin. The applet must NOT depend on mackesd
// (§6 boundary) — it parses the wire JSON into this local shape.

/// One launchable entry, as decoded from the `action/apps/list` reply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Entry {
    /// Stable id (desktop-file id for local apps, else `kind:node:name`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// `app` | `mesh-app` | `workload` | `service`.
    pub kind: String,
    /// `xdg` | `flatpak` | `peer` | `podman` | `libvirt` | `service`.
    #[serde(default)]
    pub source: String,
    /// Owning node hostname — empty for local.
    #[serde(default)]
    pub node: String,
    /// Local exec line (apps) — empty otherwise.
    #[serde(default)]
    pub exec: String,
    /// Service/remote endpoint — empty otherwise.
    #[serde(default)]
    pub endpoint: String,
    /// Icon name.
    #[serde(default)]
    pub icon: String,
    /// Mesh presence/health tier — empty for local.
    #[serde(default)]
    pub health: String,
    /// Workload run state — empty otherwise.
    #[serde(default)]
    pub state: String,
}

/// The launcher's tabs (Q7). `Favorites` is the landing tab (Q6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherTab {
    /// Pinned favorites (any kind), the default view.
    Favorites,
    /// Local XDG + Flatpak apps.
    Apps,
    /// Mesh peers' apps (remote-desktop targets).
    Mesh,
    /// Containers + VMs.
    Workloads,
    /// Published mesh services.
    Services,
}

impl LauncherTab {
    /// All tabs, in display order (Favorites first — Q6).
    #[must_use]
    pub fn all() -> [LauncherTab; 5] {
        [
            LauncherTab::Favorites,
            LauncherTab::Apps,
            LauncherTab::Mesh,
            LauncherTab::Workloads,
            LauncherTab::Services,
        ]
    }

    /// Tab label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            LauncherTab::Favorites => "Favorites",
            LauncherTab::Apps => "Apps",
            LauncherTab::Mesh => "Mesh",
            LauncherTab::Workloads => "Workloads",
            LauncherTab::Services => "Services",
        }
    }

    /// The entry `kind` this tab shows (None for Favorites, which is by-pin).
    #[must_use]
    pub fn kind(self) -> Option<&'static str> {
        match self {
            LauncherTab::Favorites => None,
            LauncherTab::Apps => Some("app"),
            LauncherTab::Mesh => Some("mesh-app"),
            LauncherTab::Workloads => Some("workload"),
            LauncherTab::Services => Some("service"),
        }
    }
}

/// Parse the `action/apps/list` reply body into entries. An error envelope or
/// malformed body yields an empty list (the dropdown shows an empty state).
#[must_use]
pub fn parse_entries(reply: &str) -> Vec<Entry> {
    serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|v| {
            v.get("entries")
                .and_then(|e| serde_json::from_value::<Vec<Entry>>(e.clone()).ok())
        })
        .unwrap_or_default()
}

/// Filter entries for the dropdown: a non-empty `query` searches across ALL tabs
/// (Q2 fuzzy-ish — case-insensitive substring on the name); an empty query shows
/// the active `tab` (Favorites = ids in `favorites`, else by kind). Sorted by name.
#[must_use]
pub fn filter_entries<'a>(
    entries: &'a [Entry],
    tab: LauncherTab,
    query: &str,
    favorites: &std::collections::HashSet<String>,
) -> Vec<&'a Entry> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            if !q.is_empty() {
                return e.name.to_lowercase().contains(&q);
            }
            match tab.kind() {
                Some(k) => e.kind == k,
                None => favorites.contains(&e.id), // Favorites tab
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// APPS-3 — human-readable bytes for the launcher header's QNM-Shared usage
/// (Q8). Binary units (KiB/MiB/GiB/TiB), one decimal above MiB.
#[must_use]
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u <= 1 {
        format!("{} {}", v.round() as u64, UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The header disk label, e.g. `"QNM-Shared: 1.2 GiB / 200.0 GiB"`. `None`
/// total → an "unavailable" label (mount not up).
#[must_use]
pub fn qnm_usage_label(usage: Option<(u64, u64)>) -> String {
    match usage {
        Some((used, total)) if total > 0 => {
            format!("QNM-Shared: {} / {}", fmt_bytes(used), fmt_bytes(total))
        }
        _ => "QNM-Shared: unavailable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_scales_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2 KiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn qnm_label_handles_present_and_absent() {
        assert_eq!(
            qnm_usage_label(Some((1024 * 1024 * 1024, 200 * 1024 * 1024 * 1024))),
            "QNM-Shared: 1.0 GiB / 200.0 GiB"
        );
        assert_eq!(qnm_usage_label(None), "QNM-Shared: unavailable");
        assert_eq!(qnm_usage_label(Some((5, 0))), "QNM-Shared: unavailable");
    }

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

    fn sample_reply() -> String {
        r#"{"ok":true,"entries":[
            {"id":"firefox","name":"Firefox","kind":"app","source":"xdg","exec":"firefox %u"},
            {"id":"gimp","name":"GIMP","kind":"app","source":"flatpak","exec":"gimp"},
            {"id":"mesh-app:peerB","name":"peerB (remote desktop)","kind":"mesh-app","source":"peer","node":"peerB","health":"online"},
            {"id":"workload:podman:c1","name":"nginx","kind":"workload","source":"podman","state":"running"},
            {"id":"service:peerB:Jellyfin","name":"Jellyfin","kind":"service","source":"service","endpoint":"http://10.42.0.2:8096"}
        ]}"#.to_string()
    }

    #[test]
    fn parse_entries_decodes_and_tolerates_garbage() {
        let e = parse_entries(&sample_reply());
        assert_eq!(e.len(), 5);
        assert_eq!(e[0].name, "Firefox");
        assert_eq!(e[0].exec, "firefox %u");
        // An error envelope / junk → empty, never a panic.
        assert!(parse_entries(r#"{"ok":false,"error":"x"}"#).is_empty());
        assert!(parse_entries("not json").is_empty());
    }

    #[test]
    fn filter_entries_by_tab_search_and_favorites() {
        let entries = parse_entries(&sample_reply());
        let none = std::collections::HashSet::new();
        // Apps tab → the two local apps, sorted (Firefox, GIMP).
        let apps = filter_entries(&entries, LauncherTab::Apps, "", &none);
        assert_eq!(
            apps.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Firefox", "GIMP"]
        );
        // Mesh / Workloads / Services each isolate their kind.
        assert_eq!(
            filter_entries(&entries, LauncherTab::Mesh, "", &none).len(),
            1
        );
        assert_eq!(
            filter_entries(&entries, LauncherTab::Workloads, "", &none).len(),
            1
        );
        assert_eq!(
            filter_entries(&entries, LauncherTab::Services, "", &none).len(),
            1
        );
        // Favorites tab honors the pin set across kinds.
        let mut favs = std::collections::HashSet::new();
        favs.insert("gimp".to_string());
        favs.insert("service:peerB:Jellyfin".to_string());
        let f = filter_entries(&entries, LauncherTab::Favorites, "", &favs);
        assert_eq!(f.len(), 2);
        // A non-empty query searches ACROSS tabs (Q2).
        let s = filter_entries(&entries, LauncherTab::Apps, "jelly", &none);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "Jellyfin");
    }

    #[test]
    fn launcher_tabs_favorites_first_and_kinds_map() {
        assert_eq!(LauncherTab::all()[0], LauncherTab::Favorites);
        assert_eq!(LauncherTab::Apps.kind(), Some("app"));
        assert_eq!(LauncherTab::Favorites.kind(), None);
        assert_eq!(LauncherTab::Services.label(), "Services");
    }
}
