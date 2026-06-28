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

/// WORKLOAD-FLEET-2 — fold EVERY peer's compute inventory off the replicated
/// QNM-Shared plane (`<root>/<host>/compute-inventory.json`, written by each
/// node's `compute_registry`) into workload entries attributed to that host, so
/// the Start-menu Workloads tab shows fleet-wide VMs/containers, not just local.
/// Empty when the share isn't mounted (caller falls back to the local doc).
#[must_use]
pub fn fleet_workload_entries(this_node: &str) -> Vec<AppEntry> {
    fleet_workload_entries_in(&crate::default_qnm_shared_root(), this_node)
}

/// Pure variant (unit-tested): read every `<root>/<host>/compute-inventory.json`.
///
/// Ports the proven `fold_bus_inventories` discipline (mde-workbench compute
/// panel): every node — including this one — publishes its own
/// `compute-inventory.json` onto the share, so without a self-skip the local
/// box's own VMs/containers would surface here AND again from the responder's
/// local-bus fallback / live probe. Skip the doc whose `hostname` is
/// `this_node`, and dedup any row already folded (node + source + name), so a
/// stale duplicate doc can't double-list a workload.
#[must_use]
pub fn fleet_workload_entries_in(root: &std::path::Path, this_node: &str) -> Vec<AppEntry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<AppEntry> = Vec::new();
    for ent in entries.flatten() {
        let path = ent.path().join("compute-inventory.json");
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(inv) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let node = inv
            .get("hostname")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .to_string();
        // This node publishes its own inventory too — skip the whole doc when
        // it's us (the local workloads come from the responder's own bus doc).
        if !this_node.is_empty() && node == this_node {
            continue;
        }
        for entry in workload_entries_from_inventory(&inv, &node) {
            let dup = out
                .iter()
                .any(|e| e.node == entry.node && e.source == entry.source && e.name == entry.name);
            if dup {
                continue;
            }
            out.push(entry);
        }
    }
    out
}

// ───────────────────────── running apps (APPS-LIVE-1) ─────────────────────────

/// APPS-LIVE-1 — fold every peer's running-app set off the replicated QNM-Shared
/// plane (`<root>/<host>/running-apps.json`, written by each node's `apps_running`
/// worker) into a map `desktop-id → sorted unique hosts running it`. Empty when
/// the share isn't mounted. The launcher uses this to badge each local app entry
/// with a live "running on <host>" indicator, mesh-wide.
#[must_use]
pub fn fleet_running_hosts_in(root: &std::path::Path) -> HashMap<String, Vec<String>> {
    let mut by_id: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return HashMap::new();
    };
    for ent in entries.flatten() {
        let path = ent.path().join("running-apps.json");
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let host = doc
            .get("hostname")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .to_string();
        if host.is_empty() {
            continue;
        }
        if let Some(ids) = doc.get("ids").and_then(|i| i.as_array()) {
            for id in ids.iter().filter_map(|v| v.as_str()) {
                by_id
                    .entry(id.to_string())
                    .or_default()
                    .insert(host.clone());
            }
        }
    }
    by_id
        .into_iter()
        .map(|(id, hosts)| (id, hosts.into_iter().collect()))
        .collect()
}

