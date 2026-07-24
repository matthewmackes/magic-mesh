//! `Remediate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;
use anyhow::Context;

const JOBS_LAUNCH_TOPIC: &str = "action/jobs/launch";
const JOBS_LAUNCH_AUTH_VERB: &str = "jobs-launch";
const JOBS_LAUNCH_AUTH_NODE: &str = "jobs";
const JOBS_LAUNCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Handle the `remediate` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: RemediateCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PLANES-11 — the remediation layer. Wires PLANES-13's
        // policy engine (which had no caller) to the job system:
        // evaluate policies → match plans → fire signed bundles.
        use mackesd_core::{policy_engine, remediation};
        let root = mackesd_core::default_qnm_shared_root();
        match cmd {
            RemediateCmd::Plans { json } => {
                let plans = remediation::load_plans(&root);
                if json {
                    println!("{}", serde_json::to_string(&plans)?);
                } else {
                    println!(
                        "{:<22} {:<20} {:<22} {:<5}",
                        "PLAN", "POLICY", "TEMPLATE", "AUTO"
                    );
                    for p in &plans {
                        println!(
                            "{:<22} {:<20} {:<22} {:<5}",
                            p.name, p.policy, p.template, p.auto
                        );
                    }
                }
                return Ok(());
            }
            RemediateCmd::Match { json } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let svc = mackesd_core::ipc::directory::DirectoryService::new(
                    &root,
                    Some(db_path.clone()),
                );
                let dir = svc.build_directory(now);
                let peers: Vec<(String, serde_json::Value)> = dir["peers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["hostname"].as_str().map(|h| (h.to_string(), p.clone())))
                    .collect();
                let policies = policy_engine::load_policies(&root);
                let violations = policy_engine::evaluate(&policies, &peers);
                let plans = remediation::load_plans(&root);
                let matched = remediation::match_all(&plans, &violations);
                if json {
                    println!("{}", serde_json::to_string(&matched)?);
                } else if matched.is_empty() {
                    println!("no drift — every policy holds across {} peers", peers.len());
                } else {
                    println!(
                        "{:<14} {:<20} {:<8} {:<22} {:<5}",
                        "PEER", "POLICY", "SEV", "PLAN", "AUTO"
                    );
                    for m in &matched {
                        println!(
                            "{:<14} {:<20} {:<8} {:<22} {:<5}",
                            m.violation.peer,
                            m.violation.policy,
                            m.violation.severity,
                            m.plan.as_deref().unwrap_or("(none)"),
                            m.auto
                        );
                    }
                }
                return Ok(());
            }
            RemediateCmd::Fire { plan, peer } => {
                let plans = remediation::load_plans(&root);
                let Some(p) = plans.iter().find(|x| x.name == plan) else {
                    anyhow::bail!("no remediation plan named '{plan}' (mded remediate plans)");
                };
                // Bind the event vars from a synthesized violation
                // for this (policy, peer) — the operator-fire path.
                let v = policy_engine::Violation {
                    policy: p.policy.clone(),
                    peer: peer.clone(),
                    severity: "warn".into(),
                    detail: format!("operator fire of '{plan}'"),
                };
                let vars = remediation::bind_vars(p, &v);
                let body = serde_json::json!({
                    "playbook": p.template,
                    "targets": { "peers": [peer] },
                    "vars": vars,
                });
                let body = signed_jobs_launch_body(body)?;
                let reply = submit_jobs_launch(&body)?;
                // Loud (W42): the launch reply — run id + resolved
                // targets — prints for the operator / audit trail.
                println!("{reply}");
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Add the same root-only, exact-body-bound capability that the jobs Bus
/// responder verifies. The CLI is an explicit authority path, but it must not
/// turn that distinction into an unauthenticated mutation path.
fn signed_jobs_launch_body(body: serde_json::Value) -> anyhow::Result<String> {
    let signer = production_jobs_signer()?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("system clock is before the Unix epoch"))
        .and_then(|duration| {
            i64::try_from(duration.as_millis())
                .map_err(|_| anyhow::anyhow!("system clock is beyond the capability range"))
        })?;
    let nonce = uuid::Uuid::new_v4().to_string();
    signed_jobs_launch_body_with_signer(&signer, body, now_ms, &nonce)
}

/// Load the same sealed root credential used by the jobs responder. This
/// binary owns the CLI command line, so it cannot call the library's
/// `pub(crate)` producer helper; keeping the loader here preserves the
/// root-only + no-environment-secret policy at this crate boundary.
fn production_jobs_signer() -> anyhow::Result<mackes_mesh_types::cloud::CloudArmSigner> {
    use mackes_mesh_types::cloud::{
        decode_cloud_arm_credential, CloudArmSigner, CLOUD_ARM_CREDENTIAL,
    };

    if !rustix::process::geteuid().is_root() {
        anyhow::bail!("jobs launch authorization requires the root service process");
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| anyhow::anyhow!("systemd action credential is unavailable"))?;
    let path = directory.join(CLOUD_ARM_CREDENTIAL);
    let raw = std::fs::read(&path)
        .with_context(|| format!("reading systemd action credential {}", path.display()))?;
    let key = decode_cloud_arm_credential(&raw).map_err(|error| anyhow::anyhow!(error))?;
    CloudArmSigner::new(key).map_err(|error| anyhow::anyhow!(error))
}

/// Testable signing seam for the direct CLI authority path.
fn signed_jobs_launch_body_with_signer(
    signer: &dyn mackes_mesh_types::cloud::CloudTokenSigner,
    mut body: serde_json::Value,
    now_ms: i64,
    nonce: &str,
) -> anyhow::Result<String> {
    use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmedToken};

    let target = {
        let object = body
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("jobs launch body must be a JSON object"))?;
        object.remove("armed_token");
        object.insert(
            "schema_version".to_string(),
            serde_json::json!(mackesd_core::ipc::action_auth::ACTION_SCHEMA_VERSION),
        );
        object
            .get("playbook")
            .and_then(serde_json::Value::as_str)
            .filter(|playbook| !playbook.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("jobs launch body needs a non-empty `playbook`"))?
    };
    let unsigned = serde_json::to_string(&body).context("serializing unsigned jobs launch")?;
    let token = CloudArmedToken::mint(
        signer,
        nonce,
        now_ms.saturating_add(mackesd_core::ipc::action_auth::MAX_AUTH_TTL_MS),
        JOBS_LAUNCH_AUTH_VERB,
        JOBS_LAUNCH_AUTH_NODE,
        &target,
        &cloud_request_digest(&unsigned).map_err(|error| anyhow::anyhow!(error))?,
    )
    .encode();
    body.as_object_mut()
        .expect("jobs launch body was validated as an object")
        .insert("armed_token".to_string(), serde_json::Value::String(token));
    serde_json::to_string(&body).context("serializing signed jobs launch")
}

