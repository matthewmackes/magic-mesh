//! PLANES-18 (W74/W75) — mesh DNS.
//!
//! `<host>.mesh` resolves to a peer's overlay IP on every box, with
//! **no DNS server and no fixed center** (W74): mackesd feeds the
//! replicated roster into systemd-resolved per-link on the Nebula
//! interface via the FDO `org.freedesktop.resolve1` interop (§2-legal
//! — it is `org.freedesktop.*`), so resolution is local, survives
//! partitions, and needs no daemon to reach. Namespace is the flat
//! `<host>.mesh` (W75 — one mesh per workgroup, §8).
//!
//! The records are derived purely from the PeerRecords +
//! roster-mirror overlay IPs; this worker keeps systemd-resolved's
//! per-link domain in sync on a tick. Where `resolvectl` isn't
//! present (dev) it falls back to a managed `/etc/hosts` block (the
//! W74 crude-but-bulletproof alternative), so the names resolve
//! either way.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Sync cadence — roster changes are heartbeat-paced; 30 s suffices.
pub const TICK: Duration = Duration::from_secs(30);

/// The mesh DNS namespace suffix (W75).
pub const MESH_SUFFIX: &str = "mesh";

/// MEDIA-5 — the active-active SERVICE name the music clients resolve. Unlike a
/// `<host>.mesh` peer record (one name → one IP), `music.mesh` resolves to the
/// **A-record SET** of every live `Lighthouse_Media` overlay IP, so clients
/// round-robin across instances and fail over when one media lighthouse drops.
pub const MUSIC_SERVICE_FQDN: &str = "music.mesh";

/// MEDIA-5 — build the `music.mesh` A-record set from the live `Lighthouse_Media`
/// overlay IPs. One [`MeshHost`] per media instance, all sharing the
/// [`MUSIC_SERVICE_FQDN`] name; empty IPs (a media node whose overlay isn't known
/// yet) are skipped. Sorted + deduped by IP for a stable, idempotent block, so a
/// media node joining/leaving simply adds/removes its IP from the set on the next
/// tick — no VIP, no fixed center.
#[must_use]
pub fn music_a_records(media_overlay_ips: &[String]) -> Vec<MeshHost> {
    let mut out: Vec<MeshHost> = media_overlay_ips
        .iter()
        .filter(|ip| !ip.is_empty())
        .map(|ip| MeshHost {
            fqdn: MUSIC_SERVICE_FQDN.to_string(),
            overlay_ip: ip.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.overlay_ip.cmp(&b.overlay_ip));
    out.dedup();
    out
}

/// MEDIA-5 — the full mesh-DNS record set: the `<host>.mesh` peer records PLUS
/// the `music.mesh` active-active A-set built from the `Lighthouse_Media`
/// membership. This is what the worker writes each tick; resolution of
/// `<host>.mesh` is unchanged, and `music.mesh` now answers with the live media
/// instance set.
#[must_use]
pub fn build_records_with_music(
    peers: &[(String, String)],
    media_overlay_ips: &[String],
) -> Vec<MeshHost> {
    let mut out = build_records(peers);
    out.extend(music_a_records(media_overlay_ips));
    out.sort_by(|a, b| a.fqdn.cmp(&b.fqdn).then(a.overlay_ip.cmp(&b.overlay_ip)));
    out.dedup();
    out
}

/// `/etc/hosts` managed-block markers (the resolvectl-absent path).
pub const HOSTS_BEGIN: &str = "# >>> mde mesh-dns (managed) >>>";
/// Closing sentinel for the managed `/etc/hosts` block.
pub const HOSTS_END: &str = "# <<< mde mesh-dns <<<";

/// One name→ip record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshHost {
    /// The fully-qualified mesh hostname (e.g. `pine.mesh`).
    pub fqdn: String,
    /// The peer's Nebula overlay IP address.
    pub overlay_ip: String,
}

