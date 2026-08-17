//! Peer + lighthouse node-lifecycle CLI verb handlers
//! (`add-peer`, `remove-peer`, `lighthouse add`, `lighthouse retire`).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved. `mint_join_token` is the shared token-minting core
//! of `add-peer` + `lighthouse add`, kept private to this module.
use crate::*;

/// The only DigitalOcean lighthouse shape. Keep the CLI gate aligned with the
/// join helper, Datacenter HCL writer, and Tofu variable validation so an
/// invalid override cannot mint a bearer before the provisioner rejects it.
const THIN_LIGHTHOUSE_SIZE: &str = "s-1vcpu-1gb";

/// SETUP-4/5 — mint a single-use v3 join token for a new peer/lighthouse on
/// THIS lighthouse. Reads the mesh-id from the local founding bundle and the
/// `?fp=` from the on-disk `/enroll` endpoint cert, mints a fresh bearer, and
/// prints the ready-to-paste token + join line. `role` only shapes the printed
/// guidance (the joining box pins its own role); add-lighthouse is `--role
/// lighthouse`.
pub fn add_peer(
    role: &str,
    note: &str,
    lighthouse: Option<String>,
    enroll_port: Option<u16>,
) -> anyhow::Result<()> {
    let parsed: mde_role::Role = role
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation"))?;
    let token = mint_join_token(parsed, note, lighthouse, enroll_port)?;
    println!("{token}");
    eprintln!(
        "single-use v3 token minted (SETUP-5) for a {} — run on the joining box:\n  \
         magic-setup   (Join → paste it)\n  or:  mackesd join '{token}' --role {}",
        parsed.as_str(),
        parsed.as_str()
    );
    Ok(())
}

/// #13/#5 — mint a single-use **v3** join token for a new peer/lighthouse on THIS
/// lighthouse: the shared core of `add-peer` (which prints it) and `lighthouse add`
/// (which feeds it to the join provisioner). Reads mesh-id from the founding
/// bundle, pins the on-disk `/enroll` endpoint cert fingerprint, and — for the
/// LIGHTHOUSE role — scopes the bearer note (#12) so the joiner is delivered the CA
/// key + a Host cert (a full signing lighthouse); any other role leaves the note
/// unchanged, so an ordinary peer bearer can never pull the CA key (ENT-12).
fn mint_join_token(
    role: mde_role::Role,
    note: &str,
    lighthouse: Option<String>,
    enroll_port: Option<u16>,
) -> anyhow::Result<String> {
    let root = mackesd_core::default_qnm_shared_root();
    let node_id = default_node_id();
    // mesh-id comes from the founding bundle this lighthouse wrote at `found`.
    let bpath = mackesd_core::ca::bundle::bundle_path(&root, &node_id);
    let bundle = mackesd_core::ca::bundle::read_bundle(&bpath).map_err(|e| {
        anyhow::anyhow!(
            "reading the founding bundle {} — is this a founded lighthouse? ({e})",
            bpath.display()
        )
    })?;
    // Pin the on-disk /enroll endpoint cert fingerprint (the v3 contract).
    let cert_path = mackesd_core::workers::nebula_enroll_listener::DEFAULT_CERT_PATH;
    let cert_pem = std::fs::read(cert_path)
        .map_err(|e| anyhow::anyhow!("reading the /enroll endpoint cert {cert_path}: {e}"))?;
    let fp = mackesd_core::nebula_enroll_endpoint::endpoint_fingerprint_from_pem(&cert_pem)
        .ok_or_else(|| anyhow::anyhow!("no certificate in {cert_path}"))?;
    // Public address the joining box dials (strip any :port; detect if absent).
    let ip = match lighthouse {
        Some(l) => l
            .rsplit_once(':')
            .map_or(l.as_str(), |(h, _)| h)
            .to_string(),
        None => detect_primary_ipv4()?,
    };
    let port = enroll_port.unwrap_or(mackesd_core::nebula_enroll_endpoint::DEFAULT_ENROLL_PORT);
    // #12 — a LIGHTHOUSE token carries a role-scoped bearer note so the signer
    // delivers the CA key + a Host cert; any other role leaves the note unchanged.
    let scoped_note = if role == mde_role::Role::Lighthouse {
        format!(
            "{} {note}",
            mackesd_core::bearer_ledger::LIGHTHOUSE_ROLE_NOTE
        )
    } else {
        note.to_string()
    };
    let bearer = mackesd_core::bearer_ledger::issue(&root, &scoped_note)
        .map_err(|e| anyhow::anyhow!("minting bearer: {e}"))?;
    Ok(mackesd_core::nebula_enroll::JoinToken {
        mesh_id: bundle.mesh_id,
        lighthouse: ip,
        port,
        bearer,
        fp: Some(fp),
    }
    .encode())
}

