//! GUI-6 (Q46/47) — the MCNF cosmic-applet's logic layer.
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

/// Parse the `favorites`/`set-favorite` reply (`{"favorites":[id,…]}`) into a
/// pin set (APPS-4). An error/garbage reply → empty set.
#[must_use]
pub fn parse_favorites(reply: &str) -> std::collections::HashSet<String> {
    serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|v| {
            v.get("favorites")
                .and_then(|f| serde_json::from_value::<Vec<String>>(f.clone()).ok())
        })
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// A workload control action (APPS-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadAction {
    /// Start a stopped VM/container.
    Start,
    /// Stop a running VM/container.
    Stop,
    /// Attach to the VM console / container shell (run in a terminal).
    Attach,
}

/// True when a libvirt/podman state string reads as actively running (so the row
/// shows Stop rather than Start).
#[must_use]
pub fn workload_running(state: &str) -> bool {
    matches!(state.trim().to_lowercase().as_str(), "running" | "up")
        || state.to_lowercase().starts_with("up ")
}

/// APPS-LIVE-1 — true when a local app entry has been stamped live by the
/// aggregator (`state=="running"`, with the host(s) in `node`). The launcher
/// badges these with a running dot + "running on <host>".
#[must_use]
pub fn app_running(e: &Entry) -> bool {
    e.kind == "app" && e.state.eq_ignore_ascii_case("running")
}

/// APPS-LIVE-1/2 — the hosts a running app entry is live on (the aggregator
/// comma-joins them into `node`). Empty when the entry isn't running.
#[must_use]
pub fn app_running_hosts(e: &Entry) -> Vec<String> {
    if !app_running(e) {
        return Vec::new();
    }
    e.node
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// APPS-LIVE-2 — true when a running app is live on THIS node (`this_host`), so a
/// click should raise its window locally rather than relaunch. Case-insensitive
/// host match against the comma-joined host list.
#[must_use]
pub fn app_running_local(e: &Entry, this_host: &str) -> bool {
    !this_host.is_empty()
        && app_running_hosts(e)
            .iter()
            .any(|h| h.eq_ignore_ascii_case(this_host))
}

/// The argv to control a local workload (APPS-6). VMs go through `virsh`
/// (`qemu:///system`, the same connection the Workbench compute panel uses);
/// containers through `podman`. `Attach` returns the inner console/shell argv
/// (the caller wraps it in a terminal). `None` for an unknown source/empty name.
#[must_use]
pub fn workload_argv(source: &str, name: &str, action: WorkloadAction) -> Option<Vec<String>> {
    if name.is_empty() {
        return None;
    }
    let v = |parts: &[&str]| parts.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    match (source, action) {
        ("libvirt", WorkloadAction::Start) => {
            Some(v(&["virsh", "-c", "qemu:///system", "start", name]))
        }
        ("libvirt", WorkloadAction::Stop) => {
            Some(v(&["virsh", "-c", "qemu:///system", "shutdown", name]))
        }
        ("libvirt", WorkloadAction::Attach) => {
            Some(v(&["virsh", "-c", "qemu:///system", "console", name]))
        }
        ("podman", WorkloadAction::Start) => Some(v(&["podman", "start", name])),
        ("podman", WorkloadAction::Stop) => Some(v(&["podman", "stop", name])),
        ("podman", WorkloadAction::Attach) => Some(v(&["podman", "exec", "-it", name, "/bin/sh"])),
        _ => None,
    }
}

/// Relevance score for a launcher search match (lower = better): `0` exact,
/// `1` name-prefix, `2` word-prefix (a word in the name starts with the query).
/// Returns `None` for a mid-word substring — so "files" matches "Files" but NOT
/// "Color Profiles" (the Start-Menu search bug fixed 2026-06-24: a bare
/// `contains` surfaced "Pro**files**" and buried the real Files app).
#[must_use]
fn name_match_score(name_lc: &str, q: &str) -> Option<u32> {
    if name_lc == q {
        return Some(0);
    }
    if name_lc.starts_with(q) {
        return Some(1);
    }
    if name_lc
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(q))
    {
        return Some(2);
    }
    None
}

