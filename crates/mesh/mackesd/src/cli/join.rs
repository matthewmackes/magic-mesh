//! `Join` CLI verb handler (one-command peer/lighthouse join).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved. The invite-redeem branch + the lighthouse etcd/
//! CA-backup provisioning helpers are join-exclusive and kept private here.
use crate::*;

/// OW-4 — redeem a wizard-minted `MDEINV1-…` invite (or its `mde-invite:` QR
/// twin) on the join side. Validates the presented code — mesh-scope + TTL
/// offline, then the bearer ledger — and maps it to the same v3 CSR the
/// lighthouse signs (`invite::redeem`). The MDEINV1 envelope is endpoint-less
/// by design (a code is presented over many transports and stays QR-short), so
/// the live network-enroll leg (CSR → signed bundle → overlay IP) is
/// integration-gated with a typed error rather than faked: a code alone cannot
/// contact a lighthouse. The operator completes a live join with the
/// endpoint-bearing v3 token from `mackesd found`.
fn cmd_join_invite(
    raw_token: &str,
    parsed: mde_role::Role,
    _name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use mackesd_core::onboard::invite;

    let root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();

    // Decode up front to learn the invite's declared mesh: a box already on a
    // mesh must present an invite FOR that mesh (cross-mesh codes refused),
    // while a fresh box ADOPTS the mesh the invite names.
    let decoded = invite::Invite::decode(raw_token)
        .ok_or_else(|| anyhow::anyhow!("invite refused: {}", invite::RedeemError::Malformed))?;
    let founded = mackesd_core::ca::bundle::read_bundle(&mackesd_core::ca::bundle::bundle_path(
        &root, &node_id,
    ))
    .is_ok();
    let expected_mesh = if founded {
        invite::resolve_mesh_id(&root, &node_id)
    } else {
        decoded.mesh_id
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));

    // Validate: mesh-scope + TTL + the bearer ledger. Expired / foreign /
    // tampered codes are refused here with a typed error, never a panic.
    let redeemed = invite::validate_for_redeem(&root, raw_token, now_ms, &expected_mesh)
        .map_err(|e| anyhow::anyhow!("invite refused ({}): {e}", e.reason()))?;

    // Pin the role when unpinned, matching the v3 join.
    match mde_role::load() {
        Ok(existing) => println!("role already pinned: {existing}"),
        Err(mde_role::LoadError::NotPinned) => {
            mde_role::pin(parsed).map_err(|e| anyhow::anyhow!("pinning role: {e}"))?;
            println!("role pinned: {}", parsed.as_str());
        }
        Err(e) => anyhow::bail!("reading role: {e}"),
    }

    // Validated — but the envelope has no `/enroll` endpoint, so the live enroll
    // leg needs the lighthouse address the invite cannot supply. Gate it
    // honestly rather than fake an endpoint: the redemption mapping is proven by
    // unit tests to yield the same v3 CSR inputs, and the 2-box network leg is
    // integration-gated.
    anyhow::bail!(
        "invite for mesh `{}` validated (live + ledger-recorded) — its redemption \
         maps to the same v3 CSR the lighthouse signs, but an MDEINV1 code is \
         endpoint-less; the live enroll leg needs the lighthouse `/enroll` endpoint. \
         Complete a network join now with the endpoint-bearing token from \
         `mackesd found` (mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>). [OW-4]",
        redeemed.mesh_id,
    );
}

