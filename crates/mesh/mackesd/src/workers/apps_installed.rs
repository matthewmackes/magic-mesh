//! APPLAUNCH-5 — the `apps_installed` worker: publish this node's set of
//! *installed* launchable `.desktop` apps to the replicated QNM-Shared plane so
//! the Front Door's Mesh filter can show a focused peer's real app set on demand
//! (the launch-on-peer target list), mesh-wide.
//!
//! ## Why a QNM-Shared file (not a live peer RPC)
//!
//! Same transport choice WORKLOAD-FLEET-1 / APPS-LIVE-1 locked for
//! `compute-inventory.json` + `running-apps.json`: the Mackes Bus is per-node
//! (there is no bus-federation worker — reading the local bus would only ever
//! show *self*). The replicated QNM-Shared mount is the cross-node plane, so each
//! node mirrors its installed-app set to `<mount>/<hostname>/apps-installed.json`
//! and a peer's set is answered on demand by the LOCAL mackesd reading that file
//! (`ipc::apps::read_peer_installed`). That keeps the on-demand `peer-list` verb a
//! local-disk read — **a slow/dead peer never blocks the UI** (APPLAUNCH-5's
//! lazy-mesh lock): the worst case is a stale or absent file (the mesh-down →
//! hide-mesh lock covers an absent share). Atomic tmp+rename so a reader never
//! sees a half-written file.
//!
//! ## What "installed" is
//!
//! The full local `.desktop` application set (XDG + Flatpak), de-duplicated by
//! desktop-file id — exactly the aggregator's local-app scan
//! ([`crate::ipc::apps::scan_local_apps`]), minus the running-state badging (this
//! is the *installed* catalog, not the live set). Published every 60 s — installed
//! apps change far less often than running ones, so a slower cadence than
//! `apps_running`'s 10 s suffices.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::{ShutdownToken, Worker};
use crate::ipc::apps::{default_app_dirs, scan_local_apps, AppEntry};

/// Tick cadence. Installed apps change rarely (a dnf/flatpak install), so a 60 s
/// republish keeps a focused peer's set fresh without measurable idle cost.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// File this node's installed-app set is mirrored to under its QNM-Shared dir.
pub const SHARED_INSTALLED_FILE: &str = "apps-installed.json";

/// The published installed-app document for one node. `entries` are the node's
/// launchable local apps (the same [`AppEntry`] shape the aggregator's `list`
/// reply uses), so the Front Door folds a peer's set in with no extra mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstalledApps {
    /// Publishing node hostname.
    pub hostname: String,
    /// The node's installed launchable `.desktop` apps (sorted by name).
    pub entries: Vec<AppEntry>,
}

/// Build the installed-app document for `hostname` from the local `.desktop`
/// scan over `app_dirs`. The entries are re-stamped onto the publishing node so a
/// reader knows which peer they came from. Pure (unit-tested); the QNM-Shared
/// write is the worker tick's only side effect.
#[must_use]
pub fn installed_doc(hostname: &str, app_dirs: &[PathBuf]) -> InstalledApps {
    let entries = scan_local_apps(app_dirs)
        .into_iter()
        .map(|mut e| {
            // Mark the owning node so the Front Door can badge "on <host>" and
            // route the launch-on-peer (the aggregator leaves `node` empty for
            // local apps; here every entry belongs to this publishing peer).
            e.node = hostname.to_string();
            e
        })
        .collect();
    InstalledApps {
        hostname: hostname.to_string(),
        entries,
    }
}

/// Write the installed-app document to `<mount>/<hostname>/apps-installed.json`,
/// atomically (tmp+rename), mirroring
/// [`crate::workers::apps_running::write_shared_running`]. No-op on an empty
/// hostname. Best-effort — a write error is logged, never fatal.
pub fn write_shared_installed(mount: &Path, doc: &InstalledApps) {
    if doc.hostname.is_empty() {
        return;
    }
    let dir = mount.join(&doc.hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("apps_installed: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(doc) else {
        return;
    };
    let tmp = dir.join("apps-installed.json.tmp");
    let final_path = dir.join(SHARED_INSTALLED_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("apps_installed: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("apps_installed: rename apps-installed failed: {e}");
    }
}

/// Worker handle.
pub struct AppsInstalledWorker {
    hostname: String,
    tick: Duration,
    mount: PathBuf,
    home: PathBuf,
}

impl AppsInstalledWorker {
    /// Construct with production defaults. `hostname` attributes the published
    /// set; `home` is the desktop user's home (the local `.desktop` scan root,
    /// same as the apps aggregator).
    #[must_use]
    pub fn new(hostname: String, home: PathBuf) -> Self {
        Self {
            hostname,
            tick: DEFAULT_TICK_INTERVAL,
            mount: crate::default_qnm_shared_root(),
            home,
        }
    }

    /// Override the mesh-storage mount path. Used in tests.
    #[must_use]
    pub fn with_mount(mut self, p: PathBuf) -> Self {
        self.mount = p;
        self
    }

    fn tick_once(&self) {
        // Only publish when the share is a real mount — never write to a bare
        // local dir masquerading as the share (the WORKLOAD-FLEET-1 guard).
        if !crate::workers::compute_registry::is_meshfs_mounted(&self.mount) {
            return;
        }
        let doc = installed_doc(&self.hostname, &default_app_dirs(&self.home));
        write_shared_installed(&self.mount, &doc);
    }
}

#[async_trait]
impl Worker for AppsInstalledWorker {
    fn name(&self) -> &'static str {
        "apps_installed"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_desktop(dir: &Path, file: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn installed_doc_scans_and_stamps_owning_node() {
        let tmp = tempfile::tempdir().unwrap();
        let apps = tmp.path().join("applications");
        write_desktop(
            &apps,
            "firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\n",
        );
        write_desktop(
            &apps,
            "gimp.desktop",
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=gimp\n",
        );
        let doc = installed_doc("anvil", &[apps]);
        assert_eq!(doc.hostname, "anvil");
        assert_eq!(doc.entries.len(), 2);
        // Sorted by name (scan_local_apps sorts) + every entry stamped on anvil.
        assert_eq!(doc.entries[0].name, "Firefox");
        assert!(doc.entries.iter().all(|e| e.node == "anvil"));
        assert!(doc.entries.iter().all(|e| e.kind == "app"));
    }

    #[test]
    fn write_shared_installed_lands_under_hostname_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = InstalledApps {
            hostname: "fedora".into(),
            entries: vec![AppEntry {
                id: "firefox".into(),
                name: "Firefox".into(),
                kind: "app".into(),
                source: "xdg".into(),
                node: "fedora".into(),
                exec: "firefox %u".into(),
                endpoint: String::new(),
                icon: "firefox".into(),
                health: String::new(),
                state: String::new(),
            }],
        };
        write_shared_installed(tmp.path(), &doc);
        let path = tmp.path().join("fedora").join(SHARED_INSTALLED_FILE);
        let body = std::fs::read_to_string(&path).expect("apps-installed written");
        let back: InstalledApps = serde_json::from_str(&body).unwrap();
        assert_eq!(back, doc);
        // No stray temp file after the atomic rename.
        assert!(!tmp
            .path()
            .join("fedora")
            .join("apps-installed.json.tmp")
            .exists());
    }

    #[test]
    fn write_shared_installed_skips_empty_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        write_shared_installed(tmp.path(), &InstalledApps::default());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
