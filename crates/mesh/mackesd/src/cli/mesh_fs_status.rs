//! `MeshFsStatus` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `mesh-fs-status` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        // MESHFS-2 — aggregate every peer's share usage from the replicated
        // directory; both GUI consumers parse this JSON.
        let report = mesh_fs_report(&mackesd_core::default_qnm_shared_root());
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}

// ── MESHFS-1: Mesh-Sync storage status ────────────────────────────────────────
// The `mesh-fs-status` verb was deleted with the LizardFS plane (SUBSTRATE-6);
// two GUIs still shell it. Restored Syncthing-native. The report is the UNION of
// the fields both consumers read: the Workbench Mesh Storage panel reads
// peers[].{addr,used_bytes,avail_bytes} + goal + quota_cap_bytes +
// limiting_peer_addr; `mde-files` reads master_reachable + peers[].undergoal_chunks
// + goal + offline_peers. Under Syncthing there is no master/chunks, so those
// LizardFS-era fields are honest constants (0 / [] / mount-present), kept in the
// wire shape as MESHFS-2/3 placeholders — never faked.

#[derive(Debug, serde::Serialize)]
struct MeshFsPeer {
    addr: String,
    used_bytes: u64,
    avail_bytes: u64,
    /// LizardFS-era field `mde-files` still reads; always 0 under Syncthing.
    undergoal_chunks: u64,
}

#[derive(Debug, serde::Serialize)]
struct MeshFsReport {
    schema: u32,
    mount: String,
    peers: Vec<MeshFsPeer>,
    goal: u64,
    quota_cap_bytes: Option<u64>,
    limiting_peer_addr: Option<String>,
    /// `mde-files`' healing check; under Syncthing = is the local mount present.
    master_reachable: bool,
    offline_peers: Vec<String>,
    /// MESHFS-3 — Mesh-Sync folder completion percent from Syncthing's REST API
    /// (`None` when Syncthing is unreachable / unprovisioned); 100 = fully
    /// replicated across the mesh.
    sync_completion_pct: Option<f64>,
}

/// MESHFS-2 — aggregate every peer's Mesh-Sync `df` usage from the replicated
/// peer directory under `qnm_root`. Each peer publishes its own usage on the
/// heartbeat (`descriptors.mesh_fs`); a peer that hasn't probed yet (pre-MESHFS-2
/// / `present: false`) is skipped rather than shown as a phantom 0-byte share.
/// Falls back to THIS node's local mount when no peer has published usage yet, so
/// the Mesh Storage panel is never empty on a fresh mesh.
fn mesh_fs_report(qnm_root: &std::path::Path) -> MeshFsReport {
    let mount = std::path::Path::new(mackesd_core::CANONICAL_QNM_MOUNT);
    let records =
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(qnm_root));
    let mut peers: Vec<MeshFsPeer> = records
        .iter()
        .filter_map(|r| {
            let u = r.descriptors.as_ref()?.mesh_fs;
            u.present.then(|| MeshFsPeer {
                addr: r.hostname.clone(),
                used_bytes: u.used_bytes,
                avail_bytes: u.avail_bytes,
                undergoal_chunks: 0,
            })
        })
        .collect();
    peers.sort_by(|a, b| a.addr.cmp(&b.addr));
    if peers.is_empty() {
        // No peer has published mesh_fs yet — report this node's local mount so
        // the panel still shows real data (reuses the heartbeat's own prober).
        let u = mackesd_core::descriptors::probe_mesh_fs();
        if u.present {
            peers.push(MeshFsPeer {
                addr: default_node_id(),
                used_bytes: u.used_bytes,
                avail_bytes: u.avail_bytes,
                undergoal_chunks: 0,
            });
        }
    }
    // full-mesh: every present node holds a copy, so the goal == the peer count.
    let goal = peers.len() as u64;
    // MESHFS-3 — real replication state from Syncthing's REST API (best-effort;
    // None when the daemon/config is absent, never a faked 100%).
    let sync = mackesd_core::syncthing::folder_health();
    MeshFsReport {
        schema: 1,
        mount: mount.display().to_string(),
        peers,
        goal,
        quota_cap_bytes: None,
        limiting_peer_addr: None,
        master_reachable: mount.is_dir(),
        offline_peers: vec![],
        sync_completion_pct: sync.reachable.then_some(sync.completion_pct),
    }
}

#[cfg(test)]
mod meshfs_tests {
    use super::*;
    use mackes_mesh_types::peers::{MeshFsUsage, PeerRecord, ServiceDescriptors};

    fn write_peer(root: &std::path::Path, host: &str, mesh_fs: MeshFsUsage) {
        let mut rec = PeerRecord::now(host, None, "healthy");
        rec.descriptors = Some(ServiceDescriptors {
            mesh_fs,
            ..Default::default()
        });
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join(format!("{host}.json")),
            serde_json::to_string(&rec).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn aggregates_present_peers_from_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        write_peer(
            tmp.path(),
            "anvil",
            MeshFsUsage {
                present: true,
                used_bytes: 100,
                avail_bytes: 900,
            },
        );
        write_peer(
            tmp.path(),
            "forge",
            MeshFsUsage {
                present: true,
                used_bytes: 200,
                avail_bytes: 800,
            },
        );
        // A peer that hasn't probed its mount yet must be SKIPPED, not shown as a
        // phantom 0-byte share.
        write_peer(tmp.path(), "lh", MeshFsUsage::default());
        let r = mesh_fs_report(tmp.path());
        assert_eq!(r.peers.len(), 2, "only present peers aggregate");
        assert_eq!(r.goal, 2);
        // sorted by addr (hostname)
        assert_eq!(r.peers[0].addr, "anvil");
        assert_eq!(r.peers[0].used_bytes, 100);
    }

    #[test]
    fn empty_directory_emits_valid_json_no_false_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No peer records and /mnt/mesh-storage absent on the build host → empty
        // peers, but still a non-empty JSON object (the panel checks stdout).
        let r = mesh_fs_report(tmp.path());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"peers\":"));
        assert!(json.contains("\"goal\":"));
    }
}
