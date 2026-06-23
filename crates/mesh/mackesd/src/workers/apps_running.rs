//! APPS-LIVE-1 — the `apps_running` worker: publish this node's set of
//! currently-running launchable apps to the replicated QNM-Shared plane so the
//! Applications-menu launcher (`mde-apps-applet`) can badge every entry with a
//! live "running on <host>" indicator, mesh-wide.
//!
//! ## Why a QNM-Shared file (not a bus topic)
//!
//! Same transport choice WORKLOAD-FLEET-1 locked for `compute-inventory.json`:
//! the Mackes Bus is per-node (there is no bus-federation worker — reading the
//! local bus would only ever show *self*). The replicated QNM-Shared mount is the
//! cross-node plane, so each node mirrors its running-app set to
//! `<mount>/<hostname>/running-apps.json` and every peer's launcher folds them in
//! (`ipc::apps::fleet_running_hosts_in`). Atomic tmp+rename so a reader never sees a
//! half-written file.
//!
//! ## How "running" is detected (process ↔ `.desktop` match)
//!
//! mackesd is a root daemon with no Wayland/X11 session of its own, so it can't
//! query a compositor's toplevel list. It *can* read every `/proc/<pid>/cmdline`
//! (root sees all PIDs regardless of owner). So the detector matches the executable
//! basename of each launchable `.desktop` entry's `Exec` line against the set of
//! running-process basenames — a `.desktop` app is "running" when its launch binary
//! has a live process. This is the `docs/WORKLIST.md` APPS-LIVE-1 acceptance
//! "process/`.desktop` ↔ running window/app match", reachable from the root daemon
//! without a per-seat compositor probe.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::{ShutdownToken, Worker};
use crate::ipc::apps::{default_app_dirs, scan_local_apps};

/// Tick cadence. Matches `compute_registry`'s 10 s inventory cadence so the
/// running badges refresh on the same beat as the workload rows.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(10);

/// File this node's running-app set is mirrored to under its QNM-Shared dir.
pub const SHARED_RUNNING_FILE: &str = "running-apps.json";

/// The published running-app document for one node. `ids` are the `.desktop`
/// file ids (the launcher's stable [`crate::ipc::apps::AppEntry::id`] for local
/// apps) found running on this `hostname`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunningApps {
    /// Publishing node hostname (the launcher attributes "running on <hostname>").
    pub hostname: String,
    /// Desktop-file ids of apps with a live process on this node, sorted+unique.
    pub ids: Vec<String>,
}

/// Extract the launch-binary basename from a `.desktop` `Exec` line. Strips XDG
/// field codes (`%U`/`%f`/…), honors an absolute path (takes the final
/// component), and drops an `env`/wrapper prefix's leading `VAR=val` assignments
/// so `env GTK_THEME=… firefox` resolves to `firefox`. `None` for an empty/blank
/// exec.
#[must_use]
pub fn exec_basename(exec: &str) -> Option<String> {
    for tok in exec.split_whitespace() {
        // Skip field codes and leading `VAR=val` environment assignments
        // (`env FOO=bar app` / `FOO=bar app`).
        if tok.starts_with('%') || tok == "env" || tok.contains('=') {
            continue;
        }
        let base = Path::new(tok)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(tok);
        if base.is_empty() {
            continue;
        }
        return Some(base.to_string());
    }
    None
}

/// The set of running-process executable basenames read from `/proc`. Each
/// `/proc/<pid>/cmdline` is NUL-separated argv; argv[0]'s basename is the program.
/// Unreadable entries (races, permission on a non-root test box) are skipped, so a
/// non-Linux / sandboxed test host yields an empty set rather than an error.
#[must_use]
pub fn running_process_basenames() -> BTreeSet<String> {
    running_process_basenames_in(Path::new("/proc"))
}

/// Pure variant (unit-tested): read argv[0] basenames from `<proc>/<pid>/cmdline`.
#[must_use]
pub fn running_process_basenames_in(proc_root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(proc_root) else {
        return out;
    };
    for ent in rd.flatten() {
        // Only numeric PID dirs.
        let name = ent.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(raw) = std::fs::read(ent.path().join("cmdline")) else {
            continue;
        };
        // argv[0] is the bytes up to the first NUL.
        let argv0 = raw.split(|&b| b == 0).next().unwrap_or(&[]);
        if argv0.is_empty() {
            continue;
        }
        let argv0 = String::from_utf8_lossy(argv0);
        if let Some(base) = Path::new(argv0.as_ref())
            .file_name()
            .and_then(|s| s.to_str())
        {
            out.insert(base.to_string());
        }
    }
    out
}

/// Pure detector (unit-tested): given the launchable apps scanned from `app_dirs`
/// and the set of `running` process basenames, return the sorted+unique desktop
/// ids whose `Exec` basename has a live process. This is the body of one tick
/// minus the QNM-Shared write — injectable for tests.
///
/// **Flatpak apps are deliberately skipped.** A Flatpak `.desktop` Exec is
/// `/usr/bin/flatpak run … org.app.Id`, so the launch *binary* basename is always
/// `flatpak` — matching that would badge *every* Flatpak app as running the moment
/// any single Flatpak (or the `flatpak` helper itself) has a live process (mass
/// false positives). Correct per-app Flatpak liveness needs `flatpak ps` app-id
/// matching, which is out of scope here; until then we only badge XDG apps, whose
/// Exec binary is a real per-app process name (the APPS-LIVE-1 Firefox case).
#[must_use]
pub fn running_app_ids(app_dirs: &[PathBuf], running: &BTreeSet<String>) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for app in scan_local_apps(app_dirs) {
        // Skip Flatpak — its launch binary is always `flatpak` (see above).
        if app.source == "flatpak" {
            continue;
        }
        if let Some(base) = exec_basename(&app.exec) {
            if running.contains(&base) {
                ids.insert(app.id);
            }
        }
    }
    ids.into_iter().collect()
}