/// Filter entries for the dropdown: a non-empty `query` searches across ALL tabs
/// by **relevance** (exact > name-prefix > word-prefix; mid-word substrings are
/// NOT matches), ranked best-first then alphabetically. An empty query shows the
/// active `tab` (Favorites = ids in `favorites`, else by kind), sorted by name.
#[must_use]
pub fn filter_entries<'a>(
    entries: &'a [Entry],
    tab: LauncherTab,
    query: &str,
    favorites: &std::collections::HashSet<String>,
) -> Vec<&'a Entry> {
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        let mut scored: Vec<(u32, &Entry)> = entries
            .iter()
            .filter_map(|e| name_match_score(&e.name.to_lowercase(), &q).map(|s| (s, e)))
            .collect();
        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        });
        return scored.into_iter().map(|(_, e)| e).collect();
    }
    let mut out: Vec<&Entry> = entries
        .iter()
        .filter(|e| match tab.kind() {
            Some(k) => e.kind == k,
            None => favorites.contains(&e.id), // Favorites tab
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

/// The header disk label, e.g. `"Mesh Sync: 1.2 GiB / 200.0 GiB"`. `None`
/// total → an "unavailable" label (share not up). SUBSTRATE-12 renamed the
/// user-facing share from "QNM-Shared" to "Mesh Sync"; the `/mnt/mesh-storage`
/// path + the fn name stay (back-compat).
#[must_use]
pub fn qnm_usage_label(usage: Option<(u64, u64)>) -> String {
    match usage {
        Some((used, total)) if total > 0 => {
            format!("Mesh Sync: {} / {}", fmt_bytes(used), fmt_bytes(total))
        }
        _ => "Mesh Sync: unavailable".to_string(),
    }
}

// ──────────────────── LIGHTHOUSE-7 — panel health indicator ────────────────
//
// A worst-of green/red lighthouse-health indicator for the Cosmic panel applet
// (LIGHTHOUSE-7's "applet" surface): a single dot that is green only when every
// lighthouse is up, red the moment any one is degraded/offline, and absent when
// the snapshot names no lighthouses. Clicking it deep-links into the Workbench
// Lighthouses tab (the same tab the Hub footer + LIGHTHOUSE-4 deep-link reach).
//
// The applet runs as the desktop user and cannot read the root-only replicated
// peer directory, so — exactly like the NEB-CRYPTO-LABEL cipher text — the data
// comes from the **world-readable** mesh-status snapshot (`/run/mde/
// mesh-status.json`), written by the root snapshot timer. This render-agnostic
// layer parses that JSON; the libcosmic shell only renders the result + spawns
// the deep-link. Lighthouse identification mirrors the Workbench
// `enrich_roles` (LIGHTHOUSE-9): a node is a lighthouse when its `role` is
// `"lighthouse"` OR its `overlay_ip` is in `network.lighthouse_ips`.

/// The Workbench focus slug that opens the Lighthouses tab (Mesh group). The Hub
/// footer presses `mesh.lighthouses:<host>` for a specific lighthouse; the
/// applet indicator is fleet-wide (worst-of), so it opens the tab with no
/// per-host focus suffix.
pub const LIGHTHOUSE_FOCUS_SLUG: &str = "mesh.lighthouses";

/// The `role` value that marks a lighthouse node — the canonical constant
/// (`mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE`) the Workbench tab + mackesd
/// also use, imported (not duplicated) so the applet's snapshot-based indicator
/// can't silently diverge from the rest of the fleet if the string ever changes.
use mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE;

/// The worst-of lighthouse-health the panel indicator renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LighthouseHealth {
    /// One or more lighthouses, and EVERY one is online → green.
    AllHealthy,
    /// One or more lighthouses but at least one is degraded/offline → red.
    Degraded,
    /// The snapshot names no lighthouses (or is absent/unparseable) → the
    /// indicator is hidden (honest empty state, never a fake green/red dot).
    None,
}

impl LighthouseHealth {
    /// The `mde-theme` token the libcosmic shell maps to the dot color
    /// (`beacon_healthy` green / `danger` red — the dedicated lighthouse-beacon
    /// hues, Q13/Q14). `None` has no dot, so no token.
    #[must_use]
    pub fn token(self) -> Option<&'static str> {
        match self {
            LighthouseHealth::AllHealthy => Some("beacon_healthy"),
            LighthouseHealth::Degraded => Some("danger"),
            LighthouseHealth::None => Option::None,
        }
    }

    /// The panel tooltip (`(healthy, total)` of the lighthouse set). `None` →
    /// no indicator, so no tooltip.
    #[must_use]
    pub fn tooltip(self, healthy: usize, total: usize) -> Option<String> {
        match self {
            LighthouseHealth::AllHealthy => {
                Some(format!("Lighthouses: {healthy}/{total} healthy — all up"))
            }
            LighthouseHealth::Degraded => Some(format!(
                "Lighthouses: {healthy}/{total} healthy — open the Lighthouses tab"
            )),
            LighthouseHealth::None => Option::None,
        }
    }
}

