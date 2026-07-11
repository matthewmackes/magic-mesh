//! PRINT-2..PRINT-6 + PRINT-8 (event half) (v5.0.0) — auto CUPS print
//! sharing + sync worker.
//!
//! Runs on **headless + full** peers (lighthouse skips it at spawn).
//! Each 5 s tick converges the fleet's printers through the
//! `mesh-storage` volume the same write-own-file / read-union way the
//! PEERVER peer-data converges:
//!
//!   1. **Publish (PRINT-2).** Enumerate local queues (`lpstat`), write
//!      `<mesh-storage>/printers/<host>.json`; copy each legacy
//!      (non-IPP) queue's PPD to `<mesh-storage>/printers/ppd/<host>/`.
//!   2. **Share (PRINT-4).** Ensure `cupsd` listens on the **overlay IP
//!      only** (never `0.0.0.0`) + `cupsctl --share-printers` +
//!      per-queue `printer-is-shared=true`, so a job submitted on a
//!      remote peer reaches this host's hardware.
//!   3. **Import (PRINT-3).** Read the union of `printers/*.json`
//!      (minus self); `lpadmin` each remote queue as `<queue>@<host>`
//!      pointing at `ipp://<host-overlay>:631/printers/<queue>`
//!      (`-m everywhere`, or the replicated PPD for legacy). Prune
//!      local `<q>@<host>` queues whose host-file vanished.
//!   4. **Defaults (PRINT-5).** Reconcile `_defaults.json` (fleet
//!      default printer + per-queue presets) last-write-wins by
//!      `written_at_ms`; apply via `lpoptions`.
//!   5. **Auto-join (PRINT-6).** The periodic tick *is* the join: a
//!      newly-enrolled peer's `printers/<host>.json` becomes readable
//!      once `mesh-storage` mounts, and the next tick imports it (same
//!      polling-convergence model as `gluster_worker`).
//!   6. **Event (PRINT-8, event half).** On a local add/remove,
//!      publish `event/printers/<host>` on the Bus so panels refresh
//!      without polling. The `action/printers/{sync-now,list}` command
//!      surface is the PRINT-8.b follow-on.
//!
//! Silent no-op when `cupsd`/`lpadmin` aren't installed (the operator
//! hasn't opted into the print stack) or the overlay-ip publish file is
//! missing (peer hasn't completed Nebula enrollment) — exactly the
//! `gluster_worker` guard shape.
//!
//! Test surface: every decision is a pure function over parsed strings
//! / `serde_json::Value` (`parse_lpstat_e`, `parse_device_uri`,
//! `queue_kind`, `own_record`, `import_plan`, `lpadmin_add_argv`,
//! `prune_list`, `resolve_defaults_lww`, `cupsd_needs_listen`); the
//! worker body is a thin shell-out layer over them.

#![cfg(feature = "async-services")]

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mde_bus::hooks::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::{json, Value};

use super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH;
use super::{ShutdownToken, Worker};

/// PRINT-8.b — the two `action/printers/<verb>` topics this worker serves.
const ACTION_VERBS: [&str; 2] = ["sync-now", "list"];

/// Tick cadence — five seconds, matching the other mesh workers.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// IPP port `cupsd` serves on (the host's overlay endpoint).
pub const IPP_PORT: u16 = 631;

/// Whether a queue is a modern IPP-Everywhere printer (no PPD needed)
/// or a legacy queue whose PPD must replicate (Q4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    /// Driverless IPP Everywhere — importing peers use `-m everywhere`.
    Everywhere,
    /// Legacy — the host's PPD replicates so importers present options.
    Ppd,
}

impl QueueKind {
    /// Wire string written into `printers/<host>.json`'s `kind` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Everywhere => "everywhere",
            Self::Ppd => "ppd",
        }
    }
}

// ───────────────────────── pure helpers ─────────────────────────