/// SETUP-5 — remove a peer: decommission its directory row, revoke its certs,
/// and ban its node-id from re-enrolling (the inverse of `add-peer`). Proceeds
/// with the revoke+ban even when no directory row matches, so a stale identity
/// can still be locked out.
pub fn remove_peer(db_path: &std::path::Path, node_id: &str, force: bool) -> anyhow::Result<()> {
    let root = mackesd_core::default_qnm_shared_root();
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .unwrap_or(1);
    let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
        schema_version: 1,
        request_id: format!("remove-peer-{node_id}-{generation}"),
        target_id: node_id.to_string(),
        intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
        generation,
        steps: vec!["offboard".into()],
    };
    let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, plan)
        .map_err(|error| anyhow::anyhow!("cannot acquire remove-peer authority: {error:?}"))?;
    let result = authority.run_next(|_| {
        remove_peer_inner(db_path, node_id, force).map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        let _ = authority.finish();
        return Err(anyhow::anyhow!("remove-peer lifecycle failure: {error:?}"));
    }
    authority
        .finish()
        .map_err(|error| anyhow::anyhow!("cannot release remove-peer authority: {error:?}"))
}

fn remove_peer_inner(db_path: &std::path::Path, node_id: &str, force: bool) -> anyhow::Result<()> {
    let root = mackesd_core::default_qnm_shared_root();
    let self_id = default_node_id();
    let mut conn = mackesd_core::store::open(db_path)
        .with_context(|| format!("opening store at {}", db_path.display()))?;
    mackesd_core::store::migrate(&conn).context("migrating store")?;

    let updated = mackesd_core::store::set_node_role(&conn, node_id, "decommissioned")?;
    if updated == 0 {
        eprintln!(
            "mackesd remove-peer: no directory row for {node_id} — revoking + banning anyway"
        );
    }
    let payload = serde_json::json!({
        "kind":  if force { "forced" } else { "soft" },
        "node":  node_id,
        "event": "remove-peer",
    })
    .to_string();
    mackesd_core::store::insert_event(&mut conn, "lifecycle", &self_id, &payload)?;

    let rows = mackesd_core::ca::revoke::revoke_peer(&conn, &root, &self_id, node_id)
        .context("revoking peer certs")?;

    // HA — if the removed peer is an etcd cluster member (a lighthouse), drop it
    // from the quorum too, so a deleted droplet never leaves a ghost voter.
    // Idempotent: a non-member target (an ordinary peer) is a no-op.
    {
        use mackesd_core::substrate::{etcd, etcd_membership, peers};
        let eps = etcd::default_endpoints();
        if !eps.is_empty() {
            let target = node_id.strip_prefix("peer:").unwrap_or(node_id).to_string();
            let member_result = etcd_membership::remove_member_blocking(
                &eps,
                &etcd_membership::MemberSel::Hostname(target.clone()),
            );
            if matches!(&member_result, Some(Ok(true))) {
                println!("etcd: removed '{node_id}' from the cluster");
            }
            ensure_member_removal_succeeded(member_result)?;
            // MIG-1 — also drop the `/mesh/peers/<hostname>` directory key, not
            // just the etcd MEMBERSHIP. Otherwise the PeerRecord lingers and the
            // roster reconcile keeps re-adding a node whose droplet is gone (the
            // stale entries we had to `etcdctl del` by hand on 2026-06-27). The
            // decommission is now complete: member + directory row both removed.
            if peers::delete_peer_blocking(&eps, &target) {
                println!("etcd: deleted directory key /mesh/peers/{target}");
            } else {
                anyhow::bail!(
                    "etcd directory cleanup failed for '{node_id}'; refusing to finish removal"
                );
            }
        }
    }

    println!(
        "removed '{node_id}': decommissioned ({updated} row), {rows} cert row(s) revoked, banned \
         (propagates to every peer via QNM-Shared)."
    );
    Ok(())
}

/// #13 — `mackesd lighthouse add`: mint a role-scoped lighthouse token on THIS
/// lighthouse, then shell the join provisioner to stand up a DO droplet that JOINS
/// this mesh as a full lighthouse (CA signer + etcd voter, am_lighthouse — all
/// automatic via #11/#12 + the roster reconcile). If the provisioner script isn't
/// installed, print the token + the exact manual command (honest fallback).
pub fn lighthouse_add(
    region: &str,
    size: Option<String>,
    image: Option<String>,
) -> anyhow::Result<()> {
    if let Some(requested) = size.as_deref() {
        anyhow::ensure!(
            requested == THIN_LIGHTHOUSE_SIZE,
            "lighthouse provisioning only supports the thin {THIN_LIGHTHOUSE_SIZE} profile; media, fileshare, and larger variants are retired"
        );
    }
    let token = mint_join_token(
        mde_role::Role::Lighthouse,
        "lighthouse via `lighthouse add`",
        None,
        None,
    )?;
    let script = "/usr/libexec/mackesd/do-lighthouse-join";
    require_lighthouse_provisioner(std::path::Path::new(script))?;
    use std::io::Write as _;
    let mut cmd = std::process::Command::new(script);
    cmd.arg("--token-stdin").args(["--region", region]);
    if let Some(s) = size {
        cmd.args(["--size", &s]);
    }
    if let Some(i) = image {
        cmd.args(["--image", &i]);
    }
    eprintln!(
        "lighthouse add: provisioning a droplet in {region} that joins this mesh as a lighthouse…"
    );
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("starting the join provisioner")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("join provisioner did not expose a private stdin"))?;
    stdin
        .write_all(token.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .context("passing lighthouse enrollment bearer through provisioner stdin")?;
    drop(stdin);
    let status = child.wait().context("running the join provisioner")?;
    if !status.success() {
        anyhow::bail!("the join provisioner failed (see output above)");
    }
    Ok(())
}

