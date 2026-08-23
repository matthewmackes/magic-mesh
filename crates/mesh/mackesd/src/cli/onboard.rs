//! `Onboard` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `onboard` subcommand.
#[allow(unreachable_code)]
pub fn run(verb: OnboardCmd, db_path: PathBuf) -> anyhow::Result<()> {
    match verb {
        OnboardCmd::Lifecycle { intent_json } => {
            use mackes_mesh_types::lifecycle::LifecyclePlanV1;
            let intent = mackes_mesh_types::lifecycle::LifecycleIntentV1::from_json(&intent_json)
                .map_err(|error| anyhow::anyhow!("invalid lifecycle intent: {error:?}"))?;
            let steps = intent.default_steps();
            let plan = LifecyclePlanV1 {
                schema_version: intent.schema_version,
                request_id: intent.request_id,
                target_id: intent.target_id,
                intent: intent.intent,
                generation: intent.generation,
                steps,
            };
            plan.validate()
                .map_err(|error| anyhow::anyhow!("invalid lifecycle plan: {error:?}"))?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        OnboardCmd::LifecycleConfirm {
            target_id,
            confirmation_json,
            verifying_key_hex,
            root,
        } => {
            use ed25519_dalek::VerifyingKey;
            use mackes_mesh_types::lifecycle::LifecycleConfirmationV1;
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let confirmation: LifecycleConfirmationV1 = serde_json::from_str(&confirmation_json)
                .map_err(|error| anyhow::anyhow!("invalid lifecycle confirmation: {error}"))?;
            let key_bytes = parse_hex_32(&verifying_key_hex).ok_or_else(|| {
                anyhow::anyhow!("verifying key must be exactly 64 hex characters")
            })?;
            let verifying_key = VerifyingKey::from_bytes(&key_bytes)
                .map_err(|error| anyhow::anyhow!("invalid verifying key: {error}"))?;
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            authority
                .accept_confirmation(confirmation, &verifying_key)
                .map_err(|error| {
                    anyhow::anyhow!("cannot accept lifecycle confirmation: {error:?}")
                })?;
            println!("{}", serde_json::to_string(authority.checkpoint())?);
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleReadiness { target_id, root } => {
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            let readiness = authority
                .readiness()
                .map_err(|error| anyhow::anyhow!("cannot derive lifecycle readiness: {error:?}"))?;
            println!("{}", serde_json::to_string(&readiness)?);
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleArtifactSelect {
            target_id,
            selection_json,
            confirmation_json,
            verifying_key_hex,
            root,
        } => {
            use ed25519_dalek::VerifyingKey;
            use mackes_mesh_types::lifecycle::LifecycleArtifactSelectionV1;
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let selection: LifecycleArtifactSelectionV1 = serde_json::from_str(&selection_json)
                .map_err(|error| {
                    anyhow::anyhow!("invalid lifecycle artifact selection: {error}")
                })?;
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            if selection.unverified_build {
                let confirmation_json = confirmation_json.ok_or_else(|| {
                    anyhow::anyhow!("unsigned artifact requires --confirmation-json")
                })?;
                let verifying_key_hex = verifying_key_hex.ok_or_else(|| {
                    anyhow::anyhow!("unsigned artifact requires --verifying-key-hex")
                })?;
                let confirmation: mackes_mesh_types::lifecycle::LifecycleConfirmationV1 =
                    serde_json::from_str(&confirmation_json).map_err(|error| {
                        anyhow::anyhow!("invalid unsigned confirmation: {error}")
                    })?;
                let key_bytes = parse_hex_32(&verifying_key_hex).ok_or_else(|| {
                    anyhow::anyhow!("verifying key must be exactly 64 hex characters")
                })?;
                let verifying_key = VerifyingKey::from_bytes(&key_bytes)
                    .map_err(|error| anyhow::anyhow!("invalid verifying key: {error}"))?;
                authority
                    .select_unsigned_artifact(selection, confirmation, &verifying_key)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot select unsigned artifact: {error:?}")
                    })?;
            } else {
                authority
                    .select_artifact(selection)
                    .map_err(|error| anyhow::anyhow!("cannot select artifact: {error:?}"))?;
            }
            println!("{}", serde_json::to_string(authority.checkpoint())?);
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleCapsuleAdmit {
            target_id,
            capsule_json,
            verifying_key_hex,
            now_ms,
            root,
        } => {
            use ed25519_dalek::VerifyingKey;
            use mackes_mesh_types::lifecycle::CommissioningCapsuleV1;
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let capsule: CommissioningCapsuleV1 = serde_json::from_str(&capsule_json)
                .map_err(|error| anyhow::anyhow!("invalid commissioning capsule: {error}"))?;
            let key_bytes = parse_hex_32(&verifying_key_hex).ok_or_else(|| {
                anyhow::anyhow!("verifying key must be exactly 64 hex characters")
            })?;
            let verifying_key = VerifyingKey::from_bytes(&key_bytes)
                .map_err(|error| anyhow::anyhow!("invalid verifying key: {error}"))?;
            let now_ms = now_ms.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or(0)
            });
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            let digest = authority
                .admit_commissioning_capsule(capsule, now_ms, &verifying_key)
                .map_err(|error| {
                    anyhow::anyhow!("cannot admit commissioning capsule: {error:?}")
                })?;
            println!(
                "{{\"bootstrap_digest_hex\":\"{digest}\",\"checkpoint\":{}}}",
                serde_json::to_string(authority.checkpoint())?
            );
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleCapsuleConfirm {
            target_id,
            capsule_id,
            root,
        } => {
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            authority
                .confirm_commissioning_capsule(&capsule_id)
                .map_err(|error| {
                    anyhow::anyhow!("cannot confirm commissioning capsule: {error:?}")
                })?;
            println!("{}", serde_json::to_string(authority.checkpoint())?);
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleCapsuleRevoke {
            target_id,
            capsule_id,
            root,
        } => {
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::resume(&root, &target_id)
                    .map_err(|error| {
                        anyhow::anyhow!("cannot resume lifecycle authority: {error:?}")
                    })?;
            authority
                .revoke_commissioning_capsule(&capsule_id)
                .map_err(|error| {
                    anyhow::anyhow!("cannot revoke commissioning capsule: {error:?}")
                })?;
            println!("{}", serde_json::to_string(authority.checkpoint())?);
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::LifecycleStart { intent_json, root } => {
            use mackes_mesh_types::lifecycle::LifecyclePlanV1;
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let intent = mackes_mesh_types::lifecycle::LifecycleIntentV1::from_json(&intent_json)
                .map_err(|error| anyhow::anyhow!("invalid lifecycle intent: {error:?}"))?;
            let steps = intent.default_steps();
            let plan = LifecyclePlanV1 {
                schema_version: intent.schema_version,
                request_id: intent.request_id,
                target_id: intent.target_id,
                intent: intent.intent,
                generation: intent.generation,
                steps,
            };
            let authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                &root, plan,
            )
            .map_err(|error| anyhow::anyhow!("cannot start lifecycle authority: {error:?}"))?;
            println!("{}", serde_json::to_string(authority.checkpoint())?);
            // Dropping the authority releases the OS lock while retaining the
            // checkpoint and lock path for crash-safe resume.
        }
        OnboardCmd::LifecycleFirstBoot {
            target_id,
            root,
            marker_dir,
            report_only,
        } => {
            use mackes_mesh_types::lifecycle::{
                LifecycleIntentKind, LifecyclePlanV1, LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            };
            use mackesd_core::onboard::firstboot;
            let root = root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let target_id = target_id.unwrap_or_else(default_node_id);
            let marker_dir = marker_dir.unwrap_or_else(firstboot::default_marker_dir);
            let role = match mde_role::load_class() {
                Ok(class) => class.role,
                Err(_) => mde_role::Role::Lighthouse,
            };
            let mut authority = match mackesd_core::lifecycle_authority::LifecycleAuthority::resume(
                &root, &target_id,
            ) {
                Ok(authority) => authority,
                Err(_) => {
                    let intent = mackes_mesh_types::lifecycle::LifecycleIntentV1 {
                        schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                        request_id: format!("firstboot-{target_id}"),
                        target_id: target_id.clone(),
                        intent: LifecycleIntentKind::VerifyAndCorrect,
                        generation: 1,
                    };
                    let steps = intent.default_steps();
                    let plan = LifecyclePlanV1 {
                        schema_version: intent.schema_version,
                        request_id: intent.request_id,
                        target_id: intent.target_id,
                        intent: intent.intent,
                        generation: intent.generation,
                        steps,
                    };
                    mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, plan)
                        .map_err(|error| {
                            anyhow::anyhow!("cannot start first-boot authority: {error:?}")
                        })?
                }
            };
            let generation = authority.checkpoint().plan.generation;
            let pending_tokens = authority.checkpoint().pending_capsule_ids.len();
            // Count invite/enrollment bearers from the workgroup ledger. The
            // capsule retain check below is independent — do not overwrite
            // the live ledger count with pending_capsule_ids.
            let facts =
                firstboot::gather_live_in(&target_id, generation, role, Some(root.as_path()));
            let checks = firstboot::assemble(&facts);
            let readiness = firstboot::record_on_authority(&mut authority, checks.clone())
                .map_err(|error| anyhow::anyhow!("cannot record first-boot checks: {error:?}"))?;
            println!("{}", serde_json::to_string(&readiness)?);
            if !report_only {
                // Credential env, never argv: a failed join can re-present
                // the bearer so first-boot retains it (S6/S17).
                let presented = std::env::var("MCNF_ENROLLMENT_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty());
                let marker = stamp_lifecycle_firstboot_markers(
                    &marker_dir,
                    &root,
                    &checks,
                    presented.as_deref(),
                )
                .map_err(|error| anyhow::anyhow!("cannot apply first-boot markers: {error}"))?;
                eprintln!(
                    "first-boot marker: {marker:?}; pending enrollment tokens: {}",
                    facts.pending_enrollment_tokens
                );
            }
            let pending_after = authority.checkpoint().pending_capsule_ids.len();
            if pending_after != pending_tokens {
                let _ = authority.finish();
                anyhow::bail!("first-boot must retain pending enrollment tokens");
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
            if !readiness.ready {
                std::process::exit(1);
            }
        }
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
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
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
            let target_id = default_node_id();
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let authority_root = mackesd_core::default_qnm_shared_root();
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("role-provision-{target_id}-{generation}"),
                target_id: target_id.clone(),
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["configuration".into()],
            };
            let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                &authority_root,
                lifecycle_plan,
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "cannot acquire lifecycle authority for role provisioning: {error:?}"
                )
            })?;
            let mut outcomes = Vec::new();
            let apply_result = authority.run_next(|_| {
                outcomes = mackesd_core::onboard::role_provision::apply(
                    &plan,
                    &mackesd_core::onboard::role_provision::SystemctlUnits,
                );
                if outcomes.iter().any(|outcome| !outcome.ok) {
                    Err("one or more role units failed".into())
                } else {
                    Ok(())
                }
            });
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
            let finish_result = authority.finish();
            if let Err(error) = apply_result {
                eprintln!("role-provision lifecycle authority recorded failure: {error:?}");
                if let Err(finish_error) = finish_result {
                    eprintln!("cannot release lifecycle authority: {finish_error:?}");
                }
                std::process::exit(1);
            }
            finish_result.map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
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
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("mesh-create-{node_id}-{generation}"),
                target_id: node_id.clone(),
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["identity".into(), "mesh".into()],
            };
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "cannot acquire lifecycle authority for mesh-create: {error:?}"
                        )
                    })?;
            let mut report = None;
            let result = authority.run_next(|_| {
                report = Some(
                    mackesd_core::onboard::mesh_create::create(
                        &mackesd_core::ca::SubprocessBackend,
                        &conn,
                        &root,
                        &node_id,
                        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
                        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
                        std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
                        std::path::Path::new("/etc/nebula"),
                        &external_addr,
                        label.as_deref(),
                    )
                    .map_err(|error| error.to_string())?,
                );
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("mesh-create lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            // The create operation is one authority step; leave the second
            // declared mesh convergence step resumable for the supervisor.
            if let Some(report) = report {
                if report.created {
                    let mut start = std::process::Command::new("systemctl");
                    start.args(["start", "nebula.service"]);
                    mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut start);
                    let _ = start.status();
                }
                print!("{}", report.human());
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
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
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("invite-issue-{node_id}-{generation}"),
                target_id: node_id.clone(),
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["identity".into()],
            };
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "cannot acquire lifecycle authority for invite issuance: {error:?}"
                        )
                    })?;
            let mut issued = None;
            let result = authority.run_next(|_| {
                issued = Some(
                    mackesd_core::onboard::invite::issue(
                        &root,
                        &mesh_id,
                        std::time::Duration::from_secs(minutes.saturating_mul(60)),
                    )
                    .map_err(|error| error.to_string())?,
                );
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("invite issuance lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            let issued = issued.expect("successful invite authority step produces issued invite");
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
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
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
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let node_id = default_node_id();
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("network-{node_id}-{generation}"),
                target_id: node_id,
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["configuration".into()],
            };
            let lifecycle_root = mackesd_core::default_qnm_shared_root();
            let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                &lifecycle_root,
                lifecycle_plan,
            )
            .map_err(|error| {
                anyhow::anyhow!("cannot acquire lifecycle authority for network: {error:?}")
            })?;
            let result = authority.run_next(|_| {
                mackesd_core::onboard::network::apply(
                    &plan,
                    dir,
                    &mackesd_core::onboard::network::SystemConnections,
                )
                .map(|outcome| println!("  keyfile {}: {}", outcome.tag(), path.display()))
                .map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                eprintln!("  network lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
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
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("mesh-dns-{node_id}-{generation}"),
                target_id: node_id,
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["mesh".into()],
            };
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "cannot acquire lifecycle authority for mesh-dns: {error:?}"
                        )
                    })?;
            let sink = mackesd_core::onboard::mesh_dns::EtcHosts::default();
            let result = authority.run_next(|_| {
                mackesd_core::onboard::mesh_dns::apply(&zone, &sink)
                    .map(|outcome| {
                        println!(
                            "  {} → {} ({})",
                            outcome.names,
                            mackesd_core::onboard::mesh_dns::DEFAULT_HOSTS_PATH,
                            if outcome.changed {
                                "updated"
                            } else {
                                "unchanged"
                            }
                        );
                    })
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                eprintln!("mesh-dns lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
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
            // (provision → push-enroll → migrate-CA) under authority.
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("spawn-lighthouse-{node_id}-{generation}"),
                target_id: node_id,
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["mesh".into()],
            };
            let mut authority =
                mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "cannot acquire lifecycle authority for spawn-lighthouse: {error:?}"
                        )
                    })?;
            let result =
                authority.run_next(
                    |_| match sl::execute(&plan, &sl::LiveProvisioner::default()) {
                        Ok(sl::SpawnOutcome::Provisioned { endpoint }) => {
                            println!("  lighthouse provisioned at {}", endpoint.host);
                            Ok(())
                        }
                        Ok(sl::SpawnOutcome::LanOnly { reason }) => {
                            println!("  no-op — stays LAN-only ({reason}); retry available");
                            Ok(())
                        }
                        Err(error) => Err(format!("live provisioning failed: {error}")),
                    },
                );
            if let Err(error) = result {
                eprintln!("  spawn-lighthouse lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::FirstDesktop { dry_run } => {
            // Plan the first cloud-backed VM desktop: gather this node's facts
            // (mesh-id, image catalog), fold into a place/reconnect/no-image
            // plan. The live libvirt lifecycle placement + broker session
            // publish is integration-gated behind the FirstDesktopApply seam;
            // --dry-run stops at the plan + ordered steps.
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
            // (place → open-session) under lifecycle authority.
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_root = mackesd_core::default_qnm_shared_root();
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("first-desktop-{node_id}-{generation}"),
                target_id: node_id,
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["compute".into()],
            };
            let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                &lifecycle_root,
                lifecycle_plan,
            )
            .map_err(|error| {
                anyhow::anyhow!("cannot acquire lifecycle authority for first-desktop: {error:?}")
            })?;
            let mut outcome = None;
            let result = authority.run_next(|_| {
                outcome = Some(
                    fd::execute(&plan, &fd::LiveFirstDesktop::default())
                        .map_err(|error| format!("live first-desktop apply failed: {error}"))?,
                );
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("  first-desktop lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            if let Some(outcome) = outcome {
                println!("  {}", outcome.human());
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
        OnboardCmd::ServiceAdd {
            kind,
            sip_registrar,
            sip_domain,
            sip_username,
            dry_run,
        } => {
            // OW-11 — add a curated back-office service. Music on a lighthouse
            // is retired by the thin-node policy; Files remains a P2P no-op and
            // Voice uses the external SIP seam. --dry-run stops at the plan.
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
            // Live path: drive the integration-gated ServiceApply seam under
            // the same lifecycle authority as role provisioning.
            use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
            let target_id = default_node_id();
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs().max(1))
                .unwrap_or(1);
            let lifecycle_root = mackesd_core::default_qnm_shared_root();
            let lifecycle_plan = LifecyclePlanV1 {
                schema_version: 1,
                request_id: format!("service-add-{target_id}-{generation}"),
                target_id,
                intent: LifecycleIntentKind::Onboard,
                generation,
                steps: vec!["configuration".into()],
            };
            let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(
                &lifecycle_root,
                lifecycle_plan,
            )
            .map_err(|error| {
                anyhow::anyhow!("cannot acquire lifecycle authority for service-add: {error:?}")
            })?;
            let mut outcome = None;
            let result = authority.run_next(|_| {
                let applied = sa::execute(&plan, &sa::LiveServiceApply::default())
                    .map_err(|error| format!("live service apply failed: {error}"))?;
                outcome = Some(applied);
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("  service-add lifecycle authority recorded failure: {error:?}");
                authority.finish().map_err(|finish_error| {
                    anyhow::anyhow!("cannot release lifecycle authority: {finish_error:?}")
                })?;
                std::process::exit(1);
            }
            if let Some(outcome) = outcome {
                println!("  {}", outcome.human());
            }
            authority.finish().map_err(|error| {
                anyhow::anyhow!("cannot release lifecycle authority: {error:?}")
            })?;
        }
    }
    Ok(())
}

fn parse_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// Stamp first-boot markers from assembled baseline checks.
///
/// A blocking audit (including failed enrollment) never consults a caller
/// `ready` bool. When a presented bearer is supplied, the token is retained
/// and `pending-convergence` is queued even if a hostile caller planted
/// healthy facts around the check vector.
fn stamp_lifecycle_firstboot_markers(
    marker_dir: &std::path::Path,
    workgroup_root: &std::path::Path,
    checks: &[mackes_mesh_types::lifecycle::LifecycleRequirementCheckV1],
    presented: Option<&str>,
) -> std::io::Result<mackesd_core::onboard::firstboot::FirstbootMarker> {
    use mackesd_core::onboard::firstboot;
    if firstboot::has_blocking_checks(checks) {
        firstboot::queue_after_failed_enrollment(
            marker_dir,
            workgroup_root,
            presented.unwrap_or(""),
        )
    } else {
        firstboot::apply_markers_from_checks(marker_dir, checks)
    }
}

#[cfg(test)]
mod tests {
    use super::stamp_lifecycle_firstboot_markers;
    use mackesd_core::onboard::firstboot::{
        self, FirstbootFacts, FirstbootMarker, FIRSTBOOT_CONVERGED, FIRSTBOOT_PENDING,
    };
    use mackesd_core::onboard::invite::{self, EnrollEndpoint};
    use std::time::Duration;

    fn healthy(target: &str) -> FirstbootFacts {
        FirstbootFacts {
            target_id: target.to_owned(),
            generation: 1,
            package_present: true,
            package_identity: "magic-mesh-13.0.0-1.fc44.x86_64".to_owned(),
            expected_units: vec!["mackesd.service".into(), "nebula.service".into()],
            active_units: vec!["mackesd.service".into(), "nebula.service".into()],
            configuration_present: true,
            mesh_identity_present: true,
            compute_usable: true,
            ui_applicable: false,
            ui_ready: false,
            hardware_usable: true,
            pending_enrollment_tokens: 0,
        }
    }

    #[test]
    fn lifecycle_firstboot_cli_refuses_ready_over_unit_fail_and_retains_invite() {
        let tmp = tempfile::tempdir().unwrap();
        let workgroup = tmp.path().join("workgroup");
        let markers = tmp.path().join("markers");
        std::fs::create_dir_all(&workgroup).unwrap();

        let issued = invite::issue(&workgroup, "home-mesh", Duration::from_secs(600)).unwrap();
        assert_eq!(invite::count_pending(&workgroup), 1);
        let _token = invite::redeem_once(
            &workgroup,
            &issued.code,
            issued.invite.exp_ms - 1,
            "home-mesh",
            &EnrollEndpoint {
                lighthouse: "10.0.0.5".into(),
                port: 4242,
                fp: None,
            },
        )
        .expect("consume before the failed activation");
        assert!(
            !invite::is_recorded(&workgroup, &issued.code),
            "redeem_once consumed the bearer before transport"
        );

        let facts =
            firstboot::gather_live_in("seat-15", 1, mde_role::Role::Lighthouse, Some(&workgroup));
        assert_eq!(
            facts.pending_enrollment_tokens, 0,
            "consumed invite must not be counted until first-boot retains it"
        );

        let mut blocked = healthy("seat-15");
        blocked
            .active_units
            .retain(|unit| unit != "mackesd.service");
        blocked.mesh_identity_present = false;
        blocked.pending_enrollment_tokens = facts.pending_enrollment_tokens;
        let checks = firstboot::assemble(&blocked);
        assert!(
            firstboot::has_blocking_checks(&checks),
            "inactive mackesd.service is a critical activation failure"
        );
        assert_eq!(
            firstboot::apply_markers(&markers, true).unwrap(),
            FirstbootMarker::Converged,
            "sanity: the raw marker helper still accepts a hostile ready=true"
        );

        assert_eq!(
            stamp_lifecycle_firstboot_markers(&markers, &workgroup, &checks, Some(&issued.code))
                .unwrap(),
            FirstbootMarker::Pending,
            "CLI first-boot must stamp from checks and retain the failed invite"
        );
        assert!(
            invite::is_recorded(&workgroup, &issued.code),
            "failed enrollment must re-record the consumed invite"
        );
        assert_eq!(invite::count_pending(&workgroup), 1);
        assert!(!markers.join(FIRSTBOOT_CONVERGED).exists());
        assert!(markers.join(FIRSTBOOT_PENDING).exists());
        assert_eq!(
            firstboot::gather_live_in("seat-15", 1, mde_role::Role::Lighthouse, Some(&workgroup))
                .pending_enrollment_tokens,
            1
        );
    }

    #[test]
    fn lifecycle_firstboot_cli_stamps_converged_from_checks_not_a_ready_bool() {
        let tmp = tempfile::tempdir().unwrap();
        let checks = firstboot::assemble(&healthy("seat-15"));
        assert!(!firstboot::has_blocking_checks(&checks));
        assert_eq!(
            stamp_lifecycle_firstboot_markers(tmp.path(), tmp.path(), &checks, None).unwrap(),
            FirstbootMarker::Converged
        );
        assert!(tmp.path().join(FIRSTBOOT_CONVERGED).exists());
        assert!(!tmp.path().join(FIRSTBOOT_PENDING).exists());
    }
}
