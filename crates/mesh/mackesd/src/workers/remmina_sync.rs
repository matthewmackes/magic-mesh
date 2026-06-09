//! RETIRE-PY.2 — native Remmina mesh-peer sync worker.
//!
//! Replaces `python3 -m mackes.remmina_sync`. Every 60 s the worker reads
//! the mesh peer registry (each peer's `<mesh-home>/peers/<hostname>.json`
//! `PeerRecord`), TCP-probes every peer for SSH/RDP/VNC, and reconciles
//! Remmina's `Mesh Peers` group: one `.remmina` entry per (peer, open
//! protocol). Managed entries (those whose `group=Mesh Peers`) are
//! added/updated/deleted to match the live targets; **entries outside that
//! group are never touched** (the locked Q4 cleanup rule). Running twice with
//! the same input is a no-op (idempotent).
//!
//! Design locked by the v1.x 5-question survey (2026-05-17): probe :22/:3389/
//! :5900; SSH auth via the mesh key (`~/.ssh/mackes_mesh_ed25519`, pubkey);
//! RDP/VNC leave blank credential fields for Remmina's keyring. The python's
//! enable/disable + systemd-unit + tweaks-toggle surface was the *UI/CLI* path
//! (System → Tweaks); the supervised worker only ever ran one `sync()` per
//! tick, which is all this port carries.
//!
//! **Connect host:** `PeerRecord` keys on `hostname` (own-row authority, no IP
//! field), so the `.remmina` `server` targets the peer hostname — resolvable on
//! the Nebula overlay / mesh DNS. (A future refinement could map hostname → the
//! peer's Nebula IP via the lighthouse roster; hostname is the available,
//! correct-by-default target today.)

#![cfg(feature = "async-services")]

use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mackes_mesh_types::peers::{default_mesh_home, peers_dir, read_peers};
use tracing::{debug, warn};

use super::{ShutdownToken, Worker};

/// Cadence locked at 60 s per the legacy `mackes-remmina-sync.timer`.
pub const TICK_INTERVAL_S: u64 = 60;

/// The Remmina group folder this worker owns end-to-end. Files whose
/// `[remmina] group` differs are never deleted or rewritten.
const MACKES_GROUP: &str = "Mesh Peers";
/// Marker key written into every managed `.remmina` file.
const MACKES_TAG: &str = "X-Mackes-Managed";
/// Per-connection TCP probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// `(proto-key, port, Remmina protocol string)`.
const PROTOCOLS: &[(&str, u16, &str)] = &[
    ("ssh", 22, "SSH"),
    ("rdp", 3389, "RDP"),
    ("vnc", 5900, "VNC"),
];

/// A mesh peer with its open-port probe results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerProbe {
    /// Short display label (first hostname label).
    pub name: String,
    /// Connect target (the peer hostname).
    pub host: String,
    /// Port 22 accepted a connection.
    pub ssh: bool,
    /// Port 3389 accepted a connection.
    pub rdp: bool,
    /// Port 5900 accepted a connection.
    pub vnc: bool,
}

impl PeerProbe {
    fn open(&self, proto: &str) -> bool {
        match proto {
            "ssh" => self.ssh,
            "rdp" => self.rdp,
            "vnc" => self.vnc,
            _ => false,
        }
    }
}

/// What one `reconcile()` pass changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Filenames of newly created entries.
    pub added: Vec<String>,
    /// Filenames whose content was rewritten (drift).
    pub updated: Vec<String>,
    /// Filenames removed (peer/protocol no longer live).
    pub deleted: Vec<String>,
    /// Filenames already up to date (no write).
    pub skipped: Vec<String>,
    /// How many peers were probed this pass.
    pub peers_probed: usize,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("+{} new", self.added.len()));
        }
        if !self.updated.is_empty() {
            parts.push(format!("~{} updated", self.updated.len()));
        }
        if !self.deleted.is_empty() {
            parts.push(format!("-{} removed", self.deleted.len()));
        }
        if parts.is_empty() {
            parts.push("no changes".to_string());
        }
        write!(
            f,
            "Remmina sync: {} peer(s) probed · {}",
            self.peers_probed,
            parts.join(" · ")
        )
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn remmina_dir() -> PathBuf {
    home().join(".local/share/remmina")
}