/// Parse `lpstat -e` (one queue name per line) into queue names.
#[must_use]
pub fn parse_lpstat_e(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Extract the device URI for `queue` from `lpstat -v` output
/// (`device for <queue>: <uri>`).
#[must_use]
pub fn parse_device_uri(lpstat_v: &str, queue: &str) -> Option<String> {
    let needle = format!("device for {queue}:");
    lpstat_v
        .lines()
        .find_map(|l| l.trim().strip_prefix(&needle).map(|u| u.trim().to_string()))
}

/// Classify a queue from its device URI: an `ipp`/`ipps`/`dnssd` URI is
/// a driverless IPP-Everywhere candidate; everything else (usb, socket,
/// parallel, lpd, …) is legacy and replicates its PPD.
#[must_use]
pub fn queue_kind(device_uri: &str) -> QueueKind {
    let u = device_uri.trim_start();
    if u.starts_with("ipp://") || u.starts_with("ipps://") || u.starts_with("dnssd://") {
        QueueKind::Everywhere
    } else {
        QueueKind::Ppd
    }
}

/// Build this peer's `printers/<host>.json` record.
#[must_use]
pub fn own_record(
    host: &str,
    overlay_ip: &str,
    queues: &[(String, QueueKind)],
    now_ms: u64,
) -> Value {
    let q: Vec<Value> = queues
        .iter()
        .map(|(name, kind)| {
            json!({
                "name": name,
                "kind": kind.as_str(),
                "ipp_path": format!("ipp://{overlay_ip}:{IPP_PORT}/printers/{name}"),
            })
        })
        .collect();
    json!({
        "host": host,
        "overlay_ip": overlay_ip,
        "queues": q,
        "written_at_ms": now_ms,
    })
}

/// A remote queue to import locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportQueue {
    /// Local name: `<queue>@<host>`.
    pub local_name: String,
    /// `ipp://<host-overlay>:631/printers/<queue>`.
    pub uri: String,
    /// Driverless (`everywhere`) vs legacy (replicated PPD).
    pub kind: QueueKind,
    /// Source host (for the replicated-PPD lookup when `kind == Ppd`).
    pub host: String,
    /// Original queue name on the host.
    pub queue: String,
}

