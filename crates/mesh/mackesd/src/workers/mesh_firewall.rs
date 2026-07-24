//! MESH-A-5.2 (v5.0.0) — mesh-coordinated firewall DROP enforcement.
//!
//! On a ~1-min tick (R8-Q44 propagation), reads the mesh-synced
//! surrounding-host trust consensus ([`read_all_surrounding`]),
//! computes the blocked-host IPs ([`blocked_ips`] — roaming-aware), and
//! reconciles firewalld source-DROP rich-rules against an in-memory
//! shadow set: adds a rule for each newly-blocked IP, removes it when a
//! host is unblocked, `--reload` on change. Every peer runs this worker
//! and applies the same shared Blocked set locally, so an operator's
//! Block decision propagates mesh-wide within ~1 minute (mesh-sync
//! latency + tick).
//!
//! The DROP rule is the MESH-A-5.1 [`drop_rich_rule_body`]. Silent
//! no-op when `firewall-cmd` is absent (lighthouse / container-stripped
//! peer). The reconcile diff is pure + unit-tested; `firewall-cmd`
//! execution is HW-bench-gated (§0.15).

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use crate::surrounding_hosts::{blocked_ips, drop_rich_rule_body, read_all_surrounding_checked};

use super::{ShutdownToken, Worker};

/// Reconcile cadence — 1 minute (R8-Q44 propagation budget).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Worker handle.
pub struct MeshFirewallWorker {
    /// Surrounding-host snapshot root (`<workgroup-root>/surrounding`).
    base_dir: PathBuf,
    tick: Duration,
    /// IPs currently dropped (in-memory shadow). Rebuilt empty on boot —
    /// firewalld holds the `--permanent` rules durably and the next
    /// reconcile re-converges.
    active: Mutex<BTreeSet<String>>,
}

impl MeshFirewallWorker {
    /// Construct with production defaults. `base_dir` is the
    /// `surrounding` snapshot root.
    #[must_use]
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            tick: DEFAULT_TICK_INTERVAL,
            active: Mutex::new(BTreeSet::new()),
        }
    }

    /// Override the reconcile cadence. Used in tests.
    #[must_use]
    pub fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    fn tick_once(&self) {
        let desired = match desired_blocked_ips(&self.base_dir) {
            Ok(desired) => desired,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    path = %self.base_dir.join("trust.json").display(),
                    "mesh_firewall: trust authority rejected; retaining current DROP set"
                );
                return;
            }
        };
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (to_add, to_remove) = reconcile(&active, &desired);
        let mut changed = false;
        for ip in &to_add {
            if run_firewall_cmd(&add_drop_args(ip)) {
                active.insert(ip.clone());
                changed = true;
            } else {
                tracing::warn!(%ip, "mesh_firewall: add-rich-rule DROP failed");
            }
        }
        for ip in &to_remove {
            // Drop from the shadow regardless so a transient firewalld
            // error never pins a stale block; the next tick re-adds if
            // the host is still blocked.
            run_firewall_cmd(&remove_drop_args(ip));
            active.remove(ip);
            changed = true;
        }
        if changed {
            let _ = run_firewall_cmd(&["--reload".to_string()]);
        }
    }
}

fn desired_blocked_ips(base_dir: &std::path::Path) -> std::io::Result<BTreeSet<String>> {
    Ok(blocked_ips(&read_all_surrounding_checked(base_dir)?)
        .into_iter()
        .collect())
}

/// Pure reconcile — `(to_add, to_remove)` = `(desired − active,
/// active − desired)`.
fn reconcile(active: &BTreeSet<String>, desired: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    (
        desired.difference(active).cloned().collect(),
        active.difference(desired).cloned().collect(),
    )
}

/// `firewall-cmd --permanent --add-rich-rule=<drop rule>` args.
fn add_drop_args(ip: &str) -> Vec<String> {
    vec![
        "--permanent".to_string(),
        format!("--add-rich-rule={}", drop_rich_rule_body(ip)),
    ]
}

