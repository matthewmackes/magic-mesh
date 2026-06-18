//! APPS-1 — the `apps_aggregator`: one Bus verb (`action/apps/list`) that returns
//! the unified launchable-entry list for the Applications Panel (the Magic-Mesh
//! launcher that replaces Cosmic's app library — `docs/design/apps-launcher.md`).
//!
//! Per Q24 the applet is a **thin renderer**: this root-daemon responder builds
//! the single source of truth by aggregating, each open (refresh-on-open, cached):
//!   * **local apps** — XDG `.desktop` (all dirs) + Flatpak exports (Q5);
//!   * **mesh apps** — one remote-desktop launch target per joined peer, plus any
//!     apps a peer advertises in its PD-2 directory descriptor (Q17);
//!   * **workloads** — local podman containers + libvirt VMs from the compute
//!     inventory (Q19);
//!   * **services** — published mesh services from the PD-2 descriptors (Q20).
//!
//! Every entry is tagged `kind` / `source` / `node` / `health` so the applet can
//! file it under the right tab without a per-tab query. The pure builders
//! (`parse_desktop_entry`, `scan_local_apps`, `mesh_entries_from_directory`,
//! `workload_entries_from_inventory`, `build_list`) are unit-tested; the responder
//! is the thin Bus shell around them.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// One launchable entry in the unified list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Stable id — the `.desktop` file id for local apps, else `kind:node:name`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `app` | `mesh-app` | `workload` | `service` — selects the applet tab.
    pub kind: String,
    /// `xdg` | `flatpak` | `peer` | `podman` | `libvirt` | `service`.
    pub source: String,
    /// Owning node hostname — empty for the local box.
    #[serde(default)]
    pub node: String,
    /// Local exec line (apps only) — empty for mesh/workload/service.
    #[serde(default)]
    pub exec: String,
    /// Service / remote endpoint (services + mesh targets) — empty otherwise.
    #[serde(default)]
    pub endpoint: String,
    /// XDG icon name (or app fallback handled by the applet).
    #[serde(default)]
    pub icon: String,
    /// Presence/health tier for mesh entries (`online`/`stale`/…) — empty local.
    #[serde(default)]
    pub health: String,
    /// Workload run state (`running`/`exited`/`shut off`) — empty otherwise.
    #[serde(default)]
    pub state: String,
}

// ───────────────────────── local apps (XDG + Flatpak) ─────────────────────────

