//! `Found` CLI verb handler (one-command founding lighthouse).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// ONBOARD-4 — the `found` verb. One-command founding lighthouse:
/// mesh-init + `/enroll` endpoint identity + a v3 join line.
pub fn run(
    db_path: &std::path::Path,
    mesh_id: &str,
    external_addr: &str,
    role: &str,
    enroll_port: Option<u16>,
    with_backoffice: Option<&str>,
) -> anyhow::Result<()> {
    use mackesd_core::nebula_enroll_endpoint::{generate_endpoint_identity, DEFAULT_ENROLL_PORT};
    use mackesd_core::workers::nebula_enroll_listener::{DEFAULT_CERT_PATH, DEFAULT_KEY_PATH};

    let parsed: mde_role::Role = role
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation"))?;

    // DAR-18 — validate the backoffice tier UP FRONT (before any mesh-init side
    // effect), so `--with-backoffice=bogus` fails fast without half-founding a mesh.
    let backoffice_tier = match with_backoffice {
        None => None,
        Some(t) => Some(normalize_backoffice_tier(t)?),
    };
    // Resolve the externally-dialable IPv4 (strip any :port the operator
    // included; `auto` detects the primary outbound IP).
    let ip = if external_addr.eq_ignore_ascii_case("auto") {
        detect_primary_ipv4()?
    } else {
        external_addr
            .rsplit_once(':')
            .map_or(external_addr, |(host, _)| host)
            .to_string()
    };
    let enroll_port = enroll_port.unwrap_or(DEFAULT_ENROLL_PORT);

    let conn = mackesd_core::store::open(db_path)
        .with_context(|| format!("opening store at {}", db_path.display()))?;
    mackesd_core::store::migrate(&conn).context("migrating store")?;
    let root = mackesd_core::default_qnm_shared_root();
    let node_id = default_node_id();

    // Generate + persist the self-signed `/enroll` endpoint identity
    // BEFORE printing the token (the token pins its fingerprint). The
    // key lands at 0600.
    let identity = generate_endpoint_identity(&[ip.clone()])
        .map_err(|e| anyhow::anyhow!("generating /enroll endpoint identity: {e}"))?;
    let cert_path = std::path::Path::new(DEFAULT_CERT_PATH);
    if let Some(dir) = cert_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(cert_path, identity.cert_pem.as_bytes())
        .with_context(|| format!("writing {DEFAULT_CERT_PATH}"))?;
    std::fs::write(DEFAULT_KEY_PATH, identity.key_pem.as_bytes())
        .with_context(|| format!("writing {DEFAULT_KEY_PATH}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(DEFAULT_KEY_PATH, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {DEFAULT_KEY_PATH}"))?;
    }

    // LIGHTHOUSE-10 — persist this lighthouse's PUBLIC underlay address so the
    // telemetry heartbeat can stamp it into the replicated peer directory; the
    // enroll roster reads every lighthouse's external_addr to hand a joining node
    // the FULL (redundant) lighthouse set. Best-effort: a miss only delays the
    // self entry appearing in others' rosters until set-external-addr/refresh.
    if let Err(e) =
        mackesd_core::lighthouse_addr::write_external_addr(&format!("{ip}:{}", 4242_u16))
    {
        eprintln!(
            "found: could not persist external-addr ({e}) — set it with `mackesd set-external-addr`"
        );
    }

    // mesh-init: pin role, mint CA, self-sign, write the founding bundle,
    // and mint the first single-use bearer.
    let report = mackesd_core::mesh_init::mesh_init(
        &mackesd_core::ca::SubprocessBackend,
        &conn,
        &root,
        &node_id,
        mesh_id,
        &format!("{ip}:4242"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
        std::path::Path::new("/etc/nebula"),
        parsed,
    )?;

    // Re-express mesh-init's freshly-minted bearer as a v3 token that
    // points at the PUBLIC ip + enroll port and pins the endpoint fp.
    let legacy = mackesd_core::nebula_enroll::parse_join_token(&report.join_token)
        .ok_or_else(|| anyhow::anyhow!("mesh-init returned an unparseable join token"))?;
    let v3 = mackesd_core::nebula_enroll::JoinToken {
        mesh_id: mesh_id.to_string(),
        lighthouse: ip.clone(),
        port: enroll_port,
        bearer: legacy.bearer,
        fp: Some(identity.fingerprint.clone()),
    };
    let join_token = v3.encode();

    // FOUND-NEBULA-4 — materialize THIS lighthouse's /etc/nebula config INLINE,
    // before starting nebula.service. The nebula_supervisor worker only
    // materializes on LEADER-promotion, but a freshly-founded lighthouse cannot
    // take leadership: the legacy leader lock lives on QNM-Shared
    // (/mnt/mesh-storage/.mackesd-leader.lock), which the founder hasn't mounted
    // yet (and which SUBSTRATE-V2 is removing). So the supervisor never runs and
    // nebula starts against the STOCK example config.yml (pki → host.crt/ca.crt
    // that don't exist) → crash-loop → no overlay. The join path already
    // materializes inline (persist_bundle → materialize_config); found must do
    // the same with its founding bundle. ConfigRole::Host → am_lighthouse: true;
    // materialize_config writes ca.crt/host.crt/host.key + the rendered config
    // and removes the stock config.yml. Idempotent: the supervisor re-renders
    // identically once leadership is later taken. (Diagnosed live via the
    // BUILD-PLATFORM-5 L2 mini-mesh, 2026-06-22.)
    let founding_bundle =
        mackesd_core::ca::bundle::read_bundle(&report.bundle_path).map_err(|e| {
            anyhow::anyhow!("reading the founding bundle to materialize /etc/nebula: {e}")
        })?;
    mackesd_core::workers::nebula_supervisor::materialize_config(
        std::path::Path::new("/etc/nebula"),
        &founding_bundle,
        mackesd_core::workers::nebula_supervisor::ConfigRole::Host,
        &[],
        &root,
        None,
    )
    .map_err(|e| anyhow::anyhow!("materializing /etc/nebula for the founding lighthouse: {e}"))?;

    // Bring the node fully live + boot-durable: enable+start the overlay, the
    // worker daemon (activates the /enroll listener), and the health watchdog.
    // `enable` makes each start at boot independently — nebula.service ships
    // disabled, and was previously only `start`ed, so a reboot left the overlay
    // down until the supervisor happened to revive it (ONBOARD-9).
    enable_now_service("nebula.service");

    // CONNECT-4 — the founding lighthouse is an ingress node: stand up Caddy.
    provision_caddy_if_lighthouse(parsed);

    // SETUP-7 — capture the founding facts for idempotent re-convergence.
    emit_site_yml_best_effort(parsed.as_str(), mesh_id, vec![report.overlay_ip.clone()]);

    enable_now_service("mackesd.service");
    enable_now_service("mesh-health.timer");

    println!(
        "mesh `{}` founded — lighthouse {} ({})",
        report.mesh_id, node_id, report.overlay_ip
    );
    if let Some(r) = &report.pinned_role {
        println!("role pinned: {r}");
    }
    println!(
        "/enroll endpoint: https://{ip}:{enroll_port}  (cert fp {})",
        identity.fingerprint
    );
    println!("bundle: {}", report.bundle_path.display());
    println!("services: nebula + mackesd + mesh-health enabled (boot-durable) and running");
    // HA-4 — a freshly-founded mesh has exactly one lighthouse, so it is below
    // the HA floor: a single lighthouse is a SPOF for relay/discovery and (under
    // SUBSTRATE-V2) the etcd quorum + Mesh-Sync redundancy. Warn, non-blocking —
    // the mesh works with one; healthz reports `degraded: no HA` until a 2nd is
    // enrolled, then clears (the matching half of HA-4).
    println!(
        "\n⚠ HA needs a 2nd lighthouse — this mesh has 1 of {} for failover. \
         Add one with `mackesd join '<token>' --role lighthouse` on another box.",
        mackes_mesh_types::lighthouse::HA_MIN_LIGHTHOUSES
    );
    println!("\nAdd a peer — run this on the joining box:\n  mackesd join '{join_token}'");

    // DAR-18 (Lock 3) — opt-in DevOps backoffice. INTENT-RECORDING + non-destructive:
    // record `/mcnf/backoffice/intent {tier,host,ts}` to etcd and PRINT the gated
    // next step. found itself never provisions the control VM, runs `tofu apply`, or
    // spends — that stays the control VM's job (operator-gated). A failure to record
    // intent is non-fatal: the mesh IS founded; we warn and still print the next step
    // so the operator can re-run `backoffice-up.sh record-intent` by hand.
    if let Some(tier) = backoffice_tier {
        record_backoffice_intent(tier, &report.overlay_ip);
    }

    Ok(())
}