/// From the union of peer records (minus self), compute the remote
/// queues to import. `self_host` is excluded.
#[must_use]
pub fn import_plan(self_host: &str, records: &[Value]) -> Vec<ImportQueue> {
    let mut out = Vec::new();
    for rec in records {
        let Some(host) = rec.get("host").and_then(Value::as_str) else {
            continue;
        };
        if host == self_host {
            continue;
        }
        let Some(queues) = rec.get("queues").and_then(Value::as_array) else {
            continue;
        };
        for q in queues {
            let (Some(name), Some(uri)) = (
                q.get("name").and_then(Value::as_str),
                q.get("ipp_path").and_then(Value::as_str),
            ) else {
                continue;
            };
            let kind = match q.get("kind").and_then(Value::as_str) {
                Some("ppd") => QueueKind::Ppd,
                _ => QueueKind::Everywhere,
            };
            out.push(ImportQueue {
                local_name: format!("{name}@{host}"),
                uri: uri.to_string(),
                kind,
                host: host.to_string(),
                queue: name.to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.local_name.cmp(&b.local_name));
    out
}

/// `lpadmin` argv to add/refresh an imported remote queue. Legacy
/// queues get the replicated PPD path (`-P`); everywhere queues use the
/// driverless model (`-m everywhere`).
#[must_use]
pub fn lpadmin_add_argv(q: &ImportQueue, ppd_path: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        q.local_name.clone(),
        "-E".to_string(),
        "-v".to_string(),
        q.uri.clone(),
    ];
    match (q.kind, ppd_path) {
        (QueueKind::Ppd, Some(p)) => {
            argv.push("-P".to_string());
            argv.push(p.to_string());
        }
        _ => {
            argv.push("-m".to_string());
            argv.push("everywhere".to_string());
        }
    }
    argv
}

/// Local `<q>@<host>` queues to delete: any currently-present imported
/// queue not in the desired import set (the host file vanished or the
/// queue was removed upstream). Only `@`-bearing names are candidates —
/// the peer's own local queues are never pruned.
#[must_use]
pub fn prune_list(existing_local: &[String], desired_import: &[String]) -> Vec<String> {
    let desired: BTreeSet<&str> = desired_import.iter().map(String::as_str).collect();
    existing_local
        .iter()
        .filter(|n| n.contains('@') && !desired.contains(n.as_str()))
        .cloned()
        .collect()
}

/// Resolve the fleet defaults record from all peers' `_defaults.json`
/// fragments: highest `written_at_ms` wins (LWW, Q5). Returns `None`
/// when there are no records.
#[must_use]
pub fn resolve_defaults_lww(records: &[Value]) -> Option<Value> {
    records
        .iter()
        .max_by_key(|r| r.get("written_at_ms").and_then(Value::as_u64).unwrap_or(0))
        .cloned()
}

/// Does `cupsd.conf` already listen on the overlay IP? (idempotence
/// guard so the worker only rewrites + reloads on a real change).
#[must_use]
pub fn cupsd_needs_listen(cupsd_conf: &str, overlay_ip: &str) -> bool {
    let listen = format!("Listen {overlay_ip}:{IPP_PORT}");
    !cupsd_conf.lines().any(|l| l.trim() == listen)
}

/// Add the overlay-only `Listen` + an overlay-CIDR `<Location />` allow
/// to a `cupsd.conf`. Binds the Nebula overlay interface only — never
/// `0.0.0.0` (open-mesh directive + §0.7 #10 public-port lint).
#[must_use]
pub fn cupsd_with_listen(cupsd_conf: &str, overlay_ip: &str, overlay_cidr: &str) -> String {
    let block = format!(
        "\n# PRINT-4 (mde cups_sync): share local printers on the Nebula\n\
         # overlay ONLY. Never 0.0.0.0 — enrolled peers reach this via the\n\
         # tunnel; the single mesh passcode is the auth boundary.\n\
         Listen {overlay_ip}:{IPP_PORT}\n\
         <Location />\n  Order allow,deny\n  Allow from {overlay_cidr}\n</Location>\n"
    );
    let mut s = cupsd_conf.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&block);
    s
}

// ───────────────────────── worker body ─────────────────────────

/// Auto CUPS print-sharing worker. One per headless/full peer.
pub struct CupsSyncWorker {
    tick: Duration,
    mesh_home: PathBuf,
    overlay_ip_path: PathBuf,
    hostname: String,
    /// Overlay CIDR allowed in the cupsd `<Location>` (the mesh subnet).
    overlay_cidr: String,
    /// Shelled binaries (injectable for tests).
    lpstat: String,
    lpadmin: String,
    lpoptions: String,
    cupsctl: String,
    /// PRINT-8.b — Bus persist root (`~/.local/share/mde/bus`); `None`
    /// disables the action-responder (unit tests that don't need Bus).
    bus_root: Option<PathBuf>,
    /// Per-verb read cursors for the `action/printers/<verb>` topics.
    action_cursors: HashMap<String, String>,
}

impl CupsSyncWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: mackes_mesh_types::peers::default_mesh_home(),
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            hostname: local_hostname(),
            overlay_cidr: "10.42.0.0/16".to_string(),
            lpstat: "lpstat".to_string(),
            lpadmin: "lpadmin".to_string(),
            lpoptions: "lpoptions".to_string(),
            cupsctl: "cupsctl".to_string(),
            bus_root: default_bus_root(),
            action_cursors: HashMap::new(),
        }
    }

    fn printers_dir(&self) -> PathBuf {
        self.mesh_home.join("printers")
    }

    /// One tick. Guarded no-op when cups/lpadmin absent or unenrolled.
    fn tick_once(&self) {
        if which(&self.lpstat).is_none() || which(&self.lpadmin).is_none() {
            return; // print stack not installed — operator hasn't opted in.
        }
        let Some(overlay_ip) = self.read_overlay_ip() else {
            return; // not enrolled yet — no stable overlay endpoint.
        };
        let dir = self.printers_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return; // mesh-storage not mounted/writable yet.
        }

        // 1. Publish local queues + PPDs (PRINT-2).
        let local = self.local_queues();
        let changed = self.publish_own(&dir, &overlay_ip, &local);

        // 2. Ensure host-side sharing (PRINT-4).
        self.ensure_sharing(&overlay_ip, &local);

        // 3. Import the union + prune (PRINT-3).
        let union = read_peer_records(&dir);
        let plan = import_plan(&self.hostname, &union);
        self.apply_imports(&dir, &plan);
        let desired: Vec<String> = plan.iter().map(|q| q.local_name.clone()).collect();
        for stale in prune_list(&self.installed_queue_names(), &desired) {
            let _ = self.run_lpadmin(&["-x", &stale]);
        }

        // 4. Reconcile defaults + presets LWW (PRINT-5).
        self.reconcile_defaults(&dir);

        // 5. Event publish on a local change (PRINT-8, event half).
        if changed {
            self.publish_event();
        }
    }

    fn read_overlay_ip(&self) -> Option<String> {
        std::fs::read_to_string(&self.overlay_ip_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Local queues with their kind (everywhere vs legacy-PPD).
    fn local_queues(&self) -> Vec<(String, QueueKind)> {
        let names = match self.run_capture(&self.lpstat, &["-e"]) {
            Some(out) => parse_lpstat_e(&out),
            None => return Vec::new(),
        };
        // Drop already-imported `@host` queues — only our own hardware.
        let names: Vec<String> = names.into_iter().filter(|n| !n.contains('@')).collect();
        let uris = self.run_capture(&self.lpstat, &["-v"]).unwrap_or_default();
        names
            .into_iter()
            .map(|n| {
                let kind = parse_device_uri(&uris, &n).map_or(QueueKind::Ppd, |u| queue_kind(&u));
                (n, kind)
            })
            .collect()
    }

    /// Write `printers/<host>.json` + replicate legacy PPDs. Returns
    /// whether the record changed since last tick (drives the event).
    fn publish_own(
        &self,
        dir: &std::path::Path,
        overlay_ip: &str,
        local: &[(String, QueueKind)],
    ) -> bool {
        let rec = own_record(&self.hostname, overlay_ip, local, now_ms());
        let path = dir.join(format!("{}.json", self.hostname));
        let queues_now = serde_json::to_string(&rec["queues"]).unwrap_or_default();
        let prev = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| serde_json::to_string(&v["queues"]).unwrap_or_default())
            .unwrap_or_default();
        let changed = queues_now != prev;
        if let Ok(json) = serde_json::to_string_pretty(&rec) {
            let _ = std::fs::write(&path, json);
        }
        // Replicate each legacy queue's PPD (Q4).
        let ppd_dir = dir.join("ppd").join(&self.hostname);
        let _ = std::fs::create_dir_all(&ppd_dir);
        for (name, kind) in local {
            if *kind == QueueKind::Ppd {
                let src = PathBuf::from(format!("/etc/cups/ppd/{name}.ppd"));
                if src.exists() {
                    let _ = std::fs::copy(&src, ppd_dir.join(format!("{name}.ppd")));
                }
            }
        }
        changed
    }

    /// Ensure cupsd listens on the overlay + shares each local queue.
    fn ensure_sharing(&self, overlay_ip: &str, local: &[(String, QueueKind)]) {
        let conf = PathBuf::from("/etc/cups/cupsd.conf");
        if let Ok(text) = std::fs::read_to_string(&conf) {
            if cupsd_needs_listen(&text, overlay_ip) {
                let next = cupsd_with_listen(&text, overlay_ip, &self.overlay_cidr);
                if std::fs::write(&conf, next).is_ok() {
                    // EFF-20 — bound systemctl reload.
                    let mut cmd = Command::new("systemctl");
                    cmd.args(["reload", "cups.service"]);
                    let _ = crate::workers::proc::status_with_timeout(
                        cmd,
                        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
                    );
                }
            }
        }
        let _ = self.run_capture(&self.cupsctl, &["--share-printers"]);
        for (name, _) in local {
            let _ = self.run_lpadmin(&["-p", name, "-o", "printer-is-shared=true"]);
        }
    }

    fn apply_imports(&self, dir: &std::path::Path, plan: &[ImportQueue]) {
        for q in plan {
            let ppd = if q.kind == QueueKind::Ppd {
                let p = dir
                    .join("ppd")
                    .join(&q.host)
                    .join(format!("{}.ppd", q.queue));
                p.exists().then(|| p.to_string_lossy().to_string())
            } else {
                None
            };
            let argv = lpadmin_add_argv(q, ppd.as_deref());
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let _ = self.run_lpadmin(&refs);
        }
    }

    fn reconcile_defaults(&self, dir: &std::path::Path) {
        let path = dir.join("_defaults.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        // The file holds a single object today; LWW is across historical
        // writers via `written_at_ms` once multiple peers contend.
        let Ok(rec) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let winner = resolve_defaults_lww(std::slice::from_ref(&rec));
        if let Some(d) = winner {
            if let Some(def) = d.get("default_printer").and_then(Value::as_str) {
                // EFF-20 — bound lpoptions.
                let mut cmd = Command::new(&self.lpoptions);
                cmd.args(["-d", def]);
                let _ = crate::workers::proc::status_with_timeout(
                    cmd,
                    crate::workers::proc::DEFAULT_CMD_TIMEOUT,
                );
            }
        }
    }

    fn installed_queue_names(&self) -> Vec<String> {
        self.run_capture(&self.lpstat, &["-e"])
            .map(|o| parse_lpstat_e(&o))
            .unwrap_or_default()
    }

    /// PRINT-8 — announce a local printer change on `event/printers/<host>`
    /// in-process (perf-10 / arch-6). Replaces a raw `mde-bus publish … .spawn()`
    /// (a fork+exec + fresh SQLite open that was never even reaped) with a bare
    /// `Persist::write`. Byte-identical stored row; targets
    /// [`crate::bus_publish::default_bus_root`] (honours `MDE_BUS_ROOT` — the
    /// root the spawned CLI resolved via the inherited env). Best-effort.
    fn publish_event(&self) {
        let topic = format!("event/printers/{}", self.hostname);
        let body = format!(r#"{{"host":"{}","changed":true}}"#, self.hostname);
        if let Some(mut persist) =
            crate::bus_publish::open_bus(crate::bus_publish::default_bus_root())
        {
            crate::bus_publish::publish_body(&mut persist, &topic, &body);
        }
    }

    /// PRINT-8.b — poll `action/printers/{sync-now,list}` for operator
    /// commands + reply on `reply/<ulid>`. Silent no-op when Bus isn't set up.
    fn poll_bus_actions(&mut self) {
        let Some(bus_root) = self.bus_root.clone() else {
            return;
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(_) => return,
        };
        for verb in ACTION_VERBS {
            let topic = format!("action/printers/{verb}");
            let since = self.action_cursors.get(&topic).map(String::as_str);
            let msgs = match persist.list_since(&topic, since) {
                Ok(m) => m,
                Err(_) => continue,
            };
            for msg in msgs {
                self.action_cursors.insert(topic.clone(), msg.ulid.clone());
                let reply_json = self.handle_action(verb);
                let _ = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply_json),
                );
            }
        }
    }

    /// Dispatch one `action/printers/<verb>` message. Returns the JSON
    /// body to write to `reply/<ulid>`. Pure over `&self` so tests can
    /// call it without a running Bus.
    #[must_use]
    pub fn handle_action(&self, verb: &str) -> String {
        match verb {
            "sync-now" => {
                self.tick_once();
                r#"{"ok":true}"#.to_string()
            }
            "list" => {
                let union = read_peer_records(&self.printers_dir());
                serde_json::to_string(&union).unwrap_or_else(|_| "[]".to_string())
            }
            _ => r#"{"error":"unknown verb"}"#.to_string(),
        }
    }

    fn run_lpadmin(&self, args: &[&str]) -> bool {
        // EFF-20 — bound lpadmin so a wedged CUPS can't pin the tick.
        let mut cmd = Command::new(&self.lpadmin);
        cmd.args(args);
        crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn run_capture(&self, bin: &str, args: &[&str]) -> Option<String> {
        // EFF-20 — bound the capture so a wedged CUPS tool can't pin the tick.
        let mut cmd = Command::new(bin);
        cmd.args(args);
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    }
}

impl Default for CupsSyncWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for CupsSyncWorker {
    fn name(&self) -> &'static str {
        "cups_sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.tick_once();
        self.poll_bus_actions();
        loop {
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick) => {
                    self.tick_once();
                    self.poll_bus_actions();
                }
            }
        }
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    // Absolute path → check directly; bare name → scan PATH.
    let p = PathBuf::from(bin);
    if p.is_absolute() {
        return p.exists().then_some(p);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|c| c.exists())
    })
}