/// Parse one `.desktop` file body into an [`AppEntry`]. `None` when it isn't a
/// launchable application (wrong `Type`, `NoDisplay=true`, `Hidden=true`, or no
/// `Name`/`Exec`). `id` is the caller-supplied desktop-file id; `flatpak` tags
/// the source. Only the `[Desktop Entry]` group is read.
#[must_use]
pub fn parse_desktop_entry(id: &str, body: &str, flatpak: bool) -> Option<AppEntry> {
    let mut in_entry = false;
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut typ = String::new();
    let mut no_display = false;
    let mut hidden = false;
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // Ignore locale-qualified keys (`Name[de]`) — take the unqualified value.
        match k.trim() {
            "Name" => name = v.trim().to_string(),
            "Exec" => exec = v.trim().to_string(),
            "Icon" => icon = v.trim().to_string(),
            "Type" => typ = v.trim().to_string(),
            "NoDisplay" => no_display = v.trim().eq_ignore_ascii_case("true"),
            "Hidden" => hidden = v.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    if typ != "Application" || no_display || hidden || name.is_empty() || exec.is_empty() {
        return None;
    }
    Some(AppEntry {
        id: id.to_string(),
        name,
        kind: "app".into(),
        source: if flatpak {
            "flatpak".into()
        } else {
            "xdg".into()
        },
        node: String::new(),
        exec,
        endpoint: String::new(),
        icon,
        health: String::new(),
        state: String::new(),
    })
}

/// The standard local application directories, in XDG precedence order (earlier
/// wins on a duplicate desktop-file id). Flatpak export dirs are included so
/// Flatpak apps surface without a separate `flatpak list` shell-out.
#[must_use]
pub fn default_app_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/share/applications"),
        home.join(".local/share/flatpak/exports/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ]
}

/// Scan `dirs` for `*.desktop` application entries, de-duplicated by desktop-file
/// id (first occurrence — i.e. XDG precedence — wins), sorted by name. A path
/// under a `flatpak/` tree is tagged `source=flatpak`.
#[must_use]
pub fn scan_local_apps(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut by_id: HashMap<String, AppEntry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for dir in dirs {
        let flatpak = dir.to_string_lossy().contains("flatpak");
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if by_id.contains_key(id) {
                continue; // precedence: earlier dir already provided this id
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(entry) = parse_desktop_entry(id, &body, flatpak) {
                order.push(id.to_string());
                by_id.insert(id.to_string(), entry);
            }
        }
    }
    let mut out: Vec<AppEntry> = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

// ───────────────────────── mesh apps + services (PD-2) ─────────────────────────

/// Map the PD-2 directory document (`directory::build_directory`) into mesh
/// entries: one `mesh-app` remote-desktop target per joined peer (skipping
/// `this_node`), plus a `service` entry for each service a peer advertises in its
/// `descriptors.services` array. Tolerant of a missing/empty descriptor — an
/// absent advertisement is just fewer entries, never an error.
#[must_use]
pub fn mesh_entries_from_directory(dir: &serde_json::Value, this_node: &str) -> Vec<AppEntry> {
    let mut out = Vec::new();
    let Some(peers) = dir.get("peers").and_then(|p| p.as_array()) else {
        return out;
    };
    for peer in peers {
        let host = peer
            .get("hostname")
            .or_else(|| peer.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if host.is_empty() || host == this_node {
            continue;
        }
        let health = peer
            .get("presence")
            .or_else(|| peer.get("health"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let overlay = peer
            .get("overlay_ip")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // One remote-desktop launch target per peer (Q9).
        out.push(AppEntry {
            id: format!("mesh-app:{host}"),
            name: format!("{host} (remote desktop)"),
            kind: "mesh-app".into(),
            source: "peer".into(),
            node: host.to_string(),
            exec: String::new(),
            endpoint: overlay.to_string(),
            icon: "computer".into(),
            health: health.clone(),
            state: String::new(),
        });
        // Any services the peer advertises in its descriptor (Q20).
        if let Some(svcs) = peer
            .get("descriptors")
            .and_then(|d| d.get("services"))
            .and_then(|s| s.as_array())
        {
            for svc in svcs {
                let sname = svc.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if sname.is_empty() {
                    continue;
                }
                let endpoint = svc
                    .get("endpoint")
                    .or_else(|| svc.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(AppEntry {
                    id: format!("service:{host}:{sname}"),
                    name: sname.to_string(),
                    kind: "service".into(),
                    source: "service".into(),
                    node: host.to_string(),
                    exec: String::new(),
                    endpoint,
                    icon: svc
                        .get("icon")
                        .and_then(|v| v.as_str())
                        .unwrap_or("network-server")
                        .to_string(),
                    health: health.clone(),
                    state: String::new(),
                });
            }
        }
    }
    out
}

// ───────────────────────── workloads (compute inventory) ─────────────────────────

/// Map a compute inventory document (`{"vms":[…],"containers":[…]}`) into
/// `workload` entries (Q19). Tolerant of missing arrays.
#[must_use]
pub fn workload_entries_from_inventory(inv: &serde_json::Value, node: &str) -> Vec<AppEntry> {
    let mut out = Vec::new();
    let mk = |v: &serde_json::Value, source: &str, icon: &str| -> Option<AppEntry> {
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if name.is_empty() {
            return None;
        }
        Some(AppEntry {
            id: format!("workload:{source}:{id}"),
            name: name.to_string(),
            kind: "workload".into(),
            source: source.to_string(),
            node: node.to_string(),
            exec: String::new(),
            endpoint: String::new(),
            icon: icon.to_string(),
            health: String::new(),
            state: v
                .get("state")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        })
    };
    if let Some(vms) = inv.get("vms").and_then(|v| v.as_array()) {
        out.extend(vms.iter().filter_map(|v| mk(v, "libvirt", "computer")));
    }
    if let Some(cts) = inv.get("containers").and_then(|v| v.as_array()) {
        out.extend(
            cts.iter()
                .filter_map(|v| mk(v, "podman", "application-x-executable")),
        );
    }
    out
}

// ───────────────────────── assembly + responder ─────────────────────────

/// Merge all sources into the `action/apps/list` reply, with per-kind counts.
#[must_use]
pub fn build_list(local: Vec<AppEntry>, mesh: Vec<AppEntry>, workloads: Vec<AppEntry>) -> String {
    let mut entries = local;
    entries.extend(mesh);
    entries.extend(workloads);
    let count = |k: &str| entries.iter().filter(|e| e.kind == k).count();
    json!({
        "ok": true,
        "entries": entries,
        "counts": {
            "app": count("app"),
            "mesh-app": count("mesh-app"),
            "workload": count("workload"),
            "service": count("service"),
            "total": entries.len(),
        }
    })
    .to_string()
}

// ───────────────────────── favorites (APPS-4) ─────────────────────────

/// Sanitize a username into a safe single filename component.
fn sanitize_user(user: &str) -> String {
    let s: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "_".to_string()
    } else {
        s
    }
}

/// Per-user favorites file on QNM-Shared (Q10): a JSON array of entry ids at
/// `<workgroup_root>/apps-favorites/<user>.json`, so a user's pins follow them to
/// any node. mackesd (root) is the only writer with mount access; the applet
/// reads/sets via the bus verbs.
#[must_use]
pub fn favorites_path(workgroup_root: &Path, user: &str) -> PathBuf {
    workgroup_root
        .join("apps-favorites")
        .join(format!("{}.json", sanitize_user(user)))
}

/// Read a user's pinned entry ids (empty when none/absent — never an error).
#[must_use]
pub fn read_favorites(workgroup_root: &Path, user: &str) -> Vec<String> {
    std::fs::read_to_string(favorites_path(workgroup_root, user))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Pin (`pinned=true`) or unpin an entry id for a user; returns the new set.
/// Writes via temp+rename so a concurrent reader never sees a half-written file.
pub fn set_favorite(workgroup_root: &Path, user: &str, id: &str, pinned: bool) -> Vec<String> {
    let mut favs = read_favorites(workgroup_root, user);
    favs.retain(|f| f != id);
    if pinned {
        favs.push(id.to_string());
    }
    let path = favorites_path(workgroup_root, user);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(&favs) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    favs
}

/// Extract `user` from a request body (`{"user":"…"}`); `_` when absent.
fn user_from_body(body: Option<&str>) -> String {
    body.and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
        .and_then(|v| v.get("user").and_then(|u| u.as_str()).map(str::to_string))
        .unwrap_or_else(|| "_".to_string())
}

/// Responder handle — the QNM-Shared root (peers/descriptors live under it) +
/// this node's hostname (so its own peer row isn't listed as a mesh target).
#[derive(Debug, Clone)]
pub struct AppsService {
    workgroup_root: PathBuf,
    node_id: String,
    home: PathBuf,
}

impl AppsService {
    /// New service. `home` is the desktop user's home (local app scan root).
    #[must_use]
    pub fn new(workgroup_root: &Path, node_id: &str, home: &Path) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
            node_id: node_id.to_string(),
            home: home.to_path_buf(),
        }
    }
}

/// Action verbs served on `action/apps/<verb>`.
pub const ACTION_VERBS: [&str; 3] = ["list", "favorites", "set-favorite"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// `action/apps/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/apps/{verb}")
}

/// Build the reply for one `action/apps/<verb>` request. Aggregates fresh on each
/// `list` (refresh-on-open). The directory + inventory documents are read via the
/// provided closures so the heavy mackesd context isn't a hard dependency of the
/// pure aggregation (and tests inject fixtures).
#[must_use]
pub fn build_reply<D, I>(
    svc: &AppsService,
    verb: &str,
    body: Option<&str>,
    dir_doc: D,
    inv_doc: I,
) -> String
where
    D: FnOnce() -> serde_json::Value,
    I: FnOnce() -> serde_json::Value,
{
    match verb {
        "list" => {
            let local = scan_local_apps(&default_app_dirs(&svc.home));
            let mesh = mesh_entries_from_directory(&dir_doc(), &svc.node_id);
            let workloads = workload_entries_from_inventory(&inv_doc(), &svc.node_id);
            build_list(local, mesh, workloads)
        }
        // APPS-4 — per-user favorites on QNM-Shared (Q10).
        "favorites" => {
            let user = user_from_body(body);
            json!({ "ok": true, "favorites": read_favorites(&svc.workgroup_root, &user) })
                .to_string()
        }
        "set-favorite" => {
            let v: serde_json::Value = body
                .and_then(|b| serde_json::from_str(b).ok())
                .unwrap_or(json!({}));
            let user = v.get("user").and_then(|u| u.as_str()).unwrap_or("_");
            let id = v.get("id").and_then(|i| i.as_str()).unwrap_or_default();
            let pinned = v
                .get("pinned")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if id.is_empty() {
                json!({ "ok": false, "error": "set-favorite: missing id" }).to_string()
            } else {
                let favs = set_favorite(&svc.workgroup_root, user, id, pinned);
                json!({ "ok": true, "favorites": favs }).to_string()
            }
        }
        other => json!({ "ok": false, "error": format!("unknown apps verb: {other}") }).to_string(),
    }
}

/// Read this node's latest published compute inventory (`compute/inventory/<node>`)
/// from the bus, for the Workloads source (Q19). Opens its own short-lived Persist
/// handle (the responder owns the serve-loop handle); an absent inventory is an
/// honest empty doc, never an error.
#[must_use]
pub fn read_local_inventory(node_id: &str) -> serde_json::Value {
    let topic = format!("compute/inventory/{node_id}");
    let Some(root) = mde_bus::default_data_dir() else {
        return json!({});
    };
    let Ok(persist) = Persist::open(root) else {
        return json!({});
    };
    let Ok(Some(latest)) = persist.latest_ulid(&topic) else {
        return json!({});
    };
    // Re-read the newest message's body (latest_ulid gives the cursor; one
    // bounded list_since from the prior ulid returns it).
    persist
        .list_since(&topic, None)
        .ok()
        .and_then(|msgs| msgs.into_iter().rev().find(|m| m.ulid == latest))
        .and_then(|m| m.body)
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_else(|| json!({}))
}

/// Seed each verb cursor at the topic's current tail so a restart doesn't
/// reprocess the backlog (the MUSIC-WEDGE lesson — a stale `list` request must
/// not re-run the aggregation scan on every boot).
#[must_use]
pub fn seed_cursors_at_tail(persist: &Persist) -> HashMap<String, String> {
    let mut cursors = HashMap::new();
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        if let Ok(Some(latest)) = persist.latest_ulid(&topic) {
            cursors.insert(topic, latest);
        }
    }
    cursors
}

/// One poll sweep: answer every new `action/apps/<verb>` request on `reply/<ulid>`.
/// `dir_doc`/`inv_doc` are re-invoked per request so each `list` is fresh.
pub fn poll_once<D, I>(
    persist: &Persist,
    svc: &AppsService,
    cursors: &mut HashMap<String, String>,
    dir_doc: &D,
    inv_doc: &I,
) where
    D: Fn() -> serde_json::Value,
    I: Fn() -> serde_json::Value,
{
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "apps responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref(), dir_doc, inv_doc)
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "apps responder: reply write failed");
            }
        }
    }
}