/// APPS-LIVE-1 — stamp running state onto the local app entries: for each entry
/// whose desktop id appears in `running` (the [`fleet_running_hosts_in`] map), set
/// `state="running"` and `node` to the comma-joined hosts it's live on. Pure +
/// unit-tested; `apps` is consumed and returned so the merge is a simple map.
#[must_use]
pub fn apply_running_state(
    mut apps: Vec<AppEntry>,
    running: &HashMap<String, Vec<String>>,
) -> Vec<AppEntry> {
    for app in &mut apps {
        if let Some(hosts) = running.get(&app.id) {
            if !hosts.is_empty() {
                app.state = "running".into();
                app.node = hosts.join(", ");
            }
        }
    }
    apps
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

// ───────────────────────── peer-app discovery (APPLAUNCH-5) ─────────────────────────

/// APPLAUNCH-5 — read a peer's installed `.desktop` app set off the replicated
/// QNM-Shared plane (`<root>/<host>/apps-installed.json`, published by that
/// peer's `apps_installed` worker). This is a **local-disk** read of the
/// Syncthing-mirrored file, so it never blocks on a network round-trip to a
/// slow/dead peer (the lazy-mesh lock) — the worst case is a stale or absent
/// file, which is an honest empty set (mesh-down → no entries). Returns the
/// peer's launchable apps; empty when the share/file is absent or unparseable.
#[must_use]
pub fn read_peer_installed(workgroup_root: &Path, node: &str) -> Vec<AppEntry> {
    if node.is_empty() {
        return Vec::new();
    }
    let path = workgroup_root.join(node).join("apps-installed.json");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    doc.get("entries")
        .and_then(|e| serde_json::from_value::<Vec<AppEntry>>(e.clone()).ok())
        .unwrap_or_default()
}

/// APPLAUNCH-5 — build the `action/apps/peer-list` reply: a focused peer's
/// installed app set. An empty/self `node` answers THIS node's live local scan
/// (so the verb works mesh-down for the local box too); any other node reads the
/// replicated [`read_peer_installed`] file. Always `ok:true` — an unknown peer is
/// an empty list, not an error (the Front Door simply shows "no apps discovered").
#[must_use]
pub fn build_peer_list(svc: &AppsService, node: &str) -> String {
    let entries = if node.is_empty() || node == svc.node_id {
        // Self / unscoped → the live local scan (works with the mesh down).
        scan_local_apps(&default_app_dirs(&svc.home))
    } else {
        read_peer_installed(&svc.workgroup_root, node)
    };
    json!({
        "ok": true,
        "node": node,
        "entries": entries,
        "count": entries.len(),
    })
    .to_string()
}

// ───────────────────────── operator groups (APPLAUNCH-4) ─────────────────────────

/// APPLAUNCH-4 — one operator-curated group: a named, ordered bucket of entry
/// ids that renders as a collapsible section in the Apps view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppGroup {
    /// Display name of the group (e.g. "Dev tools").
    pub name: String,
    /// The entry ids in this group, in operator order.
    #[serde(default)]
    pub ids: Vec<String>,
}

/// Per-user app-groups file on QNM-Shared (APPLAUNCH-4): a JSON array of
/// [`AppGroup`] at `<workgroup_root>/app-groups/<user>.json`, so a user's curated
/// sections follow them to any node (mirrors the favorites store). mackesd (root)
/// is the only writer with mount access; the Front Door reads/sets via the bus
/// verbs.
#[must_use]
pub fn app_groups_path(workgroup_root: &Path, user: &str) -> PathBuf {
    workgroup_root
        .join("app-groups")
        .join(format!("{}.json", sanitize_user(user)))
}

/// Read a user's curated groups (empty when none/absent — never an error).
#[must_use]
pub fn read_app_groups(workgroup_root: &Path, user: &str) -> Vec<AppGroup> {
    std::fs::read_to_string(app_groups_path(workgroup_root, user))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<AppGroup>>(&s).ok())
        .unwrap_or_default()
}

/// Replace a user's curated groups wholesale; returns the persisted set. Writes
/// via temp+rename so a concurrent reader never sees a half-written file. The
/// Front Door's group editor sends the full set on each edit (add/rename/reorder/
/// remove), so the responder needs only this set-the-whole-thing verb.
pub fn set_app_groups(workgroup_root: &Path, user: &str, groups: &[AppGroup]) -> Vec<AppGroup> {
    let path = app_groups_path(workgroup_root, user);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(groups) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    groups.to_vec()
}

// ───────────────────────── uninstall (APPLAUNCH-6) ─────────────────────────

/// APPLAUNCH-6 — the launch binary of a local app's `Exec` line (what `rpm -qf`
/// maps to an owning package). Strips XDG field codes + a leading `env`/`VAR=val`
/// wrapper, mirroring [`crate::workers::apps_running::exec_basename`] but keeping
/// the full path (`rpm -qf` wants a path on disk, not a bare basename). `None`
/// for an empty/blank exec.
#[must_use]
pub fn exec_binary(exec: &str) -> Option<String> {
    exec.split_whitespace()
        .find(|t| !t.starts_with('%') && *t != "env" && !t.contains('='))
        .map(str::to_string)
}