/// `(health, healthy, total)` of the lighthouse set, parsed from the mesh-status
/// snapshot JSON. A node is a lighthouse when its `role == "lighthouse"` OR its
/// `overlay_ip` is in `network.lighthouse_ips` (LIGHTHOUSE-9). A lighthouse is
/// counted healthy iff its `presence == "online"` (the snapshot already folds
/// the directory health tier → presence: `healthy→online`, `degraded→idle`,
/// else `offline`). The indicator is worst-of: green only when `healthy ==
/// total` (and `total > 0`), red otherwise; `None` when no lighthouses exist.
/// A missing/garbage snapshot yields `(None, 0, 0)` — never a panic.
#[must_use]
pub fn lighthouse_health_from_snapshot(snapshot: &str) -> (LighthouseHealth, usize, usize) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return (LighthouseHealth::None, 0, 0);
    };
    // The lighthouse overlay-IP set (LIGHTHOUSE-9) — empty when absent.
    let lighthouse_ips: Vec<&str> = v
        .get("network")
        .and_then(|n| n.get("lighthouse_ips"))
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let Some(nodes) = v.get("nodes").and_then(serde_json::Value::as_array) else {
        return (LighthouseHealth::None, 0, 0);
    };
    let mut total = 0usize;
    let mut healthy = 0usize;
    for node in nodes {
        let role = node.get("role").and_then(serde_json::Value::as_str);
        let overlay_ip = node
            .get("overlay_ip")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let is_lighthouse = role == Some(LIGHTHOUSE_ROLE)
            || (!overlay_ip.is_empty() && lighthouse_ips.contains(&overlay_ip));
        if !is_lighthouse {
            continue;
        }
        total += 1;
        if node.get("presence").and_then(serde_json::Value::as_str) == Some("online") {
            healthy += 1;
        }
    }
    let health = if total == 0 {
        LighthouseHealth::None
    } else if healthy == total {
        LighthouseHealth::AllHealthy
    } else {
        LighthouseHealth::Degraded
    };
    (health, healthy, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workload_argv_builds_virsh_and_podman() {
        assert_eq!(
            workload_argv("libvirt", "win10", WorkloadAction::Start).unwrap(),
            ["virsh", "-c", "qemu:///system", "start", "win10"]
        );
        assert_eq!(
            workload_argv("libvirt", "win10", WorkloadAction::Stop).unwrap(),
            ["virsh", "-c", "qemu:///system", "shutdown", "win10"]
        );
        assert_eq!(
            workload_argv("podman", "nginx", WorkloadAction::Start).unwrap(),
            ["podman", "start", "nginx"]
        );
        assert_eq!(
            workload_argv("podman", "nginx", WorkloadAction::Attach).unwrap(),
            ["podman", "exec", "-it", "nginx", "/bin/sh"]
        );
        assert!(workload_argv("xdg", "x", WorkloadAction::Start).is_none());
        assert!(workload_argv("podman", "", WorkloadAction::Start).is_none());
    }

    #[test]
    fn workload_running_reads_state() {
        assert!(workload_running("running"));
        assert!(workload_running("Up 3 minutes"));
        assert!(!workload_running("shut off"));
        assert!(!workload_running("exited"));
    }

    #[test]
    fn app_running_helpers_read_state_and_hosts() {
        // A local app stamped running on two hosts (one of them this node).
        let running = parse_entries(
            r#"{"entries":[{"id":"firefox","name":"Firefox","kind":"app","source":"xdg","exec":"firefox %u","state":"running","node":"fedora, node-13"}]}"#,
        );
        let ff = &running[0];
        assert!(app_running(ff));
        assert_eq!(app_running_hosts(ff), vec!["fedora", "node-13"]);
        // Live here → raise; live only on a peer → not local.
        assert!(app_running_local(ff, "fedora"));
        assert!(app_running_local(ff, "FEDORA")); // case-insensitive
        assert!(!app_running_local(ff, "other-node"));
        assert!(!app_running_local(ff, "")); // unknown this-host

        // A non-running app: no badge, no hosts, never local.
        let idle = parse_entries(
            r#"{"entries":[{"id":"gimp","name":"GIMP","kind":"app","source":"flatpak","exec":"gimp"}]}"#,
        );
        let g = &idle[0];
        assert!(!app_running(g));
        assert!(app_running_hosts(g).is_empty());
        assert!(!app_running_local(g, "fedora"));

        // app_running is app-kind only (a workload's "running" state isn't an app).
        let wl = parse_entries(
            r#"{"entries":[{"id":"workload:podman:c1","name":"nginx","kind":"workload","source":"podman","state":"running"}]}"#,
        );
        assert!(!app_running(&wl[0]));
    }

    #[test]
    fn parse_favorites_decodes_set() {
        let f = parse_favorites(r#"{"ok":true,"favorites":["firefox","gimp"]}"#);
        assert!(f.contains("firefox") && f.contains("gimp") && f.len() == 2);
        assert!(parse_favorites(r#"{"ok":false}"#).is_empty());
        assert!(parse_favorites("junk").is_empty());
    }

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
            "Mesh Sync: 1.0 GiB / 200.0 GiB"
        );
        assert_eq!(qnm_usage_label(None), "Mesh Sync: unavailable");
        assert_eq!(qnm_usage_label(Some((5, 0))), "Mesh Sync: unavailable");
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
    fn search_surfaces_real_match_not_midword_substring() {
        // Start-Menu bug (2026-06-24): "files" surfaced "Color Profiles"
        // (Pro-FILES) and buried the real Files app. Word-boundary relevance.
        let reply = r#"{"ok":true,"entries":[
            {"id":"cp","name":"Color Profiles","kind":"app","source":"xdg","exec":"x"},
            {"id":"files","name":"Files","kind":"app","source":"xdg","exec":"mde-files"}
        ]}"#;
        let entries = parse_entries(reply);
        let none = std::collections::HashSet::new();
        let r: Vec<&str> = filter_entries(&entries, LauncherTab::Apps, "files", &none)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(r, ["Files"], "files -> Files only; Color Profiles must not match");
    }

    #[test]
    fn launcher_tabs_favorites_first_and_kinds_map() {
        assert_eq!(LauncherTab::all()[0], LauncherTab::Favorites);
        assert_eq!(LauncherTab::Apps.kind(), Some("app"));
        assert_eq!(LauncherTab::Favorites.kind(), None);
        assert_eq!(LauncherTab::Services.label(), "Services");
    }

    // ─────────────────── LIGHTHOUSE-7 — panel health indicator ──────────────

    /// A snapshot with one lighthouse (by role) + one (by overlay-IP membership)
    /// + one ordinary node, each at a chosen presence.
    fn lh_snapshot(lh_role: &str, lh_ip: &str, peer: &str) -> String {
        format!(
            r#"{{"nodes":[
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"{lh_role}","role":"lighthouse"}},
                {{"hostname":"lh-02","overlay_ip":"10.42.0.2","presence":"{lh_ip}","role":"server"}},
                {{"hostname":"ws-1","overlay_ip":"10.42.0.50","presence":"{peer}","role":"workstation"}}
            ],"network":{{"lighthouse_ips":["10.42.0.1","10.42.0.2"]}}}}"#
        )
    }

    #[test]
    fn lighthouse_all_healthy_is_green() {
        // Both lighthouses online (one by role, one by overlay-IP membership);
        // the workstation's state is irrelevant.
        let (h, healthy, total) =
            lighthouse_health_from_snapshot(&lh_snapshot("online", "online", "offline"));
        assert_eq!(h, LighthouseHealth::AllHealthy);
        assert_eq!((healthy, total), (2, 2));
        assert_eq!(h.token(), Some("beacon_healthy"));
    }

    #[test]
    fn lighthouse_any_down_is_red() {
        // The role lighthouse is online but the overlay-IP lighthouse is idle →
        // worst-of red.
        let (h, healthy, total) =
            lighthouse_health_from_snapshot(&lh_snapshot("online", "idle", "online"));
        assert_eq!(h, LighthouseHealth::Degraded);
        assert_eq!((healthy, total), (1, 2));
        assert_eq!(h.token(), Some("danger"));
        // An offline lighthouse is just as red.
        let (h2, _, _) =
            lighthouse_health_from_snapshot(&lh_snapshot("offline", "online", "online"));
        assert_eq!(h2, LighthouseHealth::Degraded);
    }

    #[test]
    fn no_lighthouses_means_no_indicator() {
        // A snapshot with nodes but none that are lighthouses → hidden, no token.
        let none = r#"{"nodes":[{"hostname":"ws-1","overlay_ip":"10.42.0.50","presence":"online","role":"workstation"}],"network":{"lighthouse_ips":[]}}"#;
        let (h, healthy, total) = lighthouse_health_from_snapshot(none);
        assert_eq!(h, LighthouseHealth::None);
        assert_eq!((healthy, total), (0, 0));
        assert_eq!(h.token(), None);
        assert_eq!(h.tooltip(0, 0), None);
    }

    #[test]
    fn lighthouse_role_identifies_even_without_ip_membership() {
        // role=="lighthouse" counts even when network.lighthouse_ips is absent
        // (older snapshots before LIGHTHOUSE-9 wrote the IP set).
        let by_role = r#"{"nodes":[{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online","role":"lighthouse"}],"network":{}}"#;
        let (h, healthy, total) = lighthouse_health_from_snapshot(by_role);
        assert_eq!(h, LighthouseHealth::AllHealthy);
        assert_eq!((healthy, total), (1, 1));
    }

    #[test]
    fn missing_or_garbage_snapshot_is_none_not_a_panic() {
        assert_eq!(
            lighthouse_health_from_snapshot("not json").0,
            LighthouseHealth::None
        );
        assert_eq!(
            lighthouse_health_from_snapshot("{}").0,
            LighthouseHealth::None
        );
        assert_eq!(
            lighthouse_health_from_snapshot(r#"{"nodes":[]}"#).0,
            LighthouseHealth::None
        );
    }

    #[test]
    fn lighthouse_tooltips_present_only_when_an_indicator_shows() {
        assert_eq!(
            LighthouseHealth::AllHealthy.tooltip(3, 3),
            Some("Lighthouses: 3/3 healthy — all up".to_string())
        );
        assert_eq!(
            LighthouseHealth::Degraded.tooltip(1, 3),
            Some("Lighthouses: 1/3 healthy — open the Lighthouses tab".to_string())
        );
        assert_eq!(LighthouseHealth::None.tooltip(0, 0), None);
    }

    #[test]
    fn lighthouse_focus_slug_matches_the_workbench_tab() {
        // Must match the Mesh-group "lighthouses" panel slug the Workbench
        // resolves (model.rs::view_from_focus_slug) — the Hub uses the same
        // base slug (LIGHTHOUSE-4).
        assert_eq!(LIGHTHOUSE_FOCUS_SLUG, "mesh.lighthouses");
    }
}