/// DAR-18 — normalize + validate a `--with-backoffice` tier. Accepts only
/// `minimal` / `full` (bare `--with-backoffice` already defaulted to `minimal` at
/// the clap layer). Returns the canonical lowercase tier or a clear error so a
/// typo fails the verb before any mesh side effect. PURE.
///
/// # Errors
/// Returns `Err` for any tier other than `minimal` / `full`.
fn normalize_backoffice_tier(tier: &str) -> anyhow::Result<&'static str> {
    match tier.trim().to_ascii_lowercase().as_str() {
        "minimal" => Ok("minimal"),
        "full" => Ok("full"),
        other => Err(anyhow::anyhow!(
            "unknown --with-backoffice tier `{other}` — expected `minimal` or `full`"
        )),
    }
}

/// DAR-18 — record the backoffice INTENT by shelling out to the orchestrator's
/// `record-intent` mode (the single owner of the `/mcnf/backoffice/intent` etcd
/// write, which resolves endpoints via the shared DAR-1b resolver — never the dead
/// `.192`). Non-destructive: this writes one small non-secret etcd key and prints
/// the gated next command; it does NOT run the heavy bring-up. Best-effort — a
/// failure is warned, not fatal (the mesh is already founded).
///
/// `tier` is the validated `minimal`/`full`; `host` is the founding overlay IP
/// (the control VM defaults to this overlay until one is provisioned).
fn record_backoffice_intent(tier: &str, host: &str) {
    println!("\n--with-backoffice={tier} — recording DevOps backoffice intent…");
    let script = backoffice_up_script_path();
    if !script.is_file() {
        eprintln!(
            "found: backoffice orchestrator not found at {} — record intent by hand:\n  \
             automation/backoffice/backoffice-up.sh record-intent --tier {tier}",
            script.display()
        );
        return;
    }
    let status = std::process::Command::new("bash")
        .arg(&script)
        .arg("record-intent")
        .arg("--tier")
        .arg(tier)
        .arg("--host")
        .arg(host)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!(
            "found: recording backoffice intent exited {} — re-run by hand:\n  \
             {} record-intent --tier {tier}",
            s.code().unwrap_or(-1),
            script.display()
        ),
        Err(e) => eprintln!(
            "found: could not run the backoffice orchestrator ({e}) — re-run by hand:\n  \
             {} record-intent --tier {tier}",
            script.display()
        ),
    }
}

