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

use crate::surrounding_hosts::{blocked_ips, drop_rich_rule_body, read_all_surrounding};

use super::{ShutdownToken, Worker};

/// Reconcile cadence — 1 minute (R8-Q44 propagation budget).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Worker handle.
pub struct MeshFirewallWorker {
    /// Surrounding-host snapshot root (`~/.local/share/mde/surrounding`).
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
        let desired: BTreeSet<String> = blocked_ips(&read_all_surrounding(&self.base_dir))
            .into_iter()
            .collect();
        let mut active = self.active.lock().expect("active mutex");
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
    Command::new("firewall-cmd")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn binary_present(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
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
}