/// #13 — `mackesd lighthouse retire`: drain-gate (hold the HA floor unless
/// `--force`), then `remove-peer` (revoke + ban + etcd member-remove, all in
/// [`remove_peer`]), then delete the DO droplet LAST.
pub fn lighthouse_retire(
    db_path: &std::path::Path,
    node_id: &str,
    droplet_id: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    let droplet_id = require_lighthouse_droplet_id(droplet_id)?;
    // HA drain gate — use the authoritative etcd member list and a direct,
    // bounded probe of every survivor. Replicated directory rows can be stale
    // and must never authorize a destructive retirement.
    let endpoints = mackesd_core::substrate::etcd::default_endpoints();
    anyhow::ensure!(
        !endpoints.is_empty(),
        "refusing lighthouse retirement without configured etcd endpoints"
    );
    let members = mackesd_core::substrate::etcd_membership::member_snapshots_blocking(&endpoints)
        .ok_or_else(|| anyhow::anyhow!("etcd retirement preflight could not run"))?
        .map_err(|error| anyhow::anyhow!("etcd retirement preflight failed ({error})"))?;
    let target = node_id.strip_prefix("peer:").unwrap_or(node_id);
    mackesd_core::lighthouse_lifecycle::drain_gate_members(&members, target, force)
        .map_err(|error| anyhow::anyhow!(error))?;
    // Decommission + revoke + ban + etcd member-remove (all in remove_peer).
    remove_peer(db_path, node_id, force)?;
    // Delete the droplet LAST (the inverse of `add`'s provision step).
    let ctx = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
    eprintln!("lighthouse retire: deleting droplet {droplet_id} via doctl (context {ctx})…");
    let status = std::process::Command::new("doctl")
        .args([
            "compute",
            "droplet",
            "delete",
            &droplet_id,
            "--context",
            &ctx,
            "--force",
        ])
        .status()
        .context("running doctl droplet delete")?;
    if !status.success() {
        anyhow::bail!("lighthouse retire: provider deletion failed for droplet {droplet_id}");
    }
    Ok(())
}

/// A missing packaged provisioner is an automation failure, never an excuse to
/// print a secret-bearing command for a human to run later.
fn require_lighthouse_provisioner(path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.is_file(),
        "lighthouse provisioning helper is unavailable at {}; automatic remediation must restore the packaged helper before retrying",
        path.display()
    );
    Ok(())
}

/// All production lighthouses are provider-managed.  Require their immutable
/// provider identity before destructive mutation so an unattended retry knows
/// exactly what still needs deletion.
fn require_lighthouse_droplet_id(droplet_id: Option<String>) -> anyhow::Result<String> {
    let id = droplet_id.ok_or_else(|| anyhow::anyhow!(
        "lighthouse retirement requires the provider droplet id before drain/revoke; refusing a partial retirement"
    ))?;
    anyhow::ensure!(!id.trim().is_empty(), "lighthouse retirement requires a non-empty provider droplet id");
    Ok(id)
}

/// A lighthouse droplet must never be deleted after an unconfirmed etcd
/// membership removal. `None` means the blocking bridge could not run, while
/// `Err` means etcd rejected the mutation; both are fail-closed outcomes.
fn ensure_member_removal_succeeded(result: Option<Result<bool, String>>) -> anyhow::Result<()> {
    match result {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => {
            anyhow::bail!("etcd member removal failed ({error}); refusing to finish peer removal")
        }
        None => anyhow::bail!("etcd member removal could not run; refusing to finish peer removal"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_member_removal_succeeded, require_lighthouse_droplet_id, require_lighthouse_provisioner};

    #[test]
    fn etcd_member_removal_failures_are_fail_closed() {
        assert!(ensure_member_removal_succeeded(Some(Ok(true))).is_ok());
        assert!(ensure_member_removal_succeeded(Some(Ok(false))).is_ok());
        assert!(ensure_member_removal_succeeded(Some(Err("offline".into()))).is_err());
        assert!(ensure_member_removal_succeeded(None).is_err());
    }

    #[test]
    fn provider_lighthouse_lifecycle_refuses_manual_handoffs() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(require_lighthouse_provisioner(&temporary.path().join("missing")).is_err());
        let helper = temporary.path().join("do-lighthouse-join");
        std::fs::write(&helper, "#!/bin/sh\n").unwrap();
        assert!(require_lighthouse_provisioner(&helper).is_ok());
        assert!(require_lighthouse_droplet_id(None).is_err());
        assert!(require_lighthouse_droplet_id(Some("".into())).is_err());
        assert_eq!(require_lighthouse_droplet_id(Some("1234".into())).unwrap(), "1234");
    }
}