/// This host's name (the peer row to exclude — no Remmina entry to self).
fn local_hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Lowercase; collapse non-alphanumeric runs to a single `-`; trim `-`.
fn safe_slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn file_for(dir: &Path, peer: &PeerProbe, proto: &str) -> PathBuf {
    dir.join(format!(
        "mackes-mesh-{}-{proto}.remmina",
        safe_slug(&peer.name)
    ))
}

/// Render a minimal `.remmina` INI for one `(peer, protocol)`. `home_dir` +
/// `user` are injected so the output is deterministic (and testable).
fn render(
    peer: &PeerProbe,
    proto: &str,
    port: u16,
    rproto: &str,
    home_dir: &Path,
    user: &str,
) -> String {
    // Proto-specific credential block: SSH uses the mesh key (pubkey auth);
    // RDP/VNC leave blank fields for Remmina's keyring to fill on first use.
    let creds = if proto == "ssh" {
        format!(
            "ssh_username={user}\nssh_auth=3\nssh_privatekey={}\n",
            home_dir.join(".ssh/mackes_mesh_ed25519").display()
        )
    } else {
        "username=\npassword=\n".to_string()
    };
    format!(
        "[remmina]\n\
         group={MACKES_GROUP}\n\
         {MACKES_TAG}=1\n\
         name={} ({})\n\
         server={}:{port}\n\
         protocol={rproto}\n\
         {creds}",
        peer.name,
        proto.to_uppercase(),
        peer.host,
    )
}

/// True when `host:port` accepts a TCP connection within `PROBE_TIMEOUT`.
fn tcp_open(host: &str, port: u16) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok() {
            return true;
        }
    }
    false
}

/// Probe a peer's SSH/RDP/VNC ports.
fn probe_peer(host: &str) -> (bool, bool, bool) {
    (
        tcp_open(host, 22),
        tcp_open(host, 3389),
        tcp_open(host, 5900),
    )
}

/// The live mesh peers (excluding self), each TCP-probed. Empty when the mesh
/// home / peers dir is absent (no mesh → no entries, never an error).
fn current_peers() -> Vec<PeerProbe> {
    let me = local_hostname();
    let dir = peers_dir(&default_mesh_home());
    let mut out = Vec::new();
    for rec in read_peers(&dir) {
        if rec.hostname.is_empty() || rec.hostname == me {
            continue;
        }
        let name = rec
            .hostname
            .split('.')
            .next()
            .unwrap_or(&rec.hostname)
            .to_string();
        let (ssh, rdp, vnc) = probe_peer(&rec.hostname);
        out.push(PeerProbe {
            name,
            host: rec.hostname,
            ssh,
            rdp,
            vnc,
        });
    }
    out
}