/// `firewall-cmd --permanent --remove-rich-rule=<drop rule>` args.
fn remove_drop_args(ip: &str) -> Vec<String> {
    vec![
        "--permanent".to_string(),
        format!("--remove-rich-rule={}", drop_rich_rule_body(ip)),
    ]
}

fn run_firewall_cmd(args: &[String]) -> bool {
    // EFF-20 — bound firewall-cmd so a wedged invocation can't pin the
    // runtime thread the worker tick runs on.
    let mut cmd = Command::new("firewall-cmd");
    cmd.args(args);
    crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .map(|s| s.success())
        .unwrap_or(false)
}

fn binary_present(bin: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .is_ok()
}

#[async_trait::async_trait]
impl Worker for MeshFirewallWorker {
    fn name(&self) -> &'static str {
        "mesh_firewall"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        if !binary_present("firewall-cmd") {
            tracing::debug!("mesh_firewall: firewall-cmd absent; worker idle");
            return Ok(());
        }
        let mut tick = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn reconcile_computes_add_and_remove_deltas() {
        let active = set(&["10.0.0.5", "10.0.0.9"]);
        let desired = set(&["10.0.0.9", "10.0.0.20"]);
        let (to_add, to_remove) = reconcile(&active, &desired);
        assert_eq!(to_add, vec!["10.0.0.20"], "newly blocked");
        assert_eq!(to_remove, vec!["10.0.0.5"], "unblocked");
    }

    #[test]
    fn reconcile_noop_when_equal() {
        let s = set(&["10.0.0.5"]);
        let (a, r) = reconcile(&s, &s);
        assert!(a.is_empty() && r.is_empty());
    }

    #[test]
    fn drop_args_use_permanent_rich_rule() {
        let add = add_drop_args("10.0.0.5");
        assert_eq!(add[0], "--permanent");
        assert!(add[1].starts_with("--add-rich-rule="));
        assert!(add[1].contains(r#"source address="10.0.0.5""#));
        assert!(add[1].contains("drop"));
        let rem = remove_drop_args("10.0.0.5");
        assert!(rem[1].starts_with("--remove-rich-rule="));
    }

    #[test]
    fn trust_consumer_rejects_tamper_instead_of_unblocking() {
        use crate::surrounding_hosts::{
            save_trust_store_with_signer, HostType, SurroundingHost, TrustState, TrustStore,
        };

        let tmp = tempfile::tempdir().unwrap();
        let peer = tmp.path().join("peer-a");
        std::fs::create_dir_all(&peer).unwrap();
        let host = SurroundingHost {
            ip: "10.0.0.5".into(),
            mac: "aa:bb".into(),
            vendor: String::new(),
            hostname: String::new(),
            services: Vec::new(),
            host_type: HostType::Unknown,
            trust: TrustState::Unknown,
            first_seen_ms: 1,
            last_seen_ms: 1,
        };
        std::fs::write(
            peer.join("20260101T000000-a.json"),
            serde_json::to_vec(&vec![host]).unwrap(),
        )
        .unwrap();
        let signer = ed25519_dalek::SigningKey::from_bytes(&[17_u8; 32]);
        let mut trust = TrustStore::new();
        trust.insert("aa:bb".into(), TrustState::Blocked);
        let path = tmp.path().join("trust.json");
        save_trust_store_with_signer(&path, &trust, &signer).unwrap();
        assert_eq!(desired_blocked_ips(tmp.path()).unwrap(), set(&["10.0.0.5"]));

        let mut envelope: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        envelope["entries"]["aa:bb"] = serde_json::json!("trusted");
        crate::ca::seal::write_atomic_sealed(
            &path,
            serde_json::to_string(&envelope).unwrap().as_bytes(),
        )
        .unwrap();
        assert!(
            desired_blocked_ips(tmp.path()).is_err(),
            "the firewall tick must retain its active DROP set on bad provenance"
        );
    }
}
