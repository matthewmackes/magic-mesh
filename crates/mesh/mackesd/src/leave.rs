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
//! 3. **Wipe local state** — `/etc/nebula/*`, the authenticated relay
//!    authority pin (plus its private signing key on a Lighthouse), the
//!    published overlay-ip/role markers, and `role.toml` (the box returns
//!    to the ENT-2 fail-closed unpinned state).
//!
//! Deliberately **no ban**: a ban blocks future enrollment
//! (`sign_pending_csr` refuses banned node-ids), and ENT-5's
//! acceptance is that re-enroll is a clean fresh join. Banning is
//! the hostile-eviction path (`ca revoke` + ban), not goodbye.

use std::path::{Path, PathBuf};

/// What `leave` accomplished — printed by the CLI. Trust-material removal
/// records failures separately so an already-absent file remains idempotent
/// while an unsafe or failed unlink cannot be reported as a successful leave.
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
    /// Enrollment-authenticated relay authority pin removed.
    pub relay_trust_authority_pin_removed: bool,
    /// The relay authority pin could not be safely removed or confirmed absent.
    /// Idempotent absence leaves this `false`.
    pub relay_trust_authority_pin_removal_failed: bool,
    /// Lighthouse-only relay authority private key removed.
    pub relay_trust_authority_key_removed: bool,
    /// The Lighthouse relay authority private key could not be safely removed
    /// or confirmed absent. Idempotent absence leaves this `false`.
    pub relay_trust_authority_key_removal_failed: bool,
    /// `role.toml` removed (box is unpinned again).
    pub role_unpinned: bool,
    /// Own `<host>/media-registry.json` removed from the shared media
    /// plane (MEDIA-7 — present only on a Lighthouse_Media node).
    pub media_registry_removed: bool,
}

/// Execute the voluntary exit. Every step is attempted and reported; nothing
/// panics on partial state (a half-enrolled box can still leave cleanly). The
/// CLI treats a reported trust-material removal failure as fatal.
pub fn leave(
    workgroup_root: &Path,
    hostname: &str,
    node_id: &str,
    nebula_config_dir: &Path,
    role_toml_path: &Path,
) -> LeaveReport {
    leave_with_relay_authority_paths(
        workgroup_root,
        hostname,
        node_id,
        nebula_config_dir,
        role_toml_path,
        Path::new(crate::ca::bundle::RELAY_TRUST_AUTHORITY_PIN_PATH),
        Path::new(crate::ca::bundle::RELAY_TRUST_AUTHORITY_KEY_PATH),
    )
}

fn leave_with_relay_authority_paths(
    workgroup_root: &Path,
    hostname: &str,
    node_id: &str,
    nebula_config_dir: &Path,
    role_toml_path: &Path,
    relay_trust_authority_pin_path: &Path,
    relay_trust_authority_key_path: &Path,
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
    // Remove the private signer first. If teardown is interrupted between the
    // two unlinks, the old public pin still fails closed instead of leaving an
    // unpinned old-mesh signing key active on a former Lighthouse.
    let relay_key_removal = remove_local_trust_material(
        relay_trust_authority_key_path,
        "relay trust authority private key",
    );
    report.relay_trust_authority_key_removed = relay_key_removal.removed;
    report.relay_trust_authority_key_removal_failed = relay_key_removal.failed;
    let relay_pin_removal =
        remove_local_trust_material(relay_trust_authority_pin_path, "relay trust authority pin");
    report.relay_trust_authority_pin_removed = relay_pin_removal.removed;
    report.relay_trust_authority_pin_removal_failed = relay_pin_removal.failed;
    report.role_unpinned = std::fs::remove_file(role_toml_path).is_ok();

    report
}

/// Unlink one active local trust anchor without following the final path or a
/// symlink substituted for its parent directory. The parent descriptor anchors
/// the unlink across rename races; a final symlink is itself removed, never its
/// target. Missing material is an honest `false`, matching the other report
/// fields, while unexpected filesystem failures are logged without secret
/// contents. `removed=false, failed=false` means the material was already
/// absent and is therefore an idempotent success.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TrustMaterialRemoval {
    removed: bool,
    failed: bool,
}