fn local_hostname() -> String {
    let cmd = Command::new("hostname");
    crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn read_peer_records(dir: &std::path::Path) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json")
                && p.file_name().is_some_and(|n| n != "_defaults.json")
            {
                if let Ok(v) = std::fs::read_to_string(&p).and_then(|s| {
                    serde_json::from_str::<Value>(&s)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                }) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lpstat_e_lists_queues() {
        assert_eq!(
            parse_lpstat_e("Office\nBackroom\n\n  Label  \n"),
            vec!["Office", "Backroom", "Label"]
        );
    }

    #[test]
    fn device_uri_parsed_and_classified() {
        let out =
            "device for Office: ipp://10.0.0.5:631/ipp/print\ndevice for USBHP: usb://HP/LaserJet";
        assert_eq!(
            parse_device_uri(out, "Office").as_deref(),
            Some("ipp://10.0.0.5:631/ipp/print")
        );
        assert_eq!(
            queue_kind(&parse_device_uri(out, "Office").unwrap()),
            QueueKind::Everywhere
        );
        assert_eq!(
            queue_kind(&parse_device_uri(out, "USBHP").unwrap()),
            QueueKind::Ppd
        );
    }

    #[test]
    fn own_record_shape() {
        let rec = own_record(
            "anvil",
            "10.42.0.7",
            &[("Office".to_string(), QueueKind::Everywhere)],
            123,
        );
        assert_eq!(rec["host"], "anvil");
        assert_eq!(rec["written_at_ms"], 123);
        assert_eq!(rec["queues"][0]["name"], "Office");
        assert_eq!(rec["queues"][0]["kind"], "everywhere");
        assert_eq!(
            rec["queues"][0]["ipp_path"],
            "ipp://10.42.0.7:631/printers/Office"
        );
    }

    #[test]
    fn import_plan_names_at_host_and_excludes_self() {
        let anvil = own_record(
            "anvil",
            "10.42.0.7",
            &[("Office".to_string(), QueueKind::Everywhere)],
            1,
        );
        let forge = own_record(
            "forge",
            "10.42.0.8",
            &[("Lab".to_string(), QueueKind::Ppd)],
            1,
        );
        let plan = import_plan("anvil", &[anvil, forge]);
        // Only forge's queue imported (self excluded).
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].local_name, "Lab@forge");
        assert_eq!(plan[0].uri, "ipp://10.42.0.8:631/printers/Lab");
        assert_eq!(plan[0].kind, QueueKind::Ppd);
    }

    #[test]
    fn lpadmin_argv_everywhere_vs_ppd() {
        let q = ImportQueue {
            local_name: "Lab@forge".into(),
            uri: "ipp://10.42.0.8:631/printers/Lab".into(),
            kind: QueueKind::Everywhere,
            host: "forge".into(),
            queue: "Lab".into(),
        };
        let argv = lpadmin_add_argv(&q, None);
        assert!(argv.windows(2).any(|w| w == ["-m", "everywhere"]));
        let q2 = ImportQueue {
            kind: QueueKind::Ppd,
            ..q
        };
        let argv2 = lpadmin_add_argv(&q2, Some("/mesh/ppd/forge/Lab.ppd"));
        assert!(argv2
            .windows(2)
            .any(|w| w == ["-P", "/mesh/ppd/forge/Lab.ppd"]));
    }

    #[test]
    fn prune_targets_only_stale_at_host_queues() {
        let existing = vec![
            "Office".to_string(),     // local — never pruned
            "Lab@forge".to_string(),  // desired — keep
            "Old@beacon".to_string(), // vanished — prune
        ];
        let desired = vec!["Lab@forge".to_string()];
        assert_eq!(
            prune_list(&existing, &desired),
            vec!["Old@beacon".to_string()]
        );
    }

    #[test]
    fn defaults_lww_highest_timestamp_wins() {
        let a = json!({"default_printer": "Office@anvil", "written_at_ms": 10});
        let b = json!({"default_printer": "Lab@forge", "written_at_ms": 20});
        let w = resolve_defaults_lww(&[a, b]).unwrap();
        assert_eq!(w["default_printer"], "Lab@forge");
    }

    #[test]
    fn cupsd_listen_idempotence_and_overlay_only() {
        let base = "LogLevel warn\nListen localhost:631\n";
        assert!(cupsd_needs_listen(base, "10.42.0.7"));
        let next = cupsd_with_listen(base, "10.42.0.7", "10.42.0.0/16");
        assert!(next.contains("Listen 10.42.0.7:631"));
        assert!(next.contains("Allow from 10.42.0.0/16"));
        assert!(!next.contains("Listen 0.0.0.0"));
        // Second pass: already present → no rewrite needed.
        assert!(!cupsd_needs_listen(&next, "10.42.0.7"));
    }

    // ── PRINT-8.b: handle_action pure dispatch ─────────────────────────────

    fn test_worker() -> CupsSyncWorker {
        CupsSyncWorker {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: PathBuf::from("/nonexistent/mesh-home"),
            overlay_ip_path: PathBuf::from("/nonexistent/overlay-ip"),
            hostname: "testpeer".to_string(),
            overlay_cidr: "10.42.0.0/16".to_string(),
            lpstat: "lpstat".to_string(),
            lpadmin: "lpadmin".to_string(),
            lpoptions: "lpoptions".to_string(),
            cupsctl: "cupsctl".to_string(),
            bus_root: None, // no Bus in unit tests
            action_cursors: HashMap::new(),
        }
    }

    #[test]
    fn handle_action_list_returns_json_array() {
        let w = test_worker();
        let reply = w.handle_action("list");
        // printers dir doesn't exist → empty array, not an error.
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(v.is_array(), "expected array, got: {reply}");
    }

    #[test]
    fn handle_action_sync_now_returns_ok() {
        let w = test_worker();
        // tick_once is a no-op when lpstat/lpadmin are absent or not installed.
        let reply = w.handle_action("sync-now");
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(v["ok"], serde_json::json!(true), "got: {reply}");
    }

    #[test]
    fn handle_action_unknown_verb_returns_error() {
        let w = test_worker();
        let reply = w.handle_action("frobnicate");
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(v["error"].is_string(), "got: {reply}");
    }

    #[test]
    fn action_verbs_are_the_locked_two() {
        assert_eq!(ACTION_VERBS, ["sync-now", "list"]);
    }
}