/// Run the `action/apps/*` responder loop until `should_stop`. `dir_doc`/`inv_doc`
/// supply the live directory + compute-inventory documents (refresh-on-open).
pub fn serve_bus<F, D, I>(
    persist: &Persist,
    svc: &AppsService,
    dir_doc: D,
    inv_doc: I,
    should_stop: F,
) where
    F: Fn() -> bool,
    D: Fn() -> serde_json::Value,
    I: Fn() -> serde_json::Value,
{
    let mut cursors = seed_cursors_at_tail(persist);
    while !should_stop() {
        poll_once(persist, svc, &mut cursors, &dir_doc, &inv_doc);
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_desktop_entry_reads_application_and_skips_nondisplay() {
        let ok = "[Desktop Entry]\nType=Application\nName=Files\nExec=mde-files %U\nIcon=system-file-manager\n";
        let e = parse_desktop_entry("mde-files", ok, false).expect("entry");
        assert_eq!(e.name, "Files");
        assert_eq!(e.exec, "mde-files %U");
        assert_eq!(e.kind, "app");
        assert_eq!(e.source, "xdg");
        // NoDisplay / Hidden / non-Application / missing fields → skipped.
        assert!(parse_desktop_entry(
            "x",
            "[Desktop Entry]\nType=Application\nName=N\nExec=e\nNoDisplay=true\n",
            false
        )
        .is_none());
        assert!(parse_desktop_entry(
            "x",
            "[Desktop Entry]\nType=Application\nName=N\nExec=e\nHidden=true\n",
            false
        )
        .is_none());
        assert!(
            parse_desktop_entry("x", "[Desktop Entry]\nType=Link\nName=N\nURL=u\n", false)
                .is_none()
        );
        assert!(
            parse_desktop_entry("x", "[Desktop Entry]\nType=Application\nExec=e\n", false)
                .is_none()
        );
        // A key outside [Desktop Entry] (an action group) must not leak in.
        let multi = "[Desktop Entry]\nType=Application\nName=Real\nExec=real\n[Desktop Action new]\nName=New Window\nExec=real --new\n";
        assert_eq!(parse_desktop_entry("r", multi, false).unwrap().name, "Real");
        // Flatpak tag.
        assert_eq!(
            parse_desktop_entry("f", ok, true).unwrap().source,
            "flatpak"
        );
    }

    #[test]
    fn scan_local_apps_dedups_by_id_with_precedence_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let d1 = tmp.path().join("d1");
        let d2 = tmp.path().join("d2");
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        // Same id in both dirs — d1 (earlier) must win.
        std::fs::write(
            d1.join("z.desktop"),
            "[Desktop Entry]\nType=Application\nName=Zeta-d1\nExec=z\n",
        )
        .unwrap();
        std::fs::write(
            d2.join("z.desktop"),
            "[Desktop Entry]\nType=Application\nName=Zeta-d2\nExec=z\n",
        )
        .unwrap();
        std::fs::write(
            d2.join("a.desktop"),
            "[Desktop Entry]\nType=Application\nName=Alpha\nExec=a\n",
        )
        .unwrap();
        std::fs::write(
            d2.join("skip.desktop"),
            "[Desktop Entry]\nType=Application\nName=S\nExec=s\nNoDisplay=true\n",
        )
        .unwrap();
        let apps = scan_local_apps(&[d1, d2]);
        assert_eq!(apps.len(), 2); // z (deduped) + a; skip excluded
        assert_eq!(apps[0].name, "Alpha"); // sorted
        assert_eq!(apps[1].name, "Zeta-d1"); // d1 precedence
    }

    #[test]
    fn mesh_entries_skip_self_and_map_services() {
        let dir = json!({"peers": [
            {"hostname": "me", "presence": "online", "overlay_ip": "10.42.0.1"},
            {"hostname": "peerB", "presence": "online", "overlay_ip": "10.42.0.2",
             "descriptors": {"services": [{"name": "Jellyfin", "endpoint": "http://10.42.0.2:8096"}]}}
        ]});
        let e = mesh_entries_from_directory(&dir, "me");
        // self ("me") skipped; peerB → 1 mesh-app + 1 service.
        assert_eq!(e.iter().filter(|x| x.kind == "mesh-app").count(), 1);
        let svc = e.iter().find(|x| x.kind == "service").expect("service");
        assert_eq!(svc.name, "Jellyfin");
        assert_eq!(svc.endpoint, "http://10.42.0.2:8096");
        assert_eq!(svc.node, "peerB");
        assert!(!e.iter().any(|x| x.node == "me"));
    }

    #[test]
    fn workload_entries_map_vms_and_containers() {
        let inv = json!({
            "vms": [{"id": "uuid1", "name": "win10", "state": "running"}],
            "containers": [{"id": "c1", "name": "nginx", "state": "exited"}]
        });
        let e = workload_entries_from_inventory(&inv, "node1");
        assert_eq!(e.len(), 2);
        let vm = e.iter().find(|x| x.source == "libvirt").unwrap();
        assert_eq!(vm.name, "win10");
        assert_eq!(vm.state, "running");
        assert_eq!(vm.kind, "workload");
        let ct = e.iter().find(|x| x.source == "podman").unwrap();
        assert_eq!(ct.name, "nginx");
    }

    #[test]
    fn favorites_round_trip_per_user() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Empty until pinned.
        assert!(read_favorites(root, "mm").is_empty());
        // Pin two; idempotent (no dupes); unpin one.
        set_favorite(root, "mm", "firefox", true);
        let after = set_favorite(root, "mm", "firefox", true); // dup pin
        assert_eq!(after, vec!["firefox"]);
        set_favorite(root, "mm", "gimp", true);
        assert_eq!(read_favorites(root, "mm"), vec!["firefox", "gimp"]);
        set_favorite(root, "mm", "firefox", false); // unpin
        assert_eq!(read_favorites(root, "mm"), vec!["gimp"]);
        // Per-user isolation: another user is unaffected.
        assert!(read_favorites(root, "alice").is_empty());
        // Username is sanitized into a single path component (no traversal).
        assert!(favorites_path(root, "../etc/passwd")
            .to_string_lossy()
            .ends_with("apps-favorites/___etc_passwd.json"));
    }

    #[test]
    fn build_list_merges_and_counts() {
        let local = vec![AppEntry {
            id: "a".into(),
            name: "A".into(),
            kind: "app".into(),
            source: "xdg".into(),
            node: String::new(),
            exec: "a".into(),
            endpoint: String::new(),
            icon: String::new(),
            health: String::new(),
            state: String::new(),
        }];
        let mesh = mesh_entries_from_directory(
            &json!({"peers":[{"hostname":"p","presence":"online"}]}),
            "me",
        );
        let reply: serde_json::Value =
            serde_json::from_str(&build_list(local, mesh, vec![])).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["counts"]["app"], 1);
        assert_eq!(reply["counts"]["mesh-app"], 1);
        assert_eq!(reply["counts"]["total"], 2);
    }
}