/// Resolve the deployed `backoffice-up.sh` orchestrator path: under `$MCNF_REPO`
/// (the project-wide repo-root convention, matching the secret store) when set,
/// else the default install root. Used by [`record_backoffice_intent`].
fn backoffice_up_script_path() -> std::path::PathBuf {
    let repo = std::env::var_os("MCNF_REPO")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/opt/mcnf"));
    repo.join("automation/backoffice/backoffice-up.sh")
}

#[cfg(test)]
mod found_backoffice_tests {
    //! DAR-18 — the `mackesd found --with-backoffice[=minimal|full]` flag.
    //!
    //! Asserts the clap parse semantics (absent = OFF; bare = minimal; `=full`;
    //! a bogus tier is caught by [`normalize_backoffice_tier`]) and that the flag
    //! is purely ADDITIVE — `found` without it parses byte-for-byte the same
    //! (the regression that found is unchanged when the flag is absent).
    use super::normalize_backoffice_tier;
    use crate::{Cli, Cmd};
    use clap::Parser;

    /// Extract the `with_backoffice` field from a parsed `found` (panics if the
    /// args didn't parse to a `Found`).
    fn parse_found(args: &[&str]) -> Option<String> {
        let cli = Cli::try_parse_from(args).expect("found args should parse");
        match cli.cmd {
            Cmd::Found {
                with_backoffice, ..
            } => with_backoffice,
            other => panic!(
                "expected Cmd::Found, got something else: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn found_without_flag_leaves_backoffice_off() {
        // The regression guard: a plain `found` records NO backoffice intent.
        assert_eq!(parse_found(&["mackesd", "found", "home-mesh"]), None);
    }

    #[test]
    fn bare_with_backoffice_defaults_to_minimal() {
        // `--with-backoffice` with no value = the bare default `minimal`.
        assert_eq!(
            parse_found(&["mackesd", "found", "home-mesh", "--with-backoffice"]),
            Some("minimal".to_string())
        );
    }

    #[test]
    fn with_backoffice_full_parses() {
        assert_eq!(
            parse_found(&["mackesd", "found", "home-mesh", "--with-backoffice=full"]),
            Some("full".to_string())
        );
        // The space form parses too.
        assert_eq!(
            parse_found(&[
                "mackesd",
                "found",
                "home-mesh",
                "--with-backoffice",
                "minimal"
            ]),
            Some("minimal".to_string())
        );
    }

    #[test]
    fn with_backoffice_keeps_the_other_found_flags() {
        // The new flag is additive — the existing flags still parse alongside it.
        let cli = Cli::try_parse_from([
            "mackesd",
            "found",
            "home-mesh",
            "--external-addr",
            "203.0.113.7",
            "--role",
            "lighthouse",
            "--with-backoffice=full",
        ])
        .expect("parse");
        match cli.cmd {
            Cmd::Found {
                mesh_id,
                external_addr,
                role,
                with_backoffice,
                ..
            } => {
                assert_eq!(mesh_id, "home-mesh");
                assert_eq!(external_addr, "203.0.113.7");
                assert_eq!(role, "lighthouse");
                assert_eq!(with_backoffice.as_deref(), Some("full"));
            }
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn normalize_tier_accepts_minimal_and_full_case_insensitively() {
        assert_eq!(normalize_backoffice_tier("minimal").unwrap(), "minimal");
        assert_eq!(normalize_backoffice_tier("full").unwrap(), "full");
        assert_eq!(normalize_backoffice_tier("FULL").unwrap(), "full");
        assert_eq!(normalize_backoffice_tier("  Minimal ").unwrap(), "minimal");
    }

    #[test]
    fn normalize_tier_rejects_a_bogus_tier() {
        let e = normalize_backoffice_tier("bogus").unwrap_err().to_string();
        assert!(e.contains("bogus"), "{e}");
        assert!(e.contains("minimal") && e.contains("full"), "{e}");
        assert!(normalize_backoffice_tier("").is_err());
    }
}