/// APPLAUNCH-6 — resolve the typed uninstall argv for a local app entry (no
/// shell interpolation — a real `Command` argv, §9). A Flatpak app (its
/// desktop-file id is the Flatpak app-id, e.g. `org.gimp.GIMP`) → `flatpak
/// uninstall -y <id>`. An XDG app → `dnf remove -y <package>`, where `package`
/// is the entry's owning RPM resolved by the caller (via `rpm -qf` on the exec
/// binary) and passed in here. `None` for a non-app / empty entry, or an XDG app
/// whose package couldn't be resolved (mesh apps, workloads, services are never
/// "uninstalled" from here — out of scope).
#[must_use]
pub fn uninstall_argv(source: &str, id: &str, package: Option<&str>) -> Option<Vec<String>> {
    if id.is_empty() {
        return None;
    }
    match source {
        "flatpak" => Some(vec![
            "flatpak".into(),
            "uninstall".into(),
            "-y".into(),
            id.into(),
        ]),
        "xdg" => {
            let pkg = package.map(str::trim).filter(|p| !p.is_empty())?;
            Some(vec!["dnf".into(), "remove".into(), "-y".into(), pkg.into()])
        }
        _ => None,
    }
}

/// APPLAUNCH-6 — resolve the owning RPM package of an XDG app's exec binary via
/// `rpm -qf`. Returns the package NAME (e.g. `firefox`), or `None` when `rpm`
/// isn't present / the binary isn't owned by a package / the query fails. Runs a
/// real subprocess (the responder's effect); the argv is fixed (no shell), so
/// it's a typed call, not a shell channel (§9).
#[must_use]
pub fn resolve_owning_package(exec: &str) -> Option<String> {
    let bin = exec_binary(exec)?;
    let out = std::process::Command::new("rpm")
        .args(["-qf", "--queryformat", "%{NAME}", &bin])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() || name.contains("not owned") {
        None
    } else {
        Some(name)
    }
}