/// ONBOARD-4 — the `join` verb. One-command peer join: pin role +
/// fingerprint-pinned network-enroll + materialize /etc/nebula.
pub fn run(
    token: Option<String>,
    role: &str,
    name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    // No token → hand off to the enrollment TUI (ONBOARD-5, `mde-enroll`).
    let Some(raw_token) = token else {
        let launched = std::process::Command::new("mde-enroll").status();
        return match launched {
            Ok(s) if s.success() => Ok(()),
            _ => Err(anyhow::anyhow!(
                "no token given and the `mde-enroll` TUI isn't on PATH. \
                 Pass the token from `mackesd found`:\n  mackesd join '<token>'"
            )),
        };
    };

    let parsed: mde_role::Role = role
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation"))?;

    // OW-4 — a wizard-minted `MDEINV1-…` invite (or its `mde-invite:` QR twin) is
    // a DIFFERENT token type than the v3 `mesh:<id>@<ip>:<port>#<bearer>` join
    // token, so `parse_join_token` would reject it. Redeem it on this branch:
    // validate mesh-scope + TTL + the bearer ledger, then gate the endpoint-
    // needing live leg (the envelope is endpoint-less by design).
    if mackesd_core::onboard::invite::looks_like_invite(&raw_token) {
        return cmd_join_invite(&raw_token, parsed, name, workgroup_root);
    }

    let token = mackesd_core::nebula_enroll::parse_join_token(&raw_token).ok_or_else(|| {
        anyhow::anyhow!("invalid join token (expected mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>)")
    })?;
    let root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();
    let display_name = name.unwrap_or_else(|| {
        node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string()
    });

    // Pin the role when unpinned (an already-pinned box keeps its role).
    match mde_role::load() {
        Ok(existing) => println!("role already pinned: {existing}"),
        Err(mde_role::LoadError::NotPinned) => {
            mde_role::pin(parsed).map_err(|e| anyhow::anyhow!("pinning role: {e}"))?;
            println!("role pinned: {}", parsed.as_str());
        }
        Err(e) => anyhow::bail!("reading role: {e}"),
    }

    if token.fp.is_none() {
        // No fingerprint → legacy co-located QNM-Shared flow (the network
        // path requires the pinned fp). Honest fallback, not an error.
        println!("token has no fingerprint — using the co-located QNM-Shared enroll flow");
        let outcome = mackesd_core::nebula_enroll::enroll_with_token(
            &root,
            &node_id,
            &display_name,
            &raw_token,
        )
        .map_err(|e| anyhow::anyhow!("enroll: {e}"))?;
        println!(
            "enrolled into `{}` as {} (waited {:?})",
            outcome.mesh_id, outcome.overlay_ip, outcome.waited
        );
        return Ok(());
    }

    // Network enroll (the MESH-1 fix) — runs on a small async runtime.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building async runtime for network enroll")?;
    let config_dir = std::path::PathBuf::from("/etc/nebula");
    let bundle = runtime.block_on(mackesd_core::nebula_enroll_client::network_enroll(
        &root,
        &config_dir,
        &node_id,
        &display_name,
        token,
    ))?;

    // Bring the peer fully live + boot-durable (ONBOARD-9): the overlay, the
    // worker daemon, and the health watchdog — not just nebula. A `join` now
    // leaves a node that survives reboot and self-recovers, instead of one the
    // operator must `systemctl restart mackesd` by hand.
    enable_now_service("nebula.service");

    // CONNECT-4 — if this peer joined as a Lighthouse, it's an ingress node too.
    provision_caddy_if_lighthouse(parsed);

    // LIGHTHOUSE-10 — an ADDITIONAL lighthouse (the 2nd–5th) persists its own
    // public underlay address so its heartbeat publishes it to the directory and
    // every node's enroll roster includes it (full redundancy). Auto-detect the
    // primary public IPv4 (override later with `mackesd set-external-addr`).
    if parsed == mde_role::Role::Lighthouse {
        match detect_primary_ipv4() {
            Ok(ip) => {
                if let Err(e) =
                    mackesd_core::lighthouse_addr::write_external_addr(&format!("{ip}:4242"))
                {
                    eprintln!(
                        "join: could not persist external-addr ({e}) — set it with `mackesd set-external-addr`"
                    );
                }
            }
            Err(e) => eprintln!(
                "join: could not auto-detect public IP ({e}) — run `mackesd set-external-addr <ip:4242>` so this lighthouse is reachable"
            ),
        }
        // HA / turn-key — a new lighthouse auto-joins the etcd quorum as a voter
        // (no manual `etcdctl member add`). Best-effort: failure logs an
        // actionable message and the enrolled lighthouse still comes up.
        lighthouse_join_etcd(&bundle, &display_name);

        // MIG-3 — a joined lighthouse inherits the mesh CA (same mesh,
        // same signing key as the founder), so it will hold ca.key and
        // the backup worker would otherwise loud-warn SEC-7/ENT-11
        // "UNBACKED-UP" every boot. Provision the sealed CA-backup
        // passphrase credential now (generated-on-joiner, host-bound via
        // systemd-creds — never transmitted off this box) + write the
        // LoadCredentialEncrypted drop-in so the upcoming mackesd restart
        // picks it up. Best-effort: a miss logs an actionable line but
        // never aborts the join.
        provision_ca_backup_passphrase_if_lighthouse(parsed);
    }

    // SETUP-7 — capture the joined facts (mesh-id + lighthouse roster from the
    // signed bundle) for idempotent re-convergence.
    let roster: Vec<String> = bundle
        .lighthouses
        .iter()
        .map(|lh| lh.overlay_ip.clone())
        .collect();
    emit_site_yml_best_effort(parsed.as_str(), &bundle.mesh_id, roster);

    enable_now_service("mackesd.service");
    enable_now_service("mesh-health.timer");

    println!(
        "joined `{}` as {} (overlay {})",
        bundle.mesh_id, node_id, bundle.overlay_ip
    );
    println!("services: nebula + mackesd + mesh-health enabled (boot-durable) and running");
    Ok(())
}