/// Read the `[remmina] group` value from a `.remmina` file's text, if any.
fn file_group(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("group") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// Existing `.remmina` files in `dir` whose group is `MACKES_GROUP`.
fn existing_managed(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("remmina") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if file_group(&text).as_deref() == Some(MACKES_GROUP) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Reconcile the `Mesh Peers` group in `dir` against `peers`. Pure I/O against
/// the given directory so it is fully testable; `home_dir`/`user` feed `render`.
fn reconcile(
    dir: &Path,
    peers: &[PeerProbe],
    home_dir: &Path,
    user: &str,
) -> std::io::Result<SyncReport> {
    let mut report = SyncReport {
        peers_probed: peers.len(),
        ..SyncReport::default()
    };
    fs::create_dir_all(dir)?;

    // Desired (path → content).
    let mut targets: Vec<(PathBuf, String)> = Vec::new();
    for peer in peers {
        for &(proto, port, rproto) in PROTOCOLS {
            if !peer.open(proto) {
                continue;
            }
            targets.push((
                file_for(dir, peer, proto),
                render(peer, proto, port, rproto, home_dir, user),
            ));
        }
    }

    // Add / update / skip.
    for (path, desired) in &targets {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        match fs::read_to_string(path) {
            Ok(existing) if existing == *desired => report.skipped.push(name),
            Ok(_) => {
                fs::write(path, desired)?;
                report.updated.push(name);
            }
            Err(_) => {
                fs::write(path, desired)?;
                report.added.push(name);
            }
        }
    }

    // Delete managed files no longer targeted.
    let target_paths: std::collections::BTreeSet<&PathBuf> =
        targets.iter().map(|(p, _)| p).collect();
    for path in existing_managed(dir) {
        if !target_paths.contains(&path) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if fs::remove_file(&path).is_ok() {
                report.deleted.push(name);
            }
        }
    }
    Ok(report)
}

/// One full sync pass: read live peers, reconcile the local Remmina group.
/// Blocking (TCP probes + file I/O) — call from `spawn_blocking`.
fn sync_once() -> std::io::Result<SyncReport> {
    let user = std::env::var("USER").unwrap_or_default();
    reconcile(&remmina_dir(), &current_peers(), &home(), &user)
}

/// Async worker: run `sync_once` on a 60 s tick until shutdown. Native Rust —
/// no `python3` is ever spawned (RETIRE-PY.2).
pub struct RemminaSyncWorker {
    interval: Duration,
}

impl Default for RemminaSyncWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl RemminaSyncWorker {
    /// Construct the worker with the locked 60 s tick cadence.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            interval: Duration::from_secs(TICK_INTERVAL_S),
        }
    }
}

#[async_trait::async_trait]
impl Worker for RemminaSyncWorker {
    fn name(&self) -> &'static str {
        "remmina-sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                biased;
                () = shutdown.wait() => return Ok(()),
                _ = tick.tick() => {
                    match tokio::task::spawn_blocking(sync_once).await {
                        Ok(Ok(report)) => debug!(target: "remmina_sync", "{report}"),
                        Ok(Err(e)) => warn!(target: "remmina_sync", "sync failed: {e}"),
                        Err(e) => warn!(target: "remmina_sync", "sync task panicked: {e}"),
                    }
                }
            }
        }
    }
}