/// APPLAUNCH-6 — perform an app uninstall and return the `action/apps/uninstall`
/// reply. Resolves the typed argv ([`uninstall_argv`], with the owning package
/// resolved server-side for XDG apps) and spawns it as a real `Command` (no
/// shell). Always returns an `{ok, …}` envelope. The Front Door gates this behind
/// the operator's typed confirm before it ever publishes the request (§9 —
/// destructive, confirm-gated).
#[must_use]
pub fn build_uninstall(body: Option<&str>) -> String {
    let v: serde_json::Value = body
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(json!({}));
    let source = v.get("source").and_then(|s| s.as_str()).unwrap_or("");
    let id = v.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let exec = v.get("exec").and_then(|e| e.as_str()).unwrap_or("");
    let package = if source == "xdg" {
        resolve_owning_package(exec)
    } else {
        None
    };
    let Some(argv) = uninstall_argv(source, id, package.as_deref()) else {
        return json!({
            "ok": false,
            "error": format!("cannot uninstall '{id}' (unresolved package or unsupported source '{source}')")
        })
        .to_string();
    };
    let Some((cmd, args)) = argv.split_first() else {
        // uninstall_argv never yields an empty vec, but degrade rather than panic.
        return json!({ "ok": false, "error": "empty uninstall command" }).to_string();
    };
    match std::process::Command::new(cmd).args(args).output() {
        Ok(out) if out.status.success() => {
            json!({ "ok": true, "detail": format!("uninstalled {id}") }).to_string()
        }
        Ok(out) => json!({
            "ok": false,
            "error": String::from_utf8_lossy(&out.stderr).trim().to_string()
        })
        .to_string(),
        Err(e) => json!({ "ok": false, "error": format!("spawn {cmd} failed: {e}") }).to_string(),
    }
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

/// Action verbs served on `action/apps/<verb>`. APPLAUNCH adds `peer-list`
/// (APPLAUNCH-5 — a focused peer's installed `.desktop` set), `groups` +
/// `set-groups` (APPLAUNCH-4 — per-user operator-curated buckets), and
/// `uninstall` (APPLAUNCH-6 — confirm-gated dnf/flatpak removal).
pub const ACTION_VERBS: [&str; 8] = [
    "list",
    "favorites",
    "set-favorite",
    "launch",
    "peer-list",
    "groups",
    "set-groups",
    "uninstall",
];

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
            // APPS-LIVE-1 — stamp each local app entry with live running state +
            // the host(s) it runs on, folded off the replicated running-apps plane
            // (each node's apps_running worker). An unmounted share = no badges.
            let local = apply_running_state(
                scan_local_apps(&default_app_dirs(&svc.home)),
                &fleet_running_hosts_in(&svc.workgroup_root),
            );
            let mesh = mesh_entries_from_directory(&dir_doc(), &svc.node_id);
            // WORKLOAD-FLEET-2 — fleet-wide workloads from the replicated plane
            // (this node's own doc is self-skipped — its workloads come from the
            // local bus inventory below); fall back to that bus inventory wholly
            // when the share is absent.
            let mut workloads = fleet_workload_entries(&svc.node_id);
            if workloads.is_empty() {
                workloads = workload_entries_from_inventory(&inv_doc(), &svc.node_id);
            }
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
        // APPS-5 — resolve a peer's remote-desktop connection target from the
        // PD-2 directory (Q9/Q23/Q24: the thin applet asks; mackesd resolves). The
        // applet then spawns the local RD client to `protocol://target`.
        "launch" => {
            let node = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("node").and_then(|n| n.as_str()).map(str::to_string))
                .unwrap_or_default();
            match resolve_launch(&dir_doc(), &node) {
                Some((protocol, target)) => {
                    json!({ "ok": true, "protocol": protocol, "target": target }).to_string()
                }
                None => json!({
                    "ok": false,
                    "error": format!("no reachable remote-desktop target for peer '{node}'")
                })
                .to_string(),
            }
        }
        // APPLAUNCH-5 — a focused peer's installed `.desktop` set, read on demand
        // off the replicated plane (or the live local scan for self/unscoped). A
        // local-disk read, so a slow/dead peer never blocks the caller.
        "peer-list" => {
            let node = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("node").and_then(|n| n.as_str()).map(str::to_string))
                .unwrap_or_default();
            build_peer_list(svc, &node)
        }
        // APPLAUNCH-4 — per-user operator-curated groups on QNM-Shared.
        "groups" => {
            let user = user_from_body(body);
            json!({ "ok": true, "groups": read_app_groups(&svc.workgroup_root, &user) }).to_string()
        }
        "set-groups" => {
            let v: serde_json::Value = body
                .and_then(|b| serde_json::from_str(b).ok())
                .unwrap_or(json!({}));
            let user = v.get("user").and_then(|u| u.as_str()).unwrap_or("_");
            let groups: Vec<AppGroup> = v
                .get("groups")
                .and_then(|g| serde_json::from_value(g.clone()).ok())
                .unwrap_or_default();
            let saved = set_app_groups(&svc.workgroup_root, user, &groups);
            json!({ "ok": true, "groups": saved }).to_string()
        }
        // APPLAUNCH-6 — confirm-gated uninstall (dnf|flatpak). The Front Door
        // arms this only after the operator's typed confirm (§9 — destructive).
        "uninstall" => build_uninstall(body),
        other => json!({ "ok": false, "error": format!("unknown apps verb: {other}") }).to_string(),
    }
}

