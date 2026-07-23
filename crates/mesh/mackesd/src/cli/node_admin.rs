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
const THIN_LIGHTHOUSE_SIZE: &str = "s-1vcpu-512mb-10gb";

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
            match etcd_membership::remove_member_blocking(
                &eps,
                &etcd_membership::MemberSel::Hostname(target.clone()),
            ) {
                Some(Ok(true)) => println!("etcd: removed '{node_id}' from the cluster"),
                Some(Ok(false)) | None => {}
                Some(Err(e)) => {
                    eprintln!("etcd: could not remove '{node_id}' from the cluster ({e})");
                }
            }
            // MIG-1 — also drop the `/mesh/peers/<hostname>` directory key, not
            // just the etcd MEMBERSHIP. Otherwise the PeerRecord lingers and the
            // roster reconcile keeps re-adding a node whose droplet is gone (the
            // stale entries we had to `etcdctl del` by hand on 2026-06-27). The
            // decommission is now complete: member + directory row both removed.
            if peers::delete_peer_blocking(&eps, &target) {
                println!("etcd: deleted directory key /mesh/peers/{target}");
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
    if !std::path::Path::new(script).exists() {
        println!("{token}");
        eprintln!(
            "lighthouse add: the join provisioner ({script}) isn't installed — run it by hand:\n  \
             do-lighthouse-join.sh '{token}' --region {region}"
        );
        return Ok(());
    }
    let mut cmd = std::process::Command::new(script);
    cmd.arg(&token).args(["--region", region]);
    if let Some(s) = size {
        cmd.args(["--size", &s]);
    }
    if let Some(i) = image {
        cmd.args(["--image", &i]);
    }
    eprintln!(
        "lighthouse add: provisioning a droplet in {region} that joins this mesh as a lighthouse…"
    );
    let status = cmd.status().context("running the join provisioner")?;
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
    let root = mackesd_core::default_qnm_shared_root();
    // HA drain gate — never drop below the lighthouse floor without --force.
    let current =
        mackesd_core::substrate::etcd_membership::voter_overlays_from_directory(&root).len();
    mackesd_core::lighthouse_lifecycle::drain_gate(current, force)
        .map_err(|e| anyhow::anyhow!(e))?;
    // Decommission + revoke + ban + etcd member-remove (all in remove_peer).
    remove_peer(db_path, node_id, force)?;
    // Delete the droplet LAST (the inverse of `add`'s provision step).
    if let Some(id) = droplet_id {
        let ctx = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
        eprintln!("lighthouse retire: deleting droplet {id} via doctl (context {ctx})…");
        let status = std::process::Command::new("doctl")
            .args([
                "compute",
                "droplet",
                "delete",
                &id,
                "--context",
                &ctx,
                "--force",
            ])
            .status()
            .context("running doctl droplet delete")?;
        if !status.success() {
            eprintln!("lighthouse retire: doctl droplet delete {id} failed — delete it by hand");
        }
    } else {
        eprintln!(
            "lighthouse retire: no --droplet-id given; the node is drained + revoked, but the DO \
             droplet (if any) was NOT deleted — remove it with `doctl compute droplet delete`"
        );
    }
    Ok(())
}