/// Build the supervisor-ready worker (call site in `run_serve`).
#[must_use]
pub const fn build() -> RemminaSyncWorker {
    RemminaSyncWorker::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mde-remmina-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn peer(name: &str, ssh: bool, rdp: bool, vnc: bool) -> PeerProbe {
        PeerProbe {
            name: name.to_string(),
            host: format!("{name}.mesh"),
            ssh,
            rdp,
            vnc,
        }
    }

    #[test]
    fn worker_name_matches_phase_b_lock() {
        assert_eq!(build().name(), "remmina-sync");
        assert_eq!(TICK_INTERVAL_S, 60);
    }

    #[test]
    fn safe_slug_lowercases_and_dashes() {
        assert_eq!(safe_slug("Laptop MM"), "laptop-mm");
        assert_eq!(safe_slug("host.local"), "host-local");
        assert_eq!(safe_slug("--A__b--"), "a-b");
    }

    #[test]
    fn render_ssh_uses_mesh_key_pubkey_auth() {
        let p = peer("box", true, false, false);
        let out = render(&p, "ssh", 22, "SSH", Path::new("/home/u"), "u");
        assert!(out.contains("group=Mesh Peers\n"));
        assert!(out.contains("X-Mackes-Managed=1\n"));
        assert!(out.contains("name=box (SSH)\n"));
        assert!(out.contains("server=box.mesh:22\n"));
        assert!(out.contains("protocol=SSH\n"));
        assert!(out.contains("ssh_username=u\n"));
        assert!(out.contains("ssh_auth=3\n"));
        assert!(out.contains("ssh_privatekey=/home/u/.ssh/mackes_mesh_ed25519\n"));
    }

    #[test]
    fn render_rdp_leaves_blank_credentials() {
        let p = peer("box", false, true, false);
        let out = render(&p, "rdp", 3389, "RDP", Path::new("/home/u"), "u");
        assert!(out.contains("server=box.mesh:3389\n"));
        assert!(out.contains("protocol=RDP\n"));
        assert!(out.contains("username=\n"));
        assert!(out.contains("password=\n"));
        assert!(!out.contains("ssh_privatekey"));
    }

    #[test]
    fn tcp_open_is_false_for_closed_port() {
        // 127.0.0.1:1 is reserved/closed.
        assert!(!tcp_open("127.0.0.1", 1));
    }

    #[test]
    fn tcp_open_is_true_for_a_listening_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_open("127.0.0.1", port));
    }

    #[test]
    fn file_group_parses_the_group_key() {
        assert_eq!(
            file_group("[remmina]\ngroup=Mesh Peers\n").as_deref(),
            Some("Mesh Peers")
        );
        assert_eq!(
            file_group("group = Mesh Peers").as_deref(),
            Some("Mesh Peers")
        );
        assert_eq!(file_group("name=x\nprotocol=SSH").as_deref(), None);
    }

    #[test]
    fn reconcile_adds_one_entry_per_open_protocol() {
        let dir = scratch("add");
        let peers = vec![peer("alpha", true, true, false)]; // ssh + rdp open, vnc closed
        let r = reconcile(&dir, &peers, Path::new("/home/u"), "u").unwrap();
        assert_eq!(r.added.len(), 2);
        assert_eq!(r.peers_probed, 1);
        assert!(dir.join("mackes-mesh-alpha-ssh.remmina").is_file());
        assert!(dir.join("mackes-mesh-alpha-rdp.remmina").is_file());
        assert!(!dir.join("mackes-mesh-alpha-vnc.remmina").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_is_idempotent_second_run_skips() {
        let dir = scratch("idem");
        let peers = vec![peer("alpha", true, false, false)];
        let _ = reconcile(&dir, &peers, Path::new("/home/u"), "u").unwrap();
        let r2 = reconcile(&dir, &peers, Path::new("/home/u"), "u").unwrap();
        assert!(r2.added.is_empty());
        assert!(r2.updated.is_empty());
        assert_eq!(r2.skipped.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_deletes_stale_managed_entries() {
        let dir = scratch("del");
        let before = vec![peer("alpha", true, false, false)];
        let _ = reconcile(&dir, &before, Path::new("/home/u"), "u").unwrap();
        // alpha goes away → its managed entry is removed.
        let r = reconcile(&dir, &[], Path::new("/home/u"), "u").unwrap();
        assert_eq!(r.deleted.len(), 1);
        assert!(!dir.join("mackes-mesh-alpha-ssh.remmina").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_never_touches_files_outside_our_group() {
        let dir = scratch("foreign");
        let foreign = dir.join("my-laptop.remmina");
        fs::write(&foreign, "[remmina]\ngroup=Personal\nname=Laptop\n").unwrap();
        // A managed file that should be deleted (no peers), plus the foreign one.
        fs::write(
            dir.join("mackes-mesh-old-ssh.remmina"),
            "[remmina]\ngroup=Mesh Peers\nname=old (SSH)\n",
        )
        .unwrap();
        let r = reconcile(&dir, &[], Path::new("/home/u"), "u").unwrap();
        assert_eq!(r.deleted.len(), 1); // only the managed one
        assert!(foreign.is_file()); // foreign untouched
        assert_eq!(
            fs::read_to_string(&foreign).unwrap(),
            "[remmina]\ngroup=Personal\nname=Laptop\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