/// Write the running-app document to `<mount>/<hostname>/running-apps.json`,
/// atomically (tmp+rename), mirroring [`crate::workers::compute_registry::write_shared_inventory`].
/// No-op on an empty hostname. Best-effort — a write error is logged, never fatal.
pub fn write_shared_running(mount: &Path, doc: &RunningApps) {
    if doc.hostname.is_empty() {
        return;
    }
    let dir = mount.join(&doc.hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("apps_running: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(doc) else {
        return;
    };
    let tmp = dir.join("running-apps.json.tmp");
    let final_path = dir.join(SHARED_RUNNING_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("apps_running: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("apps_running: rename running-apps failed: {e}");
    }
}

/// Worker handle.
pub struct AppsRunningWorker {
    hostname: String,
    tick: Duration,
    mount: PathBuf,
    home: PathBuf,
}

impl AppsRunningWorker {
    /// Construct with production defaults. `hostname` attributes the published set;
    /// `home` is the desktop user's home (the local `.desktop` scan root, same as
    /// the apps aggregator).
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
        let running = running_process_basenames();
        let ids = running_app_ids(&default_app_dirs(&self.home), &running);
        let doc = RunningApps {
            hostname: self.hostname.clone(),
            ids,
        };
        write_shared_running(&self.mount, &doc);
    }
}

#[async_trait]
impl Worker for AppsRunningWorker {
    fn name(&self) -> &'static str {
        "apps_running"
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

    #[test]
    fn exec_basename_strips_field_codes_path_and_env() {
        assert_eq!(exec_basename("firefox %u").as_deref(), Some("firefox"));
        assert_eq!(
            exec_basename("/usr/bin/firefox %U").as_deref(),
            Some("firefox")
        );
        assert_eq!(
            exec_basename("env GTK_THEME=Adwaita firefox").as_deref(),
            Some("firefox")
        );
        assert_eq!(
            exec_basename("FOO=bar /opt/app/bin/gimp-2.10 %f").as_deref(),
            Some("gimp-2.10")
        );
        assert_eq!(exec_basename("   ").as_deref(), None);
        assert_eq!(exec_basename("%U").as_deref(), None);
    }

    #[test]
    fn running_process_basenames_reads_argv0() {
        let tmp = tempfile::tempdir().unwrap();
        let proc = tmp.path();
        // PID 100 → /usr/bin/firefox with args; PID 200 → cosmic-comp.
        for (pid, cmdline) in [
            ("100", "/usr/bin/firefox\0--new-window\0https://x\0"),
            ("200", "cosmic-comp\0"),
        ] {
            let d = proc.join(pid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("cmdline"), cmdline).unwrap();
        }
        // A non-numeric dir (e.g. /proc/self) + an empty cmdline (kernel thread)
        // must be tolerated.
        std::fs::create_dir_all(proc.join("self")).unwrap();
        let k = proc.join("999");
        std::fs::create_dir_all(&k).unwrap();
        std::fs::write(k.join("cmdline"), "").unwrap();

        let names = running_process_basenames_in(proc);
        assert!(names.contains("firefox"));
        assert!(names.contains("cosmic-comp"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn running_app_ids_matches_desktop_exec_to_live_process() {
        let tmp = tempfile::tempdir().unwrap();
        let apps = tmp.path().join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("firefox.desktop"),
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox %u\n",
        )
        .unwrap();
        std::fs::write(
            apps.join("gimp.desktop"),
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=gimp %F\n",
        )
        .unwrap();

        // Firefox is running; GIMP is not.
        let running: BTreeSet<String> = ["firefox".to_string()].into_iter().collect();
        let ids = running_app_ids(&[apps], &running);
        assert_eq!(ids, vec!["firefox".to_string()]);
    }

    #[test]
    fn running_app_ids_skips_flatpak_to_avoid_mass_false_positives() {
        let tmp = tempfile::tempdir().unwrap();
        // A flatpak export dir (path contains `flatpak` → source=flatpak).
        let fp = tmp.path().join("flatpak/exports/share/applications");
        std::fs::create_dir_all(&fp).unwrap();
        std::fs::write(
            fp.join("org.gimp.GIMP.desktop"),
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=/usr/bin/flatpak run --branch=stable org.gimp.GIMP\n",
        )
        .unwrap();
        // Even with the `flatpak` helper process live, the flatpak app is NOT
        // badged (its launch binary is `flatpak`, shared across all flatpaks).
        let running: BTreeSet<String> = ["flatpak".to_string()].into_iter().collect();
        assert!(running_app_ids(&[fp], &running).is_empty());
    }

    #[test]
    fn write_shared_running_lands_under_hostname_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = RunningApps {
            hostname: "fedora".into(),
            ids: vec!["firefox".into()],
        };
        write_shared_running(tmp.path(), &doc);
        let path = tmp.path().join("fedora").join(SHARED_RUNNING_FILE);
        let body = std::fs::read_to_string(&path).expect("running-apps written");
        let back: RunningApps = serde_json::from_str(&body).unwrap();
        assert_eq!(back, doc);
        // No stray temp file after the atomic rename.
        assert!(!tmp
            .path()
            .join("fedora")
            .join("running-apps.json.tmp")
            .exists());
    }

    #[test]
    fn write_shared_running_skips_empty_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        write_shared_running(tmp.path(), &RunningApps::default());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
