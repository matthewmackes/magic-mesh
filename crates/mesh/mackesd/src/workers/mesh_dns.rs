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

/// MEDIA-5 — the well-known active-active service name for the
/// `Lighthouse_Media` Navidrome set.
///
/// `music.mesh` resolves to the overlay IPs of EVERY live media-lighthouse at
/// once (an A-record SET, not a single host), so a client reaches whichever
/// instance answers — the "run this on every `Lighthouse_Media`; clients reach
/// any instance via `music.mesh`" contract from `setup-media-navidrome.sh`.
/// Until now this was only a comment there; this worker makes it a served
/// record.
pub const MUSIC_FQDN: &str = "music.mesh";

/// MEDIA-LIGHTHOUSE — deterministic single-writer alias for Navidrome state that
/// is instance-local (notably playlists). `music.mesh` stays active-active for
/// browse/stream reads; `music-writer.mesh` points at one stable media lighthouse
/// so writes do not split across per-instance SQLite databases.
pub const MUSIC_WRITER_FQDN: &str = "music-writer.mesh";

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

/// MEDIA-5 — the active-active `music.mesh` A-record SET from the live
/// `Lighthouse_Media` overlay IPs (MEDIA-1 discovers them: a peer is a media
/// node iff `is_media_lighthouse`, surfaced as the directory row's `media`
/// flag). Every supplied IP becomes a `music.mesh` record so the name
/// resolves to the WHOLE media set at once — a client reaches any live
/// Navidrome instance (the active-active contract). Empty IPs are skipped
/// (a media-lighthouse whose overlay address isn't known yet → never a half
/// record), and the set is sorted + deduped for a stable, idempotent block.
/// Returns an empty set when there are no media-lighthouses, so the
/// `music.mesh` name is served only where the service actually exists (§7).
#[must_use]
pub fn build_music_records(media_overlay_ips: &[String]) -> Vec<MeshHost> {
    let mut out: Vec<MeshHost> = media_overlay_ips
        .iter()
        .filter(|ip| !ip.is_empty())
        .map(|ip| MeshHost {
            fqdn: MUSIC_FQDN.to_string(),
            overlay_ip: ip.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.overlay_ip.cmp(&b.overlay_ip));
    out.dedup();
    out
}

/// MEDIA-LIGHTHOUSE — choose one deterministic Navidrome writer endpoint from the
/// media-lighthouse set. The lowest overlay IP wins so every node derives the
/// same `music-writer.mesh` record from the replicated directory without a new
/// election surface. Empty IPs are skipped and duplicates collapse.
#[must_use]
pub fn build_music_writer_record(media_overlay_ips: &[String]) -> Vec<MeshHost> {
    let mut ips: Vec<String> = media_overlay_ips
        .iter()
        .filter(|ip| !ip.is_empty())
        .cloned()
        .collect();
    ips.sort_by(|a, b| {
        match (
            a.parse::<std::net::Ipv4Addr>(),
            b.parse::<std::net::Ipv4Addr>(),
        ) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        }
    });
    ips.dedup();
    let Some(overlay_ip) = ips.into_iter().next() else {
        return Vec::new();
    };
    vec![MeshHost {
        fqdn: MUSIC_WRITER_FQDN.to_string(),
        overlay_ip,
    }]
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

    /// The directory snapshot — the single replicated source (the same one the
    /// Workbench Mesh DNS panel shows) both the `<host>.mesh` join AND the
    /// MEDIA-5 `music.mesh` set read from, so one `build_directory` call feeds
    /// both (no second read, no divergence). The directory joins each peer's own
    /// recorded overlay IP with the leader's roster mirror as a fallback, so it
    /// is populated on every node, not just the signer. (Was: the local SQLite
    /// roster, empty on a peer → an empty `/etc/hosts` block → the bug where
    /// `<host>.mesh` never resolved from the terminal.)
    fn directory(&self) -> serde_json::Value {
        let svc = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            self.store_db.clone(),
        );
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        svc.build_directory(now_ms)
    }

    fn sync(&self) {
        let dir = self.directory();
        // The flat <host>.mesh join, plus the MEDIA-5 active-active
        // music.mesh set and deterministic music-writer.mesh alias — all derived
        // from the SAME directory snapshot and
        // merged into one record list so they share the existing /etc/hosts +
        // resolved emit path (no new writer surface).
        let mut records = build_records(&directory_records(&dir));
        let media_ips = media_overlay_ips(&dir);
        let music = build_music_records(&media_ips);
        let music_count = music.len();
        records.extend(music);
        let writer = build_music_writer_record(&media_ips);
        let writer_count = writer.len();
        records.extend(writer);
        tracing::debug!(
            target: "mackesd::mesh_dns",
            workgroup_root = %self.workgroup_root.display(),
            records = records.len(),
            music_records = music_count,
            music_writer_records = writer_count,
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

/// `(hostname, overlay_ip)` for every directory peer — the source for the
/// flat `<host>.mesh` join. Shared by the worker and the `mackesd dns` CLI so
/// both read the directory snapshot the same way.
#[must_use]
pub fn directory_records(dir: &serde_json::Value) -> Vec<(String, String)> {
    dir["peers"]
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

/// MEDIA-5 — the overlay IPs of every live `Lighthouse_Media` in the directory
/// snapshot, the membership of the `music.mesh` active-active set. A row is a
/// media node iff its `media` flag is set — exactly MEDIA-1's
/// `is_media_lighthouse` predicate, surfaced into the directory JSON by
/// `directory_row` (media = a `media`-tagged genuine lighthouse). Rows
/// without an overlay IP yet are dropped here so `build_music_records` only
/// ever sees real addresses.
#[must_use]
pub fn media_overlay_ips(dir: &serde_json::Value) -> Vec<String> {
    dir["peers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["media"].as_bool() == Some(true))
        .filter_map(|p| p["overlay_ip"].as_str().map(str::to_string))
        .collect()
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

    #[test]
    fn music_record_set_is_active_active_over_every_media_ip() {
        // MEDIA-5 — music.mesh is a record SET: one record per live media
        // overlay IP, all under the single `music.mesh` name (active-active).
        let recs = build_music_records(&[
            "10.42.0.5".into(),
            "10.42.0.2".into(),
            String::new(),      // a media-LH with no overlay IP yet — dropped
            "10.42.0.5".into(), // a dup (two reads of the same node) — collapsed
        ]);
        assert_eq!(recs.len(), 2, "empties dropped, dups collapsed");
        assert!(recs.iter().all(|r| r.fqdn == MUSIC_FQDN));
        // Sorted by IP for a stable, idempotent /etc/hosts block.
        assert_eq!(recs[0].overlay_ip, "10.42.0.2");
        assert_eq!(recs[1].overlay_ip, "10.42.0.5");
    }

    #[test]
    fn music_writer_record_chooses_one_stable_media_ip() {
        // MEDIA-LIGHTHOUSE — playlist writes go to one deterministic alias while
        // reads keep the active-active `music.mesh` record set.
        let recs = build_music_writer_record(&[
            "10.42.0.5".into(),
            "10.42.0.2".into(),
            String::new(),
            "10.42.0.5".into(),
        ]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].fqdn, MUSIC_WRITER_FQDN);
        assert_eq!(recs[0].overlay_ip, "10.42.0.2");
    }

    #[test]
    fn no_media_lighthouses_means_no_music_record() {
        // §7 — the name is served only where the service exists: zero media
        // nodes → an empty set → no `music.mesh` line in the hosts block.
        assert!(build_music_records(&[]).is_empty());
        assert!(build_music_writer_record(&[]).is_empty());
        let merged = merge_hosts("127.0.0.1 localhost\n", &build_music_records(&[]));
        assert_eq!(merged, "127.0.0.1 localhost\n");
    }

    #[test]
    fn media_overlay_ips_reads_only_the_media_flagged_rows() {
        // MEDIA-1's `media` flag is the membership gate (surfaced into the
        // directory JSON by directory_row); a plain lighthouse / peer is out.
        let dir = serde_json::json!({ "peers": [
            { "hostname": "media-a", "overlay_ip": "10.42.0.2", "media": true },
            { "hostname": "media-b", "overlay_ip": "10.42.0.5", "media": true },
            { "hostname": "plain-lh", "overlay_ip": "10.42.0.9", "media": false },
            { "hostname": "peer-1",   "overlay_ip": "10.42.0.7" }, // no media key
        ]});
        let ips = media_overlay_ips(&dir);
        assert_eq!(ips, vec!["10.42.0.2".to_string(), "10.42.0.5".to_string()]);
    }

    #[test]
    fn sync_serves_music_mesh_from_a_live_media_lighthouse() {
        // End-to-end through the worker: a replicated media-lighthouse record
        // lands `music.mesh -> its overlay IP` in the hosts file, alongside its
        // own `<host>.mesh` record — both via the existing merge path.
        use mackes_mesh_types::peers::{peers_dir, write_peer_record, PeerRecord};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wg");
        let pdir = peers_dir(&root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut rec = PeerRecord::now("anvil", Some("11.0".into()), "healthy");
        rec.last_seen_ms = now;
        rec.overlay_ip = Some("10.42.0.42".into());
        rec.role = Some("lighthouse".into());
        rec.media = true; // a genuine Lighthouse_Media (MEDIA-1)
        write_peer_record(&pdir, &rec).unwrap();

        let hosts = tmp.path().join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
        let w = MeshDnsWorker::new(None)
            .with_hosts_path(hosts.clone())
            .with_workgroup_root(root);
        w.sync();
        let written = std::fs::read_to_string(&hosts).unwrap();
        // The media node resolves both names to its overlay IP.
        assert!(
            written.contains("10.42.0.42\tanvil.mesh"),
            "the <host>.mesh record still emits: {written}"
        );
        assert!(
            written.contains("10.42.0.42\tmusic.mesh"),
            "music.mesh is now a served active-active record: {written}"
        );
        assert!(
            written.contains("10.42.0.42\tmusic-writer.mesh"),
            "music-writer.mesh points at the deterministic writer: {written}"
        );
        // Idempotent — a second tick is a fixed point (no churn).
        w.sync();
        assert_eq!(std::fs::read_to_string(&hosts).unwrap(), written);
    }
}
