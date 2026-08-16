//! `Ca` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;
use std::io::Read;

/// Keep stdin imports under the same ceiling as the existing bounded file
/// reader. The limit is applied to bytes before UTF-8 materialization or
/// dearmor, so a hostile pipe cannot grow an unbounded `String` first.
const MAX_IMPORT_ARMOR_BYTES: u64 = mackesd_core::ca::seal::MAX_SEALED_FILE_BYTES;

fn read_armored_stdin() -> anyhow::Result<String> {
    read_armored_reader(std::io::stdin().lock())
}

fn read_armored_reader<R: Read>(reader: R) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_IMPORT_ARMOR_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("import: reading armored bundle from stdin")?;
    if bytes.len() as u64 > MAX_IMPORT_ARMOR_BYTES {
        anyhow::bail!("import: armored bundle from stdin exceeds {MAX_IMPORT_ARMOR_BYTES}-byte limit; provide a smaller bundle and retry");
    }
    String::from_utf8(bytes).context("import: armored bundle from stdin is not valid UTF-8")
}

/// Handle the `ca` subcommand.
#[allow(unreachable_code)]
pub fn run(sub: CaCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // NF-2.6 (v2.5) — mackesd ca {mint, rotate, list,
        // dump-ca} subcommands. Operator surface backing the
        // CA module.
        let mut conn = mackesd_core::store::open(&db_path)?;
        let default_mesh = format!("mesh-{}", default_node_id());
        match sub {
            CaCmd::Mint { mesh_id } => {
                let mesh = mesh_id.unwrap_or(default_mesh);
                match mackesd_core::ca::mint::mint_ca(
                    &mackesd_core::ca::SubprocessBackend,
                    &conn,
                    &mesh,
                    None,
                    None,
                ) {
                    Ok(mackesd_core::ca::mint::MintOutcome::Created { .. }) => {
                        println!("CA minted at epoch 0 for mesh '{mesh}'.");
                    }
                    Ok(mackesd_core::ca::mint::MintOutcome::AlreadyMinted { epoch, .. }) => {
                        println!("CA for mesh '{mesh}' already exists at epoch {epoch} (no-op).");
                    }
                    Err(mackesd_core::ca::CaError::BinaryMissing) => {
                        return Err(anyhow::anyhow!(
                            "nebula-cert not on PATH. Install the Fedora `nebula` package + retry."
                        ));
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("mint: {e}"));
                    }
                }
            }
            CaCmd::SetPassphrase => {
                let root = mackesd_core::default_qnm_shared_root();
                let new = std::env::var("MDE_CA_PASSPHRASE").map_err(|_| {
                    anyhow::anyhow!("set-passphrase: export MDE_CA_PASSPHRASE first")
                })?;
                if new.len() < 8 {
                    anyhow::bail!("set-passphrase: at least 8 characters (SEC-2)");
                }
                use mackesd_core::ca::rotation_gate::{verify, GateCheck};
                if verify(&root, "") != GateCheck::NotSet {
                    let current = std::env::var("MDE_CA_PASSPHRASE_CURRENT").unwrap_or_default();
                    if verify(&root, &current) != GateCheck::Ok {
                        anyhow::bail!(
                            "set-passphrase: a gate exists — export the current phrase \
                                 in MDE_CA_PASSPHRASE_CURRENT to change it"
                        );
                    }
                }
                mackesd_core::ca::rotation_gate::set_passphrase(&root, &new)?;
                println!("CA-rotation passphrase set (SEC-2 gate armed).");
                return Ok(());
            }
            CaCmd::Rotate {
                mesh_id,
                passphrase_stdin,
            } => {
                // SEC-2 — the gate, before any rotation work.
                let root = mackesd_core::default_qnm_shared_root();
                let phrase = if passphrase_stdin {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim_end_matches('\n').to_string()
                } else {
                    std::env::var("MDE_CA_PASSPHRASE").unwrap_or_default()
                };
                let check = mackesd_core::ca::rotation_gate::verify(&root, &phrase);
                if let Some(msg) = mackesd_core::ca::rotation_gate::refusal_message(check) {
                    anyhow::bail!("{msg}");
                }
                let mesh = mesh_id.unwrap_or(default_mesh);
                match mackesd_core::ca::epoch::bump_epoch(
                    &mackesd_core::ca::SubprocessBackend,
                    &mut conn,
                    &mesh,
                    None,
                    None,
                ) {
                    Ok(o) => {
                        println!(
                                "CA rotated for mesh '{mesh}': epoch {} → {} ({} peer certs re-signed).",
                                o.retired_epoch
                                    .map(|e| e.to_string())
                                    .unwrap_or_else(|| "none".into()),
                                o.new_epoch,
                                o.re_signed,
                            );
                    }
                    Err(mackesd_core::ca::CaError::BinaryMissing) => {
                        return Err(anyhow::anyhow!(
                            "nebula-cert not on PATH. Install the Fedora `nebula` package + retry."
                        ));
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("rotate: {e}"));
                    }
                }
            }
            CaCmd::List => {
                let mut stmt = conn.prepare(
                    "SELECT mesh_id, epoch, created_at, retired_at \
                         FROM nebula_ca ORDER BY mesh_id, epoch DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                })?;
                println!(
                    "{:<24} {:>6} {:>12} {:>12}",
                    "MESH_ID", "EPOCH", "CREATED", "RETIRED"
                );
                let mut count = 0;
                for row in rows {
                    let (mesh, epoch, created, retired) = row?;
                    let retired_disp = match retired {
                        Some(t) => t.to_string(),
                        None => "active".to_string(),
                    };
                    println!("{mesh:<24} {epoch:>6} {created:>12} {retired_disp:>12}",);
                    count += 1;
                }
                if count == 0 {
                    println!("(no CAs minted yet — run `mackesd ca mint`)");
                }
            }
            CaCmd::DumpCa { mesh_id } => {
                let mesh = mesh_id.unwrap_or(default_mesh);
                match mackesd_core::ca::mint::current_ca(&conn, &mesh) {
                    Ok(Some((_epoch, pem))) => {
                        print!("{pem}");
                    }
                    Ok(None) => {
                        return Err(anyhow::anyhow!("no active CA for mesh '{mesh}'"));
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("dump-ca: {e}"));
                    }
                }
            }
            CaCmd::Export {
                mesh_id,
                passphrase_stdin,
                output,
                ca_key,
            } => {
                // NF-18.1 — encrypted CA backup. EFF-21: prefer
                // --passphrase-stdin (env is environ-visible +
                // child-inherited); env stays the fallback.
                let mesh = mesh_id.unwrap_or(default_mesh);
                let passphrase = if passphrase_stdin {
                    read_secret_line("export")?
                } else {
                    std::env::var("MDE_BACKUP_PASSPHRASE").map_err(|_| {
                        anyhow::anyhow!(
                            "export: pass --passphrase-stdin (preferred) or set \
                                 MDE_BACKUP_PASSPHRASE before invoking"
                        )
                    })?
                };
                let key_path = ca_key.unwrap_or_else(|| {
                    mackesd_core::nebula_enroll::SignCsrPaths::production_defaults().ca_key
                });
                let ca_key_pem = mackesd_core::ca::seal::read_sealed(&key_path).map_err(|e| {
                    anyhow::anyhow!("export: read CA key {}: {e}", key_path.display(),)
                })?;
                let ca_key_pem_str = String::from_utf8(ca_key_pem)
                    .map_err(|e| anyhow::anyhow!("export: CA key not UTF-8: {e}"))?;
                let plaintext =
                    mackesd_core::ca::backup::assemble_from_store(&conn, &mesh, &ca_key_pem_str)
                        .map_err(|e| anyhow::anyhow!("export: assemble: {e}"))?;
                let sealed = mackesd_core::ca::backup::seal(&passphrase, &plaintext)
                    .map_err(|e| anyhow::anyhow!("export: seal: {e}"))?;
                let armored = mackesd_core::ca::backup::armor(&sealed, plaintext.exported_at);
                match output {
                    Some(path) => {
                        // Backups contain the CA private key after decryption;
                        // publish them through the sealed atomic writer so a
                        // crash cannot leave a partial archive and an
                        // operator-selected symlink cannot redirect the write.
                        mackesd_core::ca::seal::write_atomic_sealed(&path, armored.as_bytes())
                            .map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
                        eprintln!(
                            "exported {} CA rows + {} peer certs → {} ({} bytes armored)",
                            plaintext.ca_certs.len(),
                            plaintext.peer_certs.len(),
                            path.display(),
                            armored.len(),
                        );
                    }
                    None => {
                        print!("{armored}");
                    }
                }
            }
            CaCmd::Import {
                input,
                passphrase_stdin,
            } => {
                // NF-18.1 — encrypted CA bundle restore. EFF-21:
                // --passphrase-stdin preferred (requires --input,
                // since the default bundle source is stdin).
                let passphrase = if passphrase_stdin {
                    read_secret_line("import")?
                } else {
                    std::env::var("MDE_BACKUP_PASSPHRASE").map_err(|_| {
                        anyhow::anyhow!(
                            "import: pass --passphrase-stdin with --input \
                                 (preferred) or set MDE_BACKUP_PASSPHRASE"
                        )
                    })?
                };
                let armored = match input {
                    Some(path) => {
                        let bytes = mackesd_core::ca::seal::read_no_follow(&path)
                            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
                        String::from_utf8(bytes).with_context(|| {
                            format!("read {}: input is not UTF-8", path.display())
                        })?
                    }
                    None => read_armored_stdin()?,
                };
                let sealed = mackesd_core::ca::backup::dearmor(&armored)
                    .map_err(|e| anyhow::anyhow!("import: dearmor: {e}"))?;
                let plaintext = mackesd_core::ca::backup::unseal(&passphrase, &sealed)
                    .map_err(|e| anyhow::anyhow!("import: {e}"))?;
                mackesd_core::ca::backup::restore_to_store(&conn, &plaintext)
                    .map_err(|e| anyhow::anyhow!("import: restore: {e}"))?;
                eprintln!(
                    "imported {} CA rows + {} peer certs for mesh '{}' \
                         (exported_at = unix:{}); restart mackesd to pick up \
                         the new CA + the operator should re-write \
                         /etc/nebula/{{ca.crt,ca.key}} from the bundle.",
                    plaintext.ca_certs.len(),
                    plaintext.peer_certs.len(),
                    plaintext.mesh_id,
                    plaintext.exported_at,
                );
            }
            CaCmd::SignCsr {
                node_id,
                workgroup_root,
                mesh_id,
                ca_crt,
                ca_key,
                scratch_dir,
                lighthouse_addr,
                override_cap,
            } => {
                // NF-3.6.b — sign the peer's pending-enroll
                // CSR + write the bundle back to QNM-Shared.
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let mesh = mesh_id.unwrap_or(default_mesh);
                let mut paths = mackesd_core::nebula_enroll::SignCsrPaths::production_defaults();
                if let Some(p) = ca_crt {
                    paths.ca_crt = p;
                }
                if let Some(p) = ca_key {
                    paths.ca_key = p;
                }
                if let Some(p) = scratch_dir {
                    paths.scratch_dir = p;
                }
                // Bug #6: the joining peer must dial the lighthouse's
                // REAL external address. Resolution order:
                //   1. an explicit `--lighthouse-addr` override, else
                //   2. inherit the lighthouse's own roster (the real
                //      overlay_ip + external_addr mesh-init recorded)
                //      from its own bundle, else
                //   3. last-resort hostname guess (NOT DNS-resolvable
                //      for the peer — the old default that broke joins).
                let local_id = default_node_id();
                let lighthouses = if let Some(addr) = lighthouse_addr {
                    let self_bundle = mackesd_core::ca::bundle::read_bundle(
                        &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &local_id),
                    );
                    let relay_tls = self_bundle.as_ref().ok().and_then(|bundle| {
                        bundle
                            .relay_trust_authority
                            .as_deref()
                            .and_then(|authority| {
                                mackesd_core::ca::bundle::advertised_relay_tls_identity(
                                    &workgroup_root,
                                    &local_id,
                                    "10.42.0.1",
                                    &addr,
                                    authority,
                                )
                            })
                    });
                    vec![mackesd_core::ca::bundle::LighthouseEntry {
                        node_id: local_id.clone(),
                        overlay_ip: "10.42.0.1".to_string(),
                        external_addr: addr,
                        relay_tls,
                    }]
                } else {
                    // LIGHTHOUSE-10 — no explicit --lighthouse-addr: build the
                    // FULL roster from the canonical directory (etcd-first),
                    // self-included, so a manually-signed peer learns EVERY
                    // lighthouse (parity with the /enroll listener + auto-
                    // signer), not just this signer's own bundle. Self overlay
                    // = live nebula1 IP; self external = persisted lighthouse
                    // addr, else this node's own bundle entry, else a hostname
                    // guess (the legacy last resort kept for a pre-heartbeat
                    // founder).
                    let self_overlay = mackesd_core::voip_rtt::own_nebula_ip()
                        .unwrap_or_else(|| "10.42.0.1".to_string());
                    let self_bundle = mackesd_core::ca::bundle::read_bundle(
                        &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &local_id),
                    );
                    let self_external = mackesd_core::lighthouse_addr::read_external_addr()
                        .or_else(|| {
                            self_bundle.as_ref().ok().and_then(|b| {
                                b.lighthouses
                                    .iter()
                                    .find(|l| l.node_id == local_id)
                                    .or_else(|| b.lighthouses.first())
                                    .map(|l| l.external_addr.clone())
                            })
                        })
                        .unwrap_or_else(|| {
                            let host = std::fs::read_to_string("/etc/hostname")
                                .ok()
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(default_node_id);
                            eprintln!(
                                "mackesd ca sign-csr: no persisted external-addr or \
                                     lighthouse bundle — falling back to hostname \
                                     '{host}:4242', which the peer may not resolve. Pass \
                                     --lighthouse-addr <public-ip>:4242."
                            );
                            format!("{host}:4242")
                        });
                    let directory = mackesd_core::substrate::peers::read_directory(&workgroup_root);
                    let relay_authority = self_bundle
                        .as_ref()
                        .ok()
                        .and_then(|bundle| bundle.relay_trust_authority.as_deref());
                    mackes_mesh_types::lighthouse::roster_with_self(
                        &directory,
                        &local_id,
                        &self_overlay,
                        &self_external,
                    )
                    .into_iter()
                    .map(|a| {
                        mackesd_core::ca::bundle::lighthouse_entry_with_relay_trust(
                            &workgroup_root,
                            a.node_id,
                            a.overlay_ip,
                            a.external_addr,
                            relay_authority,
                        )
                    })
                    .collect()
                };
                match mackesd_core::nebula_enroll::sign_pending_csr(
                    &mackesd_core::ca::SubprocessBackend,
                    &conn,
                    &workgroup_root,
                    &node_id,
                    &mesh,
                    &paths,
                    lighthouses,
                    override_cap,
                ) {
                    Ok(outcome) => {
                        if override_cap {
                            eprintln!(
                                "TUNE-11 OVERRIDE ENGAGED: signed {} past the {}-peer cap. \
                                     Audit-log entry written to the journal under \
                                     `mackesd::cap_override`. Document the exception in \
                                     docs/design/cap-overrides.md.",
                                outcome.peer_id,
                                mackesd_core::ca::sign::MAX_PEER_CAP,
                            );
                        }
                        println!(
                            "signed {} into mesh '{}' at epoch {} (overlay {}); bundle at {}.",
                            outcome.peer_id,
                            mesh,
                            outcome.epoch,
                            outcome.overlay_ip,
                            outcome.bundle_path.display(),
                        );
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("sign-csr: {e}"));
                    }
                }
            }
            CaCmd::Revoke {
                node_id,
                workgroup_root,
                self_node_id,
            } => {
                // INST-7 prerequisite — revoke a peer's cert +
                // ban the identity. CLI surface replaces the
                // originally-planned D-Bus method (D-Bus retires
                // by 1.0 per AI_GOVERNANCE §3.3).
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let self_id = self_node_id.unwrap_or_else(default_node_id);
                let rows = revoke_with_authority(
                    &conn,
                    &workgroup_root,
                    &self_id,
                    &node_id,
                )?;
                println!(
                    "revoked '{node_id}': {rows} cert row(s) marked revoked; \
                         added to ban list at {self_id}'s QNM-Shared entry."
                );
            }
            CaCmd::Ban {
                node_id,
                workgroup_root,
            } => {
                // EPIC-SEC-BANLIST (Q53) — add node-id to this
                // peer's ban list. GFS replication propagates it.
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let self_id = default_node_id();
                let generation = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs().max(1))
                    .unwrap_or(1);
                let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                    schema_version: 1,
                    request_id: format!("ca-ban-{node_id}-{generation}"),
                    target_id: node_id.clone(),
                    intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                    generation,
                    steps: vec!["offboard".into()],
                };
                let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                    &workgroup_root,
                    plan,
                )
                .map_err(|error| anyhow::anyhow!("cannot acquire CA ban authority: {error:?}"))?;
                let mut added = None;
                let result = authority.run_next(|_| {
                    added = Some(
                        mackesd_core::ca::ban_list::add_banned(
                            &workgroup_root,
                            &self_id,
                            &node_id,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                    Ok(())
                });
                if let Err(error) = result {
                    let _ = authority.finish();
                    return Err(anyhow::anyhow!("CA ban lifecycle failure: {error:?}"));
                }
                authority.finish().map_err(|error| {
                    anyhow::anyhow!("cannot release CA ban authority: {error:?}")
                })?;
                match added.ok_or_else(|| anyhow::anyhow!("CA ban returned no result"))? {
                    true => println!(
                        "banned '{node_id}' (recorded in {}'s ban list; \
                             propagates to every peer via mesh-storage).",
                        self_id
                    ),
                    false => println!("'{node_id}' was already banned (no-op)."),
                }
            }
            CaCmd::Unban {
                node_id,
                workgroup_root,
            } => {
                // EPIC-SEC-BANLIST (Q53) — lift a ban THIS peer
                // set. Bans set on other peers must be lifted
                // there (the gate enforces the union).
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let self_id = default_node_id();
                let generation = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs().max(1))
                    .unwrap_or(1);
                let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                    schema_version: 1,
                    request_id: format!("ca-unban-{node_id}-{generation}"),
                    target_id: node_id.clone(),
                    intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::VerifyAndCorrect,
                    generation,
                    steps: vec!["identity".into()],
                };
                let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                    &workgroup_root,
                    plan,
                )
                .map_err(|error| anyhow::anyhow!("cannot acquire CA unban authority: {error:?}"))?;
                let mut removed = None;
                let result = authority.run_next(|_| {
                    removed = Some(
                        mackesd_core::ca::ban_list::remove_banned(
                            &workgroup_root,
                            &self_id,
                            &node_id,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                    Ok(())
                });
                if let Err(error) = result {
                    let _ = authority.finish();
                    return Err(anyhow::anyhow!("CA unban lifecycle failure: {error:?}"));
                }
                authority.finish().map_err(|error| {
                    anyhow::anyhow!("cannot release CA unban authority: {error:?}")
                })?;
                match removed.ok_or_else(|| anyhow::anyhow!("CA unban returned no result"))? {
                    true => println!("unbanned '{node_id}' from {self_id}'s ban list."),
                    false => {
                        // Still surface the union state so the
                        // operator knows if another peer banned it.
                        if mackesd_core::ca::ban_list::is_banned(&workgroup_root, &node_id) {
                            println!(
                                "'{node_id}' isn't in {self_id}'s ban list, but ANOTHER \
                                     peer still bans it — unban it on that peer too."
                            );
                        } else {
                            println!("'{node_id}' isn't banned (no-op).");
                        }
                    }
                }
            }
            CaCmd::BanList { workgroup_root } => {
                // EPIC-SEC-BANLIST (Q53) — print the enforced
                // union across every peer's ban list.
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let union = mackesd_core::ca::ban_list::load_union(&workgroup_root);
                if union.is_empty() {
                    println!("ban list empty (no node-ids banned across the mesh).");
                } else {
                    println!("Banned node-ids (mesh-wide union, {} total):", union.len());
                    for id in &union {
                        println!("  {id}");
                    }
                }
            }
        }
    }
    Ok(())
}