/// HA / turn-key — a freshly-joined lighthouse auto-joins the etcd quorum as a
/// voter via the native member API ([`mackesd_core::substrate::etcd_membership`]),
/// then starts its local etcd via `setup-etcd --join --initial-cluster`. The
/// anchors are the EXISTING lighthouses from the signed bundle. Best-effort with a
/// short retry for the just-brought-up overlay handshake; on failure it prints the
/// exact manual command and returns — the lighthouse is enrolled either way.
fn lighthouse_join_etcd(bundle: &mackesd_core::ca::bundle::NebulaBundle, self_name: &str) {
    use mackesd_core::substrate::etcd_membership;
    let self_overlay = bundle.overlay_ip.clone();
    let anchor_overlay = bundle
        .lighthouses
        .iter()
        .map(|lh| lh.overlay_ip.clone())
        .find(|ip| ip != &self_overlay);
    let Some(anchor_overlay) = anchor_overlay else {
        eprintln!(
            "join: no existing lighthouse anchor in the bundle — skipping etcd auto-join \
             (a founding lighthouse bootstraps etcd with `setup-etcd --init`)"
        );
        return;
    };
    let anchors: Vec<String> = bundle
        .lighthouses
        .iter()
        .filter(|lh| lh.overlay_ip != self_overlay)
        .map(|lh| etcd_membership::client_url(&lh.overlay_ip))
        .collect();
    let mut last = String::new();
    for attempt in 1..=5 {
        match etcd_membership::add_self_as_voter_blocking(&anchors, self_name, &self_overlay) {
            Some(Ok(csv)) => {
                let st = std::process::Command::new("/usr/libexec/mackesd/setup-etcd")
                    .args([
                        "--join",
                        &anchor_overlay,
                        "--listen",
                        &self_overlay,
                        "--initial-cluster",
                        &csv,
                    ])
                    .status();
                match st {
                    Ok(s) if s.success() => {
                        println!(
                            "etcd: joined the quorum as a voter (member added + local etcd started)"
                        );
                    }
                    _ => eprintln!(
                        "etcd: member added but `setup-etcd --join` failed — start the local \
                         member by hand: /usr/libexec/mackesd/setup-etcd --join {anchor_overlay} \
                         --listen {self_overlay}"
                    ),
                }
                return;
            }
            Some(Err(e)) => last = e,
            None => last = "bridge runtime unavailable".to_string(),
        }
        if attempt < 5 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
    eprintln!(
        "join: etcd auto-join did not complete ({last}) — the lighthouse is enrolled; add it to \
         the quorum once the overlay is up: /usr/libexec/mackesd/setup-etcd --join {anchor_overlay} \
         --listen {self_overlay}"
    );
}

/// MIG-3 — on a joined Lighthouse, ensure a sealed CA-backup passphrase
/// credential exists so the box boots without the SEC-7/ENT-11
/// "UNBACKED-UP" warning. The passphrase is GENERATED locally + sealed
/// host-bound via systemd-creds (TPM/host key) — it never leaves this
/// box and is never logged (only its presence/length). No-op for
/// non-lighthouse roles + idempotent (never rotates an existing cred).
///
/// The OFF-FLEET / off-site CA-backup push is intentionally NOT touched
/// here — that remains an operator-run step. This only clears the
/// "no backup passphrase credential" boot error.
///
/// Best-effort + idempotent: a miss logs an actionable line but never
/// aborts the join (the lighthouse still joins; the worker keeps
/// warning until the operator provisions it by hand per the unit
/// comment).
fn provision_ca_backup_passphrase_if_lighthouse(role: mde_role::Role) {
    use mackesd_core::ca::backup_provision::{provision, ProvisionOutcome};
    match provision(role) {
        Ok(ProvisionOutcome::Provisioned { sealed_bytes }) => {
            // Log presence/length only — NEVER the passphrase value.
            println!(
                "MIG-3: sealed CA-backup passphrase provisioned ({sealed_bytes}-byte credential) — CA no longer UNBACKED-UP"
            );
            // The drop-in is new; reload so the upcoming mackesd.service
            // (re)start surfaces $CREDENTIALS_DIRECTORY/backup-passphrase.
            let _ = std::process::Command::new("systemctl")
                .arg("daemon-reload")
                .status();
        }
        Ok(ProvisionOutcome::AlreadyPresent) => {
            println!("MIG-3: CA-backup passphrase credential already present — left untouched");
        }
        Ok(ProvisionOutcome::NotLighthouse) => {}
        Err(e) => eprintln!(
            "MIG-3: could not provision the CA-backup passphrase ({e}) — this lighthouse will \
             warn SEC-7/ENT-11 until you provision it by hand (see the EFF-15 comment in the \
             mackesd.service unit)"
        ),
    }
}
