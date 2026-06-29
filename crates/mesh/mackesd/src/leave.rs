//! ENT-5 — `mackesd leave`: the unified, voluntary mesh exit.
//!
//! One verb coordinates what `decommission` (DB soft-delete) and
//! `ca revoke` (trust) never did together, and adds the local
//! teardown neither performed:
//!
//! 1. **Evict our own cert from the data plane** — fingerprint
//!    `/etc/nebula/host.crt` into the replicated `ca/blocklist`
//!    (the ENT-3 machinery), so every peer's nebula drops our
//!    tunnels within a tick.
//! 2. **Leave the roster** — remove our own published files
//!    (PeerRecord, bundle, ssh pubkey, media-registry row): own-row
//!    authority applies to departure too.
//! 3. **Wipe local state** — `/etc/nebula/*`, the published
//!    overlay-ip/role markers, and `role.toml` (the box returns to
//!    the ENT-2 fail-closed unpinned state).
//!
//! Deliberately **no ban**: a ban blocks future enrollment
//! (`sign_pending_csr` refuses banned node-ids), and ENT-5's
//! acceptance is that re-enroll is a clean fresh join. Banning is
//! the hostile-eviction path (`ca revoke` + ban), not goodbye.

use std::path::{Path, PathBuf};

/// What `leave` accomplished — printed by the CLI; every field is
/// honest (false = that step found nothing / failed best-effort).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaveReport {
    /// Own cert fingerprinted into the replicated blocklist.
    pub data_plane_evicted: bool,
    /// Own PeerRecord removed from the replicated roster.
    pub roster_record_removed: bool,
    /// Own bundle removed.
    pub bundle_removed: bool,
    /// Own gossiped SSH pubkey removed.
    pub ssh_key_removed: bool,
    /// `/etc/nebula` contents wiped.
    pub nebula_config_wiped: bool,
    /// `role.toml` removed (box is unpinned again).
    pub role_unpinned: bool,
    /// Own `<host>/media-registry.json` removed from the shared media
    /// plane (MEDIA-7 — present only on a Lighthouse_Media node).
    pub media_registry_removed: bool,
}

/// Execute the voluntary exit. Every step is best-effort and
/// reported; nothing panics on partial state (a half-enrolled box
/// can still leave cleanly).
pub fn leave(
    workgroup_root: &Path,
    hostname: &str,
    node_id: &str,
    nebula_config_dir: &Path,
    role_toml_path: &Path,
) -> LeaveReport {
    let mut report = LeaveReport::default();

    // 1. Data-plane self-eviction (ENT-3 machinery).
    let own_cert = nebula_config_dir.join("host.crt");
    if let Ok(pem) = std::fs::read_to_string(&own_cert) {
        if let Some(fp) = crate::ca::blocklist::fingerprint_cert_pem(&pem) {
            // SEC-6 — sign our own retract when the key store allows.
            report.data_plane_evicted = match crate::node_key::load_or_create(std::path::Path::new(
                crate::node_key::DEFAULT_KEY_PATH,
            )) {
                Ok(key) => crate::ca::blocklist::record_revoked_signed(
                    workgroup_root,
                    node_id,
                    &[fp],
                    node_id,
                    &key,
                )
                .is_ok(),
                Err(_) => {
                    crate::ca::blocklist::record_revoked(workgroup_root, node_id, &[fp]).is_ok()
                }
            };
        }
        if !report.data_plane_evicted {
            tracing::warn!(
                "leave: could not evict own cert from the data plane \
                 (nebula-cert missing?) — peers keep trusting it until expiry"
            );
        }
    }

    // 2. Roster departure — own-row authority.
    let peer_record =
        mackes_mesh_types::peers::peers_dir(workgroup_root).join(format!("{hostname}.json"));
    report.roster_record_removed = std::fs::remove_file(&peer_record).is_ok();
    let bundle = crate::ca::bundle::bundle_path(workgroup_root, node_id);
    report.bundle_removed = std::fs::remove_file(&bundle).is_ok();
    report.ssh_key_removed = std::fs::remove_file(
        workgroup_root
            .join("ssh-keys")
            .join(format!("{hostname}.pub")),
    )
    .is_ok();
    // MEDIA-7 — de-register from the media plane so a torn-down
    // Lighthouse_Media node leaves no stale "up" row behind. Absent on
    // a non-media node (remove_file → false, honestly reported).
    report.media_registry_removed = std::fs::remove_file(
        workgroup_root
            .join(hostname)
            .join(crate::mesh_media::MEDIA_REGISTRY_FILE),
    )
    .is_ok();

    // 3. Local teardown.
    report.nebula_config_wiped = wipe_dir_contents(nebula_config_dir);
    report.role_unpinned = std::fs::remove_file(role_toml_path).is_ok();

    report
}

/// Remove every entry inside `dir` (not the dir itself). `true` when
/// the dir existed and is empty afterwards.
fn wipe_dir_contents(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut all_gone = true;
    for e in entries.filter_map(Result::ok) {
        let p: PathBuf = e.path();
        let removed = if p.is_dir() {
            std::fs::remove_dir_all(&p).is_ok()
        } else {
            std::fs::remove_file(&p).is_ok()
        };
        all_gone &= removed;
    }
    all_gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leave_tears_down_roster_bundle_ssh_config_and_role() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Seed an enrolled-looking box.
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("pine.json"), "{}").unwrap();
        let bpath = crate::ca::bundle::bundle_path(root, "peer:pine");
        std::fs::create_dir_all(bpath.parent().unwrap()).unwrap();
        std::fs::write(&bpath, "{}").unwrap();
        std::fs::create_dir_all(root.join("ssh-keys")).unwrap();
        std::fs::write(root.join("ssh-keys/pine.pub"), "ssh-ed25519 X").unwrap();
        std::fs::create_dir_all(root.join("pine")).unwrap();
        std::fs::write(root.join("pine/media-registry.json"), "{}").unwrap();
        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();
        std::fs::write(nebula.join("config.yaml"), "x").unwrap();
        std::fs::write(nebula.join("host.key"), "secret").unwrap();
        let role = tmp.path().join("role.toml");
        std::fs::write(&role, "role = \"workstation\"\n").unwrap();

        let report = leave(root, "pine", "peer:pine", &nebula, &role);

        assert!(report.roster_record_removed && !pdir.join("pine.json").exists());
        assert!(report.bundle_removed && !bpath.exists());
        assert!(report.ssh_key_removed);
        assert!(
            report.media_registry_removed && !root.join("pine/media-registry.json").exists(),
            "MEDIA-7: media-registry row pruned on leave"
        );
        assert!(report.nebula_config_wiped);
        assert!(!nebula.join("host.key").exists(), "keys must not survive");
        assert!(
            report.role_unpinned && !role.exists(),
            "back to fail-closed"
        );
        // No host.crt seeded → no fingerprint → eviction honestly false.
        assert!(!report.data_plane_evicted);
    }

    #[test]
    fn leave_never_bans_so_reenroll_stays_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();
        let _ = leave(
            tmp.path(),
            "pine",
            "peer:pine",
            &nebula,
            &tmp.path().join("role.toml"),
        );
        assert!(
            !crate::ca::ban_list::is_banned(tmp.path(), "peer:pine"),
            "ENT-5: leave is goodbye, not a ban — re-enroll must be a clean fresh join"
        );
    }

    #[test]
    fn leave_on_a_bare_box_reports_all_false_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let report = leave(
            tmp.path(),
            "ghost",
            "peer:ghost",
            &tmp.path().join("nope"),
            &tmp.path().join("role.toml"),
        );
        assert_eq!(report, LeaveReport::default());
    }
}