fn revoke_with_authority(
    conn: &rusqlite::Connection,
    workgroup_root: &std::path::Path,
    self_id: &str,
    node_id: &str,
) -> anyhow::Result<u32> {
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .unwrap_or(1);
    let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
        schema_version: 1,
        request_id: format!("ca-revoke-{node_id}-{generation}"),
        target_id: node_id.to_string(),
        intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
        generation,
        steps: vec!["offboard".into()],
    };
    let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(workgroup_root, plan)
        .map_err(|error| anyhow::anyhow!("cannot acquire CA revoke authority: {error:?}"))?;
    let mut rows = None;
    let result = authority.run_next(|_| {
        rows = Some(
            mackesd_core::ca::revoke::revoke_peer(conn, workgroup_root, self_id, node_id)
                .context("ca revoke")
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    });
    if let Err(error) = result {
        let _ = authority.finish();
        return Err(anyhow::anyhow!("CA revoke lifecycle failure: {error:?}"));
    }
    authority
        .finish()
        .map_err(|error| anyhow::anyhow!("cannot release CA revoke authority: {error:?}"))?;
    rows.ok_or_else(|| anyhow::anyhow!("CA revoke authority completed without a result"))
}

#[cfg(test)]
mod tests {
    use super::{read_armored_reader, MAX_IMPORT_ARMOR_BYTES};
    use std::io::Cursor;

    #[test]
    fn armored_stdin_reader_preserves_normal_bundle_text() {
        let bundle = "-----BEGIN MACKES NEBULA CA EXPORT-----\nsmall\n";

        let read = read_armored_reader(Cursor::new(bundle.as_bytes()))
            .expect("normal armored bundle should be read unchanged");

        assert_eq!(read, bundle);
    }

    #[test]
    fn armored_stdin_reader_rejects_oversized_bundle_before_dearmor() {
        let oversized = vec![b'x'; (MAX_IMPORT_ARMOR_BYTES + 1) as usize];

        let error = read_armored_reader(Cursor::new(oversized))
            .expect_err("oversized stdin bundle must fail closed");
        let message = error.to_string();
        assert!(message.contains("armored bundle from stdin"));
        assert!(message.contains("exceeds"));
        assert!(message.contains(&MAX_IMPORT_ARMOR_BYTES.to_string()));
    }
}