fn remove_local_trust_material(path: &Path, description: &'static str) -> TrustMaterialRemoval {
    use rustix::fs::{AtFlags, Mode, OFlags};

    let Some(parent) = path.parent() else {
        tracing::warn!(path = %path.display(), "leave: {description} path has no parent");
        return TrustMaterialRemoval {
            removed: false,
            failed: true,
        };
    };
    let Some(file_name) = path.file_name() else {
        tracing::warn!(path = %path.display(), "leave: {description} path has no filename");
        return TrustMaterialRemoval {
            removed: false,
            failed: true,
        };
    };
    let directory = match rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(rustix::io::Errno::NOENT) => return TrustMaterialRemoval::default(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "leave: refusing unsafe or unavailable parent for {description}"
            );
            return TrustMaterialRemoval {
                removed: false,
                failed: true,
            };
        }
    };
    match rustix::fs::unlinkat(&directory, file_name, AtFlags::empty()) {
        Ok(()) => {
            let directory: std::fs::File = directory.into();
            let failed = if let Err(error) = directory.sync_all() {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "leave: removed {description}, but could not sync its parent directory"
                );
                true
            } else {
                false
            };
            TrustMaterialRemoval {
                removed: true,
                failed,
            }
        }
        Err(rustix::io::Errno::NOENT) => TrustMaterialRemoval::default(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "leave: could not remove {description}"
            );
            TrustMaterialRemoval {
                removed: false,
                failed: true,
            }
        }
    }
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

    fn relay_bundle(authority: &str) -> crate::ca::bundle::NebulaBundle {
        crate::ca::bundle::NebulaBundle {
            mesh_id: "mesh-test".into(),
            epoch: 1,
            ca_cert_pem: "ca".into(),
            peer_cert_pem: "peer".into(),
            overlay_ip: "10.42.0.9".into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: Vec::new(),
            relay_trust_authority: Some(authority.into()),
            created_at: 1,
        }
    }

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
        let relay_pin = tmp.path().join("state/relay-trust-authority.pub");
        let relay_key = tmp.path().join("state/relay-trust-authority.ed25519");

        let report = leave_with_relay_authority_paths(
            root,
            "pine",
            "peer:pine",
            &nebula,
            &role,
            &relay_pin,
            &relay_key,
        );

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
        assert!(!report.relay_trust_authority_pin_removed);
        assert!(!report.relay_trust_authority_key_removed);
        assert!(!report.relay_trust_authority_pin_removal_failed);
        assert!(!report.relay_trust_authority_key_removal_failed);
    }

    #[test]
    fn leave_never_bans_so_reenroll_stays_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();
        let _ = leave_with_relay_authority_paths(
            tmp.path(),
            "pine",
            "peer:pine",
            &nebula,
            &tmp.path().join("role.toml"),
            &tmp.path().join("state/relay-trust-authority.pub"),
            &tmp.path().join("state/relay-trust-authority.ed25519"),
        );
        assert!(
            !crate::ca::ban_list::is_banned(tmp.path(), "peer:pine"),
            "ENT-5: leave is goodbye, not a ban — re-enroll must be a clean fresh join"
        );
    }

    #[test]
    fn leave_on_a_bare_box_reports_all_false_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let report = leave_with_relay_authority_paths(
            tmp.path(),
            "ghost",
            "peer:ghost",
            &tmp.path().join("nope"),
            &tmp.path().join("role.toml"),
            &tmp.path().join("state/relay-trust-authority.pub"),
            &tmp.path().join("state/relay-trust-authority.ed25519"),
        );
        assert_eq!(report, LeaveReport::default());
    }

    #[test]
    fn leave_clears_relay_trust_material_and_allows_a_different_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let relay_pin = state.join("relay-trust-authority.pub");
        let relay_key = state.join("relay-trust-authority.ed25519");
        let old_bundle = relay_bundle(&"11".repeat(32));
        let new_bundle = relay_bundle(&"22".repeat(32));
        crate::ca::bundle::write_relay_trust_authority_pin(&old_bundle, &relay_pin)
            .expect("seed old authority pin");
        crate::ca::seal::write_sealed(&relay_key, &[7_u8; 32])
            .expect("seed lighthouse authority key");

        let error = crate::ca::bundle::write_relay_trust_authority_pin(&new_bundle, &relay_pin)
            .expect_err("join must not replace an active authority pin");
        assert!(error.to_string().contains("refusing to replace"));

        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();
        let report = leave_with_relay_authority_paths(
            tmp.path(),
            "pine",
            "peer:pine",
            &nebula,
            &tmp.path().join("role.toml"),
            &relay_pin,
            &relay_key,
        );

        assert!(report.relay_trust_authority_key_removed);
        assert!(report.relay_trust_authority_pin_removed);
        assert!(!relay_key.exists());
        assert!(!relay_pin.exists());
        crate::ca::bundle::write_relay_trust_authority_pin(&new_bundle, &relay_pin)
            .expect("fresh join may pin a different authenticated authority after leave");
        assert!(crate::ca::bundle::relay_trust_authority_matches_pin(
            &new_bundle,
            &relay_pin
        ));
    }

    #[test]
    fn relay_trust_teardown_unlinks_leaf_symlinks_without_following_targets() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let pin_target = tmp.path().join("pin-target");
        let key_target = tmp.path().join("key-target");
        std::fs::write(&pin_target, "keep-pin").unwrap();
        std::fs::write(&key_target, "keep-key").unwrap();
        let relay_pin = state.join("relay-trust-authority.pub");
        let relay_key = state.join("relay-trust-authority.ed25519");
        symlink(&pin_target, &relay_pin).unwrap();
        symlink(&key_target, &relay_key).unwrap();
        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();

        let report = leave_with_relay_authority_paths(
            tmp.path(),
            "pine",
            "peer:pine",
            &nebula,
            &tmp.path().join("role.toml"),
            &relay_pin,
            &relay_key,
        );

        assert!(report.relay_trust_authority_pin_removed);
        assert!(report.relay_trust_authority_key_removed);
        assert!(!report.relay_trust_authority_pin_removal_failed);
        assert!(!report.relay_trust_authority_key_removal_failed);
        assert_eq!(std::fs::read_to_string(pin_target).unwrap(), "keep-pin");
        assert_eq!(std::fs::read_to_string(key_target).unwrap(), "keep-key");
    }

    #[test]
    fn relay_trust_teardown_refuses_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let real_state = tmp.path().join("real-state");
        std::fs::create_dir_all(&real_state).unwrap();
        let relay_pin = real_state.join("relay-trust-authority.pub");
        std::fs::write(&relay_pin, "keep-pin").unwrap();
        let linked_state = tmp.path().join("linked-state");
        symlink(&real_state, &linked_state).unwrap();
        let safe_state = tmp.path().join("safe-state");
        std::fs::create_dir_all(&safe_state).unwrap();
        let nebula = tmp.path().join("etc-nebula");
        std::fs::create_dir_all(&nebula).unwrap();

        let report = leave_with_relay_authority_paths(
            tmp.path(),
            "pine",
            "peer:pine",
            &nebula,
            &tmp.path().join("role.toml"),
            &linked_state.join("relay-trust-authority.pub"),
            &safe_state.join("relay-trust-authority.ed25519"),
        );

        assert!(!report.relay_trust_authority_pin_removed);
        assert!(report.relay_trust_authority_pin_removal_failed);
        assert!(!report.relay_trust_authority_key_removed);
        assert!(!report.relay_trust_authority_key_removal_failed);
        assert_eq!(std::fs::read_to_string(relay_pin).unwrap(), "keep-pin");
    }
}
