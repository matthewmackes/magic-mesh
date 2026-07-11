//! `Onboard` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `onboard` subcommand.
#[allow(unreachable_code)]
pub fn run(verb: OnboardCmd, db_path: PathBuf) -> anyhow::Result<()> {
    match verb {
        OnboardCmd::SelfTest { json } => {
            // Probe the live node, fold into the report, print, and exit on
            // its verdict (non-zero iff a critical check failed).
            let node_id = default_node_id();
            let root = mackesd_core::default_qnm_shared_root();
            let probes = mackesd_core::onboard::self_test::gather(&node_id, &db_path, &root);
            let report = mackesd_core::onboard::self_test::assemble(&probes);
            // OW-10 (send half) — publish the overall verdict on the mesh Bus
            // (`event/onboard/self-test`) so the egui shell's Mesh Map opens
            // when onboarding goes all-green. Best-effort, before the print +
            // verdict exit; the same one-shot `mde-bus publish` path
            // `ca::revoke` fires on. The published `{ ok }` is the REAL
            // computed verdict (green iff no critical check failed).
            report.publish_verdict();
            if json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print!("{}", report.human());
            }
            std::process::exit(report.exit_code());
        }
        OnboardCmd::RoleProvision { role, dry_run } => {
            let parsed: mde_role::Role = role.parse().map_err(|_| {
                anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation")
            })?;
            let plan = mackesd_core::onboard::role_provision::plan(parsed);
            if dry_run {
                println!(
                    "onboard role-provision --role {} (dry-run, {} units):",
                    parsed.as_str(),
                    plan.len()
                );
                for u in &plan {
                    println!("  {:?}\t{}", u.action, u.unit);
                }
                return Ok(());
            }
            let outcomes = mackesd_core::onboard::role_provision::apply(
                &plan,
                &mackesd_core::onboard::role_provision::SystemctlUnits,
            );
            let mut failed = 0usize;
            for o in &outcomes {
                if o.ok {
                    println!("  {:?} {} — ok", o.action, o.unit);
                } else {
                    failed += 1;
                    eprintln!(
                        "  {:?} {} — FAILED: {}",
                        o.action,
                        o.unit,
                        o.error.as_deref().unwrap_or("unknown error")
                    );
                }
            }
            println!(
                "role-provision {}: {} units applied, {failed} failed",
                parsed.as_str(),
                outcomes.len()
            );
            if failed > 0 {
                std::process::exit(1);
            }
        }
        OnboardCmd::MeshCreate { label } => {
            // Found a mesh-of-one on this Workstation, reusing mesh_init's
            // CA-bootstrap. Resolve the LAN/underlay address best-effort — a
            // truly offline lone box has no default route, so fall back to
            // loopback (OW-6 wires the real mesh-DNS / network); the founding
            // node's lighthouse entry is self-referential on a mesh-of-one.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            mackesd_core::store::migrate(&conn).context("migrating store")?;
            let root = mackesd_core::default_qnm_shared_root();
            let node_id = default_node_id();
            let external_addr = detect_primary_ipv4()
                .map(|ip| format!("{ip}:4242"))
                .unwrap_or_else(|_| "127.0.0.1:4242".to_string());
            let report = mackesd_core::onboard::mesh_create::create(
                &mackesd_core::ca::SubprocessBackend,
                &conn,
                &root,
                &node_id,
                std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
                std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
                std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
                &external_addr,
                label.as_deref(),
            )?;
            // Best-effort overlay start on a fresh founding (mirrors
            // mesh-init; the next serve's supervisor also materializes +
            // starts). A no-op founding leaves the running overlay untouched.
            if report.created {
                let _ = std::process::Command::new("systemctl")
                    .args(["start", "nebula.service"])
                    .status();
            }
            print!("{}", report.human());
        }
        OnboardCmd::InviteIssue { ttl } => {
            // Mint a short-TTL, mesh-scoped invite on THIS node, record it in
            // the bearer ledger, and print both encodings headlessly. When this
            // node has the local /enroll endpoint identity, also print the
            // endpoint-bearing `mesh:` token that `mackesd join` can consume
            // directly; its bearer is the same canonical invite payload already
            // recorded in the ledger, so the short code and join token spend the
            // same single-use capability.
            let node_id = default_node_id();
            let root = mackesd_core::default_qnm_shared_root();
            let mesh_id = mackesd_core::onboard::invite::resolve_mesh_id(&root, &node_id);
            let minutes = ttl.unwrap_or(mackesd_core::onboard::invite::DEFAULT_TTL_MINUTES);
            let issued = mackesd_core::onboard::invite::issue(
                &root,
                &mesh_id,
                std::time::Duration::from_secs(minutes.saturating_mul(60)),
            )?;
            println!(
                "invite-issue: mesh '{mesh_id}' — expires in {minutes} min \
                     (exp {} epoch-ms){}",
                issued.invite.exp_ms,
                if issued.recorded {
                    ""
                } else {
                    " [NOT recorded — zero TTL]"
                }
            );
            println!("  code: {}", issued.code);
            println!("  qr:   {}", issued.qr);
            match invite_issue_join_token(&issued, None, None) {
                Ok(join_token) => {
                    println!("  join-token: {join_token}");
                    println!("  join: mackesd join '{join_token}'");
                }
                Err(e) => eprintln!(
                    "  join-token: unavailable ({e}); use the code/QR in a local wizard, \
                         or mint an endpoint-bearing token on a lighthouse with `mackesd add-peer`"
                ),
            }
        }
        OnboardCmd::Network { dry_run } => {
            // Detect DHCP-vs-static on the primary LAN interface (reusing
            // router_discovery's default-gateway detection) and render the
            // NetworkManager keyfile. The live apply (write + `nmcli reload`) is
            // the integration-gated LAN bring-up; --dry-run stops at the plan.
            let facts = mackesd_core::onboard::network::gather();
            let plan = match mackesd_core::onboard::network::plan_network(&facts) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("onboard network: cannot plan LAN bring-up — {e}");
                    std::process::exit(1);
                }
            };
            println!("onboard network: {}", plan.human());
            let dir = std::path::Path::new(mackesd_core::onboard::network::SYSTEM_CONNECTIONS_DIR);
            let path = mackesd_core::onboard::network::keyfile_path(dir);
            if dry_run {
                println!("--- {} (dry-run, not written) ---", path.display());
                print!("{}", mackesd_core::onboard::network::render_keyfile(&plan));
                return Ok(());
            }
            match mackesd_core::onboard::network::apply(
                &plan,
                dir,
                &mackesd_core::onboard::network::SystemConnections,
            ) {
                Ok(outcome) => println!("  keyfile {}: {}", outcome.tag(), path.display()),
                Err(e) => {
                    eprintln!("  keyfile apply failed (LAN bring-up is integration-gated): {e}");
                    std::process::exit(1);
                }
            }
        }
        OnboardCmd::MeshDns { dry_run } => {
            // Fold the replicated peer roster into the mesh-DNS zone and
            // publish the managed /etc/hosts block. Headless: prints the zone,
            // then (unless --dry-run) writes the block idempotently.
            let node_id = default_node_id();
            let root = mackesd_core::default_qnm_shared_root();
            let mesh_id = mackesd_core::onboard::invite::resolve_mesh_id(&root, &node_id);
            let zone = mackesd_core::onboard::mesh_dns::resolve_zone(&root, &mesh_id);
            println!(
                "onboard mesh-dns: mesh '{mesh_id}' — {} name(s):",
                zone.len()
            );
            for (name, ip) in &zone {
                println!("  {name}\t{ip}");
            }
            if dry_run {
                print!("{}", mackesd_core::onboard::mesh_dns::render_hosts(&zone));
                return Ok(());
            }
            let sink = mackesd_core::onboard::mesh_dns::EtcHosts::default();
            match mackesd_core::onboard::mesh_dns::apply(&zone, &sink) {
                Ok(outcome) => println!(
                    "  {} → {} ({})",
                    outcome.names,
                    mackesd_core::onboard::mesh_dns::DEFAULT_HOSTS_PATH,
                    if outcome.changed {
                        "updated"
                    } else {
                        "unchanged"
                    }
                ),
                Err(e) => {
                    eprintln!("mesh-dns apply failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        OnboardCmd::SpawnLighthouse { pair, dry_run } => {
            // Plan the spawn: gather this node's facts (mesh-id, CA holder,
            // cloud token), fold into a plan. The live provision/SSH/CA-move
            // is integration-gated behind the Provisioner seam; --dry-run stops
            // at the plan + rendered spec.
            use mackesd_core::onboard::spawn_lighthouse as sl;
            let node_id = default_node_id();
            let root = mackesd_core::default_qnm_shared_root();
            let facts = sl::gather(&root, &node_id);
            let req = sl::SpawnRequest {
                target: sl::SpawnTarget::default_cloud(),
                pair,
            };
            let plan = sl::plan_spawn(&req, &facts);
            println!("onboard spawn-lighthouse: {}", plan.human());
            if dry_run {
                if let Some(spec) = plan.provision_spec() {
                    println!("--- provision spec (dry-run, not provisioned) ---");
                    print!("{}", spec.document());
                }
                return Ok(());
            }
            // Live path: drive the integration-gated Provisioner seam
            // (provision → push-enroll → migrate-CA).
            match sl::execute(&plan, &sl::LiveProvisioner::default()) {
                Ok(sl::SpawnOutcome::Provisioned { endpoint }) => {
                    println!("  lighthouse provisioned at {}", endpoint.host);
                }
                Ok(sl::SpawnOutcome::LanOnly { reason }) => {
                    println!("  no-op — stays LAN-only ({reason}); retry available");
                }
                Err(e) => {
                    eprintln!(
                        "  spawn-lighthouse failed (live provisioning is integration-gated): {e}"
                    );
                    std::process::exit(1);
                }
            }
        }
        OnboardCmd::FirstDesktop { dry_run } => {
            // Plan the first cloud-backed VM desktop: gather this node's facts
            // (mesh-id, image catalog), fold into a place/reconnect/no-image
            // plan. The live Nova placement + broker session publish is
            // integration-gated behind the FirstDesktopApply seam; --dry-run
            // stops at the plan + ordered steps.
            use mackesd_core::onboard::first_desktop as fd;
            let node_id = default_node_id();
            let root = mackesd_core::default_qnm_shared_root();
            let facts = fd::gather(&root, &node_id);
            let plan = fd::plan_first_desktop(&facts);
            println!("onboard first-desktop: {}", plan.human());
            if dry_run {
                for (i, step) in plan.steps().iter().enumerate() {
                    println!("  {}. {}", i + 1, step.describe());
                }
                return Ok(());
            }
            // Live path: drive the integration-gated FirstDesktopApply seam
            // (place → open-session).
            match fd::execute(&plan, &fd::LiveFirstDesktop::default()) {
                Ok(outcome) => println!("  {}", outcome.human()),
                Err(e) => {
                    eprintln!(
                            "  first-desktop failed (live Nova placement + session is integration-gated): {e}"
                        );
                    std::process::exit(1);
                }
            }
        }
        OnboardCmd::ServiceAdd {
            kind,
            sip_registrar,
            sip_domain,
            sip_username,
            dry_run,
        } => {
            // OW-11 — add a curated back-office service. Gather the mesh's
            // lighthouses (media servers live on lighthouses, #19), fold into a
            // per-kind plan: Music provisions Navidrome on a media-lighthouse
            // (DO Spaces); Files is a real P2P no-op; Voice registers to an
            // external SIP. The live provision / SIP register is integration-gated
            // behind the ServiceApply seam; --dry-run stops at the plan + steps.
            use mackesd_core::onboard::service_add as sa;
            let Some(service_kind) = sa::ServiceKind::parse(&kind) else {
                eprintln!("service-add: unknown service '{kind}' (expected music | files | voice)");
                std::process::exit(2);
            };
            // Voice: build the external SIP account only when the operator
            // supplied registrar + username; otherwise the plan is the honest
            // VoiceNeedsAccount retryable outcome (never a fabricated account).
            let sip = match (sip_registrar, sip_username) {
                (Some(registrar), Some(username)) => {
                    let domain = sip_domain.unwrap_or_else(|| registrar.clone());
                    Some(sa::SipAccount::new(&registrar, &domain, &username))
                }
                _ => None,
            };
            let req = sa::ServiceAddRequest {
                kind: service_kind,
                sip,
            };
            let root = mackesd_core::default_qnm_shared_root();
            let facts = sa::gather(&root);
            let plan = sa::plan_service_add(&req, &facts);
            println!("onboard service-add: {}", plan.human());
            if dry_run {
                for (i, step) in plan.steps().iter().enumerate() {
                    println!("  {}. {}", i + 1, step);
                }
                return Ok(());
            }
            // Live path: drive the integration-gated ServiceApply seam.
            match sa::execute(&plan, &sa::LiveServiceApply::default()) {
                Ok(outcome) => println!("  {}", outcome.human()),
                Err(e) => {
                    eprintln!(
                            "  service-add failed (live Navidrome provision / SIP register is integration-gated): {e}"
                        );
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