/// Resolve a peer's remote-desktop `(protocol, target)` from the PD-2 directory
/// (APPS-5). Prefers the peer's advertised remote-access descriptor; falls back to
/// RDP at the peer's overlay IP. `None` when the peer/overlay isn't known.
#[must_use]
pub fn resolve_launch(dir: &serde_json::Value, node: &str) -> Option<(String, String)> {
    let peers = dir.get("peers").and_then(|p| p.as_array())?;
    let peer = peers.iter().find(|p| {
        p.get("hostname")
            .or_else(|| p.get("name"))
            .and_then(|v| v.as_str())
            == Some(node)
    })?;
    // An explicit remote-access descriptor wins (protocol + host/port).
    if let Some(ra) = peer.get("descriptors").and_then(|d| d.get("remote_access")) {
        if let Some(host) = ra.get("host").and_then(|h| h.as_str()) {
            let proto = ra
                .get("protocol")
                .and_then(|p| p.as_str())
                .unwrap_or("rdp")
                .to_string();
            let target = match ra.get("port").and_then(serde_json::Value::as_u64) {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            return Some((proto, target));
        }
    }
    // Fallback: RDP to the overlay IP.
    let overlay = peer.get("overlay_ip").and_then(|v| v.as_str())?;
    if overlay.is_empty() {
        return None;
    }
    Some(("rdp".to_string(), overlay.to_string()))
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
    fn fleet_workload_entries_unions_every_peer_attributed_by_host() {
        let tmp = tempfile::tempdir().unwrap();
        for (host, vm) in [("fedora", "MDE-KVM-1"), ("node-13", "web1")] {
            let d = tmp.path().join(host);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("compute-inventory.json"),
                json!({"hostname": host, "vms": [{"id": format!("{host}-uuid"), "name": vm, "state": "running"}], "containers": []}).to_string(),
            )
            .unwrap();
        }
        // A non-inventory dir + file must be tolerated.
        std::fs::create_dir_all(tmp.path().join("peers")).unwrap();
        // this_node is some other host so both peer docs are folded.
        let e = fleet_workload_entries_in(tmp.path(), "self-host");
        assert_eq!(e.len(), 2, "one workload per peer file");
        let f = e.iter().find(|x| x.name == "MDE-KVM-1").unwrap();
        assert_eq!(f.node, "fedora");
        assert_eq!(f.kind, "workload");
        assert!(e.iter().any(|x| x.name == "web1" && x.node == "node-13"));
    }

    #[test]
    fn fleet_workload_entries_empty_when_no_root() {
        assert!(
            fleet_workload_entries_in(std::path::Path::new("/nonexistent-xyz"), "self").is_empty()
        );
    }

    #[test]
    fn fleet_workload_entries_self_skips_own_doc_and_dedups() {
        // Ports the fold_bus_inventories discipline (compute panel): the local
        // node's own published doc must NOT be folded here (it duplicates the
        // responder's local-bus inventory), and a stale duplicate peer doc must
        // not double-list a workload.
        let tmp = tempfile::tempdir().unwrap();
        // This node's own doc — must be skipped wholesale.
        let me = tmp.path().join("fedora");
        std::fs::create_dir_all(&me).unwrap();
        std::fs::write(
            me.join("compute-inventory.json"),
            json!({"hostname": "fedora",
                   "vms": [{"id": "u-local", "name": "MDE-KVM-1", "state": "running"}],
                   "containers": []})
            .to_string(),
        )
        .unwrap();
        // A genuine peer doc.
        let peer = tmp.path().join("node-13");
        std::fs::create_dir_all(&peer).unwrap();
        std::fs::write(
            peer.join("compute-inventory.json"),
            json!({"hostname": "node-13",
                   "vms": [{"id": "u-web", "name": "web1", "state": "running"}],
                   "containers": [{"id": "c-db", "name": "db", "state": "exited"}]})
            .to_string(),
        )
        .unwrap();
        // A stale second copy of the SAME peer (different dir, same hostname +
        // same workloads) — the dedup must collapse it to one row each.
        let stale = tmp.path().join("node-13-stale");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(
            stale.join("compute-inventory.json"),
            json!({"hostname": "node-13",
                   "vms": [{"id": "u-web2", "name": "web1", "state": "running"}],
                   "containers": []})
            .to_string(),
        )
        .unwrap();

        let e = fleet_workload_entries_in(tmp.path(), "fedora");
        // self ("fedora") doc skipped → no MDE-KVM-1; node-13's web1 + db, each once.
        assert!(
            !e.iter().any(|x| x.node == "fedora"),
            "this node's own doc must be self-skipped"
        );
        assert_eq!(
            e.iter().filter(|x| x.name == "web1").count(),
            1,
            "duplicate peer doc must be deduped to one web1"
        );
        assert!(e.iter().any(|x| x.name == "db" && x.node == "node-13"));
        assert_eq!(e.len(), 2, "web1 + db, no self rows, no dupes");
    }

    #[test]
    fn fleet_running_hosts_unions_peers_and_dedups_by_host() {
        let tmp = tempfile::tempdir().unwrap();
        // fedora runs firefox + gimp; node-13 also runs firefox.
        for (host, ids) in [
            ("fedora", vec!["firefox", "gimp"]),
            ("node-13", vec!["firefox"]),
        ] {
            let d = tmp.path().join(host);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("running-apps.json"),
                json!({"hostname": host, "ids": ids}).to_string(),
            )
            .unwrap();
        }
        // A non-running-apps dir is tolerated.
        std::fs::create_dir_all(tmp.path().join("peers")).unwrap();
        let map = fleet_running_hosts_in(tmp.path());
        let mut ff = map.get("firefox").cloned().unwrap();
        ff.sort();
        assert_eq!(ff, vec!["fedora".to_string(), "node-13".to_string()]);
        assert_eq!(
            map.get("gimp").cloned().unwrap(),
            vec!["fedora".to_string()]
        );
    }

    #[test]
    fn fleet_running_hosts_empty_when_no_root() {
        assert!(fleet_running_hosts_in(std::path::Path::new("/nonexistent-xyz")).is_empty());
    }

    #[test]
    fn apply_running_state_badges_matching_apps_only() {
        let apps = vec![
            AppEntry {
                id: "firefox".into(),
                name: "Firefox".into(),
                kind: "app".into(),
                source: "xdg".into(),
                node: String::new(),
                exec: "firefox %u".into(),
                endpoint: String::new(),
                icon: String::new(),
                health: String::new(),
                state: String::new(),
            },
            AppEntry {
                id: "gimp".into(),
                name: "GIMP".into(),
                kind: "app".into(),
                source: "flatpak".into(),
                node: String::new(),
                exec: "gimp".into(),
                endpoint: String::new(),
                icon: String::new(),
                health: String::new(),
                state: String::new(),
            },
        ];
        let mut running = HashMap::new();
        running.insert(
            "firefox".to_string(),
            vec!["fedora".to_string(), "node-13".to_string()],
        );
        let out = apply_running_state(apps, &running);
        let ff = out.iter().find(|a| a.id == "firefox").unwrap();
        assert_eq!(ff.state, "running");
        assert_eq!(ff.node, "fedora, node-13");
        // Unmatched app is untouched.
        let g = out.iter().find(|a| a.id == "gimp").unwrap();
        assert!(g.state.is_empty());
        assert!(g.node.is_empty());
    }

    #[test]
    fn resolve_launch_prefers_descriptor_then_overlay() {
        let dir = json!({"peers": [
            {"hostname": "plain", "overlay_ip": "10.42.0.3"},
            {"hostname": "rich", "overlay_ip": "10.42.0.4",
             "descriptors": {"remote_access": {"protocol": "vnc", "host": "10.42.0.4", "port": 5900}}},
            {"hostname": "noip", "overlay_ip": ""}
        ]});
        // Fallback: RDP to the overlay IP.
        assert_eq!(
            resolve_launch(&dir, "plain"),
            Some(("rdp".into(), "10.42.0.3".into()))
        );
        // Descriptor wins (protocol + host:port).
        assert_eq!(
            resolve_launch(&dir, "rich"),
            Some(("vnc".into(), "10.42.0.4:5900".into()))
        );
        // No overlay / unknown peer → None.
        assert_eq!(resolve_launch(&dir, "noip"), None);
        assert_eq!(resolve_launch(&dir, "ghost"), None);
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

    // ───────────────────── APPLAUNCH-5 — peer-list ─────────────────────

    #[test]
    fn read_peer_installed_folds_the_replicated_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Seed a peer's apps-installed.json (what its apps_installed worker writes).
        let dir = root.join("anvil");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("apps-installed.json"),
            r#"{"hostname":"anvil","entries":[{"id":"firefox","name":"Firefox","kind":"app","source":"xdg","node":"anvil","exec":"firefox %u"}]}"#,
        )
        .unwrap();
        let got = read_peer_installed(root, "anvil");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Firefox");
        assert_eq!(got[0].node, "anvil");
        // An unknown peer / absent file → empty, never an error.
        assert!(read_peer_installed(root, "ghost").is_empty());
        // An empty node selector → empty (the verb routes self elsewhere).
        assert!(read_peer_installed(root, "").is_empty());
    }

    #[test]
    fn peer_list_self_uses_live_scan_peer_uses_share() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let home = tmp.path().join("home");
        // A local app on THIS node.
        let apps = home.join(".local/share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("term.desktop"),
            "[Desktop Entry]\nType=Application\nName=Terminal\nExec=cosmic-term\n",
        )
        .unwrap();
        // A peer's published set on the share.
        let peer_dir = root.join("anvil");
        std::fs::create_dir_all(&peer_dir).unwrap();
        std::fs::write(
            peer_dir.join("apps-installed.json"),
            r#"{"hostname":"anvil","entries":[{"id":"gimp","name":"GIMP","kind":"app","source":"flatpak","node":"anvil","exec":"gimp"}]}"#,
        )
        .unwrap();
        let svc = AppsService::new(root, "me", &home);
        // Self / unscoped → the live local scan.
        let me: serde_json::Value = serde_json::from_str(&build_peer_list(&svc, "")).unwrap();
        assert_eq!(me["ok"], true);
        assert_eq!(me["entries"].as_array().unwrap().len(), 1);
        assert_eq!(me["entries"][0]["name"], "Terminal");
        // A focused peer → the replicated file.
        let peer: serde_json::Value =
            serde_json::from_str(&build_peer_list(&svc, "anvil")).unwrap();
        assert_eq!(peer["node"], "anvil");
        assert_eq!(peer["entries"][0]["name"], "GIMP");
        // A dead peer → ok:true with an empty list (never blocks, never errors).
        let dead: serde_json::Value =
            serde_json::from_str(&build_peer_list(&svc, "ghost")).unwrap();
        assert_eq!(dead["ok"], true);
        assert_eq!(dead["count"], 0);
    }

    // ───────────────────── APPLAUNCH-4 — app groups ─────────────────────

    #[test]
    fn app_groups_round_trip_per_user() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(read_app_groups(root, "mm").is_empty());
        let groups = vec![
            AppGroup {
                name: "Dev".into(),
                ids: vec!["firefox".into(), "code".into()],
            },
            AppGroup {
                name: "Media".into(),
                ids: vec!["mpv".into()],
            },
        ];
        let saved = set_app_groups(root, "mm", &groups);
        assert_eq!(saved, groups);
        assert_eq!(read_app_groups(root, "mm"), groups);
        // Per-user isolation + path sanitization (no traversal).
        assert!(read_app_groups(root, "alice").is_empty());
        assert!(app_groups_path(root, "../x")
            .to_string_lossy()
            .ends_with("app-groups/___x.json"));
        // A wholesale replace (the editor sends the full set each edit).
        let replaced = set_app_groups(root, "mm", &[]);
        assert!(replaced.is_empty());
        assert!(read_app_groups(root, "mm").is_empty());
    }

    // ───────────────────── APPLAUNCH-6 — uninstall ─────────────────────

    #[test]
    fn uninstall_argv_flatpak_and_xdg_and_unsupported() {
        // Flatpak → flatpak uninstall -y <app-id>.
        assert_eq!(
            uninstall_argv("flatpak", "org.gimp.GIMP", None).unwrap(),
            ["flatpak", "uninstall", "-y", "org.gimp.GIMP"]
        );
        // XDG with a resolved package → dnf remove -y <pkg>.
        assert_eq!(
            uninstall_argv("xdg", "firefox", Some("firefox")).unwrap(),
            ["dnf", "remove", "-y", "firefox"]
        );
        // XDG with no resolved package → None (nothing to remove).
        assert!(uninstall_argv("xdg", "firefox", None).is_none());
        assert!(uninstall_argv("xdg", "firefox", Some("   ")).is_none());
        // Non-app sources are never uninstalled here.
        assert!(uninstall_argv("peer", "mesh-app:anvil", None).is_none());
        assert!(uninstall_argv("service", "service:x:y", None).is_none());
        // Empty id → None.
        assert!(uninstall_argv("flatpak", "", None).is_none());
    }

    #[test]
    fn exec_binary_strips_field_codes_and_env() {
        assert_eq!(exec_binary("firefox %u").as_deref(), Some("firefox"));
        assert_eq!(
            exec_binary("/usr/bin/firefox %U").as_deref(),
            Some("/usr/bin/firefox")
        );
        assert_eq!(
            exec_binary("env GTK_THEME=x gimp %f").as_deref(),
            Some("gimp")
        );
        assert_eq!(exec_binary("%U").as_deref(), None);
        assert_eq!(exec_binary("   ").as_deref(), None);
    }

    #[test]
    fn build_uninstall_rejects_unresolvable() {
        // A workload/service kind has no uninstall path → ok:false, never a panic.
        let reply: serde_json::Value = serde_json::from_str(&build_uninstall(Some(
            r#"{"source":"podman","id":"workload:podman:c1"}"#,
        )))
        .unwrap();
        assert_eq!(reply["ok"], false);
    }
}