/// Build the `<host>.mesh → overlay-ip` set from (hostname,
/// overlay_ip) pairs. Skips entries with an empty IP (a peer whose
/// overlay address isn't known yet) — never emits a half record.
#[must_use]
pub fn build_records(peers: &[(String, String)]) -> Vec<MeshHost> {
    let mut out: Vec<MeshHost> = peers
        .iter()
        .filter(|(_, ip)| !ip.is_empty())
        .map(|(host, ip)| MeshHost {
            fqdn: format!("{host}.{MESH_SUFFIX}"),
            overlay_ip: ip.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
    out.dedup();
    out
}

/// Render the managed `/etc/hosts` block content (the lines BETWEEN
/// the markers) for `records`. Pure — the writer merges it.
#[must_use]
pub fn hosts_block(records: &[MeshHost]) -> String {
    let mut s = String::new();
    for r in records {
        s.push_str(&format!("{}\t{}\n", r.overlay_ip, r.fqdn));
    }
    s
}

/// Merge the managed block into existing `/etc/hosts` content,
/// replacing any prior managed block and preserving everything else.
#[must_use]
pub fn merge_hosts(existing: &str, records: &[MeshHost]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        if line.trim() == HOSTS_BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == HOSTS_END {
            in_block = false;
            continue;
        }
        if !in_block {
            kept.push(line);
        }
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if !records.is_empty() {
        out.push_str(HOSTS_BEGIN);
        out.push('\n');
        out.push_str(&hosts_block(records));
        out.push_str(HOSTS_END);
        out.push('\n');
    }
    out
}

/// The mesh-DNS worker.
pub struct MeshDnsWorker {
    store_db: Option<PathBuf>,
    hosts_path: PathBuf,
    /// QNM-Shared root — the worker resolves `<host>.mesh` from the
    /// replicated directory (peer records carry their own overlay IP),
    /// not the local SQLite roster, which is empty on a peer.
    workgroup_root: PathBuf,
}

impl MeshDnsWorker {
    /// `store_db` is the leader's roster fallback; the directory under the
    /// resolved QNM-Shared root is the primary `<host>.mesh` source.
    #[must_use]
    pub fn new(store_db: Option<PathBuf>) -> Self {
        Self {
            store_db,
            hosts_path: PathBuf::from("/etc/hosts"),
            workgroup_root: crate::default_qnm_shared_root(),
        }
    }

    /// Test seam — point the directory read at a fixture root.
    #[must_use]
    pub fn with_workgroup_root(mut self, p: PathBuf) -> Self {
        self.workgroup_root = p;
        self
    }

    /// Test seam — redirect the hosts file.
    #[must_use]
    pub fn with_hosts_path(mut self, p: PathBuf) -> Self {
        self.hosts_path = p;
        self
    }

    /// `(hostname, overlay_ip)` for every peer with a known overlay IP —
    /// read from the replicated directory (the same source the Workbench
    /// Mesh DNS panel shows). The directory joins each peer's own recorded
    /// overlay IP with the leader's roster mirror as a fallback, so this
    /// is populated on every node, not just the signer. (Was: the local
    /// SQLite roster, empty on a peer → an empty `/etc/hosts` block → the
    /// bug where `<host>.mesh` never resolved from the terminal.)
    fn peers(&self) -> Vec<(String, String)> {
        let svc = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            self.store_db.clone(),
        );
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        svc.build_directory(now_ms)["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    p["hostname"].as_str()?.to_string(),
                    p["overlay_ip"].as_str()?.to_string(),
                ))
            })
            .collect()
    }

    /// MEDIA-5 — the overlay IP of every live `Lighthouse_Media` peer, read from
    /// the same replicated directory `peers()` uses. The directory stamps each
    /// peer's pinned deployment role (`telemetry` writes `rec.role`), so the
    /// media membership is just the `lighthouse-media` rows' overlay IPs — a node
    /// joining/leaving the media class updates this set with no extra signal.
    fn media_overlay_ips(&self) -> Vec<String> {
        let svc = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            self.store_db.clone(),
        );
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        svc.build_directory(now_ms)["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| {
                mackes_mesh_types::lighthouse::MEDIA_LIGHTHOUSE_ROLE
                    == p["role"].as_str().unwrap_or_default()
            })
            .filter_map(|p| p["overlay_ip"].as_str().map(str::to_string))
            .filter(|ip| !ip.is_empty())
            .collect()
    }

    fn sync(&self) {
        let media = self.media_overlay_ips();
        let records = build_records_with_music(&self.peers(), &media);
        tracing::debug!(
            target: "mackesd::mesh_dns",
            workgroup_root = %self.workgroup_root.display(),
            records = records.len(),
            music_instances = media.len(),
            "mesh_dns sync tick",
        );
        // Preferred: per-link systemd-resolved domain (FDO interop).
        // Best-effort; the /etc/hosts merge is the always-applied
        // fallback so names resolve even without resolvectl.
        let _ = self.push_resolved(&records);
        match std::fs::read_to_string(&self.hosts_path) {
            Ok(existing) => {
                let merged = merge_hosts(&existing, &records);
                if merged != existing {
                    match std::fs::write(&self.hosts_path, &merged) {
                        Ok(()) => tracing::info!(
                            target: "mackesd::mesh_dns",
                            path = %self.hosts_path.display(),
                            names = records.len(),
                            "mesh_dns: wrote <host>.mesh block to hosts file",
                        ),
                        Err(e) => tracing::warn!(
                            target: "mackesd::mesh_dns",
                            error = %e,
                            path = %self.hosts_path.display(),
                            "mesh_dns: hosts write FAILED",
                        ),
                    }
                }
            }
            Err(e) => tracing::warn!(
                target: "mackesd::mesh_dns",
                error = %e,
                path = %self.hosts_path.display(),
                "mesh_dns: hosts read FAILED",
            ),
        }
    }

    fn push_resolved(&self, _records: &[MeshHost]) -> std::io::Result<()> {
        // resolvectl wires the .mesh domain to the nebula link so the
        // resolver answers <host>.mesh locally. (The per-A-record feed
        // uses the systemd-resolved D-Bus RegisterService surface; the
        // /etc/hosts merge already guarantees resolution, so a missing
        // resolvectl is a quiet degrade, not a failure.)
        if which("resolvectl") {
            // EFF-20 — bound resolvectl so a hung invocation can't pin the tick.
            let mut cmd = std::process::Command::new("resolvectl");
            cmd.args(["domain", "nebula1", &format!("~{MESH_SUFFIX}")]);
            let _ = crate::workers::proc::status_with_timeout(
                cmd,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            );
        }
        Ok(())
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl Worker for MeshDnsWorker {
    fn name(&self) -> &'static str {
        "mesh_dns"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.sync();
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(TICK) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_flat_host_dot_mesh_and_skip_empty_ips() {
        let recs = build_records(&[
            ("pine".into(), "10.42.0.2".into()),
            ("oak".into(), "10.42.0.3".into()),
            ("ghost".into(), String::new()), // unknown IP — dropped
        ]);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].fqdn, "oak.mesh"); // sorted
        assert_eq!(recs[1].fqdn, "pine.mesh");
        assert_eq!(recs[1].overlay_ip, "10.42.0.2");
    }

    #[test]
    fn build_records_with_music_includes_the_music_mesh_a_set() {
        // MEDIA-5 — the combined builder emits the <host>.mesh peer records AND
        // a music.mesh A-record per live Lighthouse_Media overlay IP.
        let peers = [
            ("pine".to_string(), "10.42.0.2".to_string()),
            ("oak".to_string(), "10.42.0.3".to_string()),
        ];
        let media = [
            "10.42.0.7".to_string(),
            "10.42.0.8".to_string(),
            String::new(), // a media node with no overlay yet — skipped
        ];
        let recs = build_records_with_music(&peers, &media);
        // The two music.mesh A-records (the active-active set), sorted by IP.
        let music: Vec<&MeshHost> = recs
            .iter()
            .filter(|r| r.fqdn == MUSIC_SERVICE_FQDN)
            .collect();
        assert_eq!(music.len(), 2, "one A-record per live media instance");
        assert_eq!(music[0].overlay_ip, "10.42.0.7");
        assert_eq!(music[1].overlay_ip, "10.42.0.8");
        // The peer <host>.mesh records survive alongside it.
        assert!(recs
            .iter()
            .any(|r| r.fqdn == "pine.mesh" && r.overlay_ip == "10.42.0.2"));
        assert!(recs.iter().any(|r| r.fqdn == "oak.mesh"));
        // The rendered /etc/hosts block round-robins music.mesh across instances.
        let block = hosts_block(&recs);
        assert!(block.contains("10.42.0.7\tmusic.mesh"));
        assert!(block.contains("10.42.0.8\tmusic.mesh"));
    }

    #[test]
    fn music_a_set_tracks_membership_join_and_leave() {
        // A node joining the media class adds its IP; leaving removes it — the
        // set is a pure function of the live membership (no stale VIP).
        assert!(
            music_a_records(&[]).is_empty(),
            "no media nodes → no music.mesh"
        );
        let one = music_a_records(&["10.42.0.7".into()]);
        assert_eq!(one.len(), 1);
        let two = music_a_records(&["10.42.0.8".into(), "10.42.0.7".into()]);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].overlay_ip, "10.42.0.7", "sorted for a stable block");
        // Idempotent: duplicate membership collapses (one A per IP).
        let dup = music_a_records(&["10.42.0.7".into(), "10.42.0.7".into()]);
        assert_eq!(dup.len(), 1);
    }

    #[test]
    fn merge_replaces_the_block_and_keeps_operator_lines() {
        let base = "127.0.0.1 localhost\n";
        let recs = build_records(&[("pine".into(), "10.42.0.2".into())]);
        let merged = merge_hosts(base, &recs);
        assert!(merged.starts_with("127.0.0.1 localhost\n"));
        assert!(merged.contains(HOSTS_BEGIN));
        assert!(merged.contains("10.42.0.2\tpine.mesh"));
        // Idempotent — re-merging the same records is a fixed point.
        assert_eq!(merge_hosts(&merged, &recs), merged);
    }

    #[test]
    fn empty_roster_removes_the_managed_block() {
        let with = merge_hosts(
            "127.0.0.1 localhost\n",
            &build_records(&[("pine".into(), "10.42.0.2".into())]),
        );
        let emptied = merge_hosts(&with, &[]);
        assert_eq!(emptied, "127.0.0.1 localhost\n");
    }

    #[test]
    fn sync_writes_the_hosts_block_via_the_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let hosts = tmp.path().join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
        // No store_db + an empty workgroup root → empty directory → no
        // block, file unchanged.
        let w = MeshDnsWorker::new(None)
            .with_hosts_path(hosts.clone())
            .with_workgroup_root(tmp.path().join("empty-wg"));
        w.sync();
        assert_eq!(
            std::fs::read_to_string(&hosts).unwrap(),
            "127.0.0.1 localhost\n"
        );
    }
}