/// Submit remediation through the real Bus request/reply path. There is no
/// local `build_reply` fallback: if the responder is absent, the CLI fails
/// rather than creating a privileged run outside the Bus authority boundary.
fn submit_jobs_launch(body: &str) -> anyhow::Result<String> {
    let bus_root = mde_bus::default_data_dir()
        .ok_or_else(|| anyhow::anyhow!("no Bus data directory; cannot submit jobs launch"))?;
    let persist = mde_bus::persist::Persist::open(bus_root).context("opening Bus persist")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .context("building jobs launch RPC runtime")?;
    let reply = runtime.block_on(mde_bus::rpc::request(
        &persist,
        JOBS_LAUNCH_TOPIC,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(body),
        JOBS_LAUNCH_TIMEOUT,
    ))?;
    reply
        .body
        .ok_or_else(|| anyhow::anyhow!("jobs launch responder returned an empty reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::CloudArmSigner;

    const AUTH_KEY: &[u8] = b"remediate-cli-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    #[test]
    fn direct_fire_body_is_signed_for_the_jobs_bus_authority_context() {
        let signer = CloudArmSigner::new(AUTH_KEY.to_vec()).unwrap();
        let body = signed_jobs_launch_body_with_signer(
            &signer,
            serde_json::json!({
                "playbook": "repair-peer.yml",
                "targets": { "peers": ["oak"] },
            }),
            AUTH_NOW,
            "remediate-cli-once-012345678901234567890123",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let token = value["armed_token"].as_str().unwrap();
        let parsed = mackes_mesh_types::cloud::CloudArmedToken::parse(token).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(parsed.verb, JOBS_LAUNCH_AUTH_VERB);
        assert_eq!(parsed.node, JOBS_LAUNCH_AUTH_NODE);
        assert_eq!(parsed.target, "repair-peer.yml");

        assert!(signer.verify_payload(&parsed.signing_payload(), &parsed.signature));
        assert_eq!(
            parsed.request_sha256,
            mackes_mesh_types::cloud::cloud_request_digest(&body).unwrap()
        );
    }

    #[test]
    fn direct_fire_body_tampering_is_rejected_without_consuming_the_capability() {
        let signer = CloudArmSigner::new(AUTH_KEY.to_vec()).unwrap();
        let body = signed_jobs_launch_body_with_signer(
            &signer,
            serde_json::json!({
                "playbook": "repair-peer.yml",
                "targets": { "peers": ["oak"] },
            }),
            AUTH_NOW,
            "remediate-cli-tamper-01234567890123456789012",
        )
        .unwrap();
        let tampered = body.replace("repair-peer.yml", "destroy-peer.yml");
        let tampered_value: serde_json::Value = serde_json::from_str(&tampered).unwrap();
        let tampered_token = mackes_mesh_types::cloud::CloudArmedToken::parse(
            tampered_value["armed_token"].as_str().unwrap(),
        )
        .unwrap();
        assert_ne!(
            tampered_token.request_sha256,
            mackes_mesh_types::cloud::cloud_request_digest(&tampered).unwrap()
        );
        let parsed = mackes_mesh_types::cloud::CloudArmedToken::parse(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["armed_token"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(signer.verify_payload(&parsed.signing_payload(), &parsed.signature));
    }

    #[test]
    fn direct_fire_requires_a_playbook_before_minting_authority() {
        let signer = CloudArmSigner::new(AUTH_KEY.to_vec()).unwrap();
        let error = signed_jobs_launch_body_with_signer(
            &signer,
            serde_json::json!({ "targets": { "peers": ["oak"] } }),
            AUTH_NOW,
            "remediate-cli-missing-playbook-0123456789",
        )
        .unwrap_err();
        assert!(error.to_string().contains("playbook"));
    }
}
