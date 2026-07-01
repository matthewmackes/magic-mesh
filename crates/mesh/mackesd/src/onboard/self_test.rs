//! OW-2 — `mackesd onboard self-test`: a node self-diagnostic.
//!
//! The shape mirrors [`crate::workers::kvm_health`]: an impure probe seam
//! ([`gather`]) collects live facts off the node, and a pure fold ([`assemble`])
//! turns those facts into a headless [`SelfTestReport`] with a critical-fail exit
//! code. The pure fold is what the unit tests pin (given probe results → report /
//! exit code); the shell never appears in a test.
//!
//! Checks:
//! * **KVM stack** — how many of [`crate::kvm::KVM_SERVICES`] are `systemctl
//!   is-active`. Non-critical: a degraded stack is a capability warning (a tiny
//!   VPS lighthouse may not run the full libvirt set), not a mesh-fatal fault.
//! * **Mesh peer directory** — how many peers the node's directory knows.
//!   Critical: a node that cannot read its own directory (zero peers, not even
//!   itself) has no working mesh substrate.
//! * **Identity** — the per-node Ed25519 signing key
//!   ([`crate::node_key::DEFAULT_KEY_PATH`]). Critical: a node with no signing
//!   identity cannot enroll, sign heartbeats, or sign audit rows.
//! * **CA** — whether this node holds a minted mesh CA (the `nebula_ca` store
//!   rows). Non-critical: only a lighthouse / founder holds the CA; a plain peer
//!   legitimately has none.

use std::path::Path;

/// One check's outcome. `Warn` never fails the run; `Fail` fails it only when the
/// check is [`Check::critical`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// The check passed.
    Pass,
    /// A non-fatal concern (surfaced, never fails the run).
    Warn,
    /// The check failed (fails the run iff [`Check::critical`]).
    Fail,
}

impl CheckStatus {
    /// Short tag for the human report line, e.g. `ok` / `warn` / `FAIL`.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Pass => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

/// One diagnostic line in the report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    /// Stable machine id (`kvm` / `mesh` / `identity` / `ca`).
    pub id: &'static str,
    /// Human label for the text report.
    pub label: &'static str,
    /// Pass / warn / fail.
    pub status: CheckStatus,
    /// Whether a [`CheckStatus::Fail`] here fails the whole run (non-zero exit).
    pub critical: bool,
    /// One-line human detail.
    pub detail: String,
}

/// The raw facts [`gather`] collects off the live node — the seam between the
/// (impure) probes and the pure [`assemble`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probes {
    /// This node's id (stamped into the report).
    pub node_id: String,
    /// KVM services reporting active (`systemctl is-active`).
    pub kvm_active: usize,
    /// KVM services in the probed catalog.
    pub kvm_total: usize,
    /// Peers the node's directory knows (including itself).
    pub mesh_peers: usize,
    /// Whether the per-node signing key is present on disk.
    pub identity_present: bool,
    /// Whether this node holds a minted mesh CA.
    pub ca_present: bool,
}

/// The assembled self-diagnostic report — the headless body both front-ends read
/// and the CLI prints (as JSON with `--json`, else as text).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SelfTestReport {
    /// The node this report describes.
    pub node_id: String,
    /// Every check, in report order.
    pub checks: Vec<Check>,
    /// `true` iff no *critical* check failed — the pass/fail verdict.
    pub ok: bool,
}

impl SelfTestReport {
    /// Process exit code: `0` when [`Self::ok`], else `1`. The CLI + a front-end
    /// that shells the verb key off this.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.ok)
    }

    /// The multi-line human report.
    #[must_use]
    pub fn human(&self) -> String {
        use std::fmt::Write;
        let mut out = format!(
            "self-test: node {} — {}\n",
            self.node_id,
            if self.ok { "OK" } else { "FAILED" }
        );
        for c in &self.checks {
            let crit = if c.critical { " (critical)" } else { "" };
            let _ = writeln!(
                out,
                "  [{}] {}: {}{crit}",
                c.status.tag(),
                c.label,
                c.detail
            );
        }
        out
    }
}

/// Pure fold: turn gathered [`Probes`] into a [`SelfTestReport`]. No I/O, no
/// clock, no systemd — fully unit-testable. The verdict ([`SelfTestReport::ok`])
/// is `false` iff a *critical* check failed.
#[must_use]
pub fn assemble(p: &Probes) -> SelfTestReport {
    let mut checks = Vec::with_capacity(4);

    // KVM — non-critical capability check.
    let kvm_status = if p.kvm_total > 0 && p.kvm_active == p.kvm_total {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    checks.push(Check {
        id: "kvm",
        label: "KVM virtualization stack",
        status: kvm_status,
        critical: false,
        detail: format!("{}/{} services active", p.kvm_active, p.kvm_total),
    });

    // Mesh peer directory — critical. Zero peers (not even self) means the node
    // can't read its own directory; one peer means it's healthy but isolated.
    let (mesh_status, mesh_detail) = match p.mesh_peers {
        0 => (
            CheckStatus::Fail,
            "no peers in the directory (unreadable / node not registered)".to_string(),
        ),
        1 => (
            CheckStatus::Warn,
            "only this node visible (mesh of one / isolated)".to_string(),
        ),
        n => (CheckStatus::Pass, format!("{n} peers reachable")),
    };
    checks.push(Check {
        id: "mesh",
        label: "Mesh peer directory",
        status: mesh_status,
        critical: true,
        detail: mesh_detail,
    });

    // Identity — critical.
    checks.push(Check {
        id: "identity",
        label: "Node identity key",
        status: if p.identity_present {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        critical: true,
        detail: if p.identity_present {
            format!("present ({})", crate::node_key::DEFAULT_KEY_PATH)
        } else {
            format!("absent ({})", crate::node_key::DEFAULT_KEY_PATH)
        },
    });

    // CA — non-critical (a plain peer holds no CA).
    checks.push(Check {
        id: "ca",
        label: "Mesh CA",
        status: if p.ca_present {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        critical: false,
        detail: if p.ca_present {
            "present (this node holds a minted mesh CA)".to_string()
        } else {
            "absent (peer node — the CA lives on the lighthouse/founder)".to_string()
        },
    });

    let ok = !checks
        .iter()
        .any(|c| c.critical && c.status == CheckStatus::Fail);
    SelfTestReport {
        node_id: p.node_id.clone(),
        checks,
        ok,
    }
}

/// Impure probe shell: collect the live facts for `node_id` off this node.
///
/// Best effort — every probe degrades to a negative/zero reading rather than an
/// error, so the diagnostic always produces a report (the report *is* the error
/// signal).
///
/// * KVM: `systemctl is-active --quiet <unit>` over [`crate::kvm::KVM_SERVICES`].
/// * Mesh: the peer directory under `workgroup_root` (the durable shared-substrate
///   copy every reader falls back to).
/// * Identity: the signing key file's presence.
/// * CA: a `nebula_ca` row count in the store at `db_path` (never *creates* the
///   store — an absent db reads as no CA).
#[must_use]
pub fn gather(node_id: &str, db_path: &Path, workgroup_root: &Path) -> Probes {
    let kvm_total = crate::kvm::KVM_SERVICES.len();
    let kvm_active = crate::kvm::KVM_SERVICES
        .iter()
        .filter(|s| unit_is_active(s.unit))
        .count();

    let mesh_peers = {
        let dir = mackes_mesh_types::peers::peers_dir(workgroup_root);
        mackes_mesh_types::peers::read_peers(&dir).len()
    };

    let identity_present = Path::new(crate::node_key::DEFAULT_KEY_PATH).exists();
    let ca_present = ca_rows_present(db_path);

    Probes {
        node_id: node_id.to_string(),
        kvm_active,
        kvm_total,
        mesh_peers,
        identity_present,
        ca_present,
    }
}

/// `systemctl is-active --quiet <unit>` — exit 0 ⇒ active. A missing systemctl (a
/// dev box) or any non-zero exit reads as inactive.
fn unit_is_active(unit: &str) -> bool {
    matches!(
        std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", unit])
            .status(),
        Ok(status) if status.success()
    )
}

/// Whether the store at `db_path` holds at least one minted CA. Read-only: an
/// absent db (or any open/query error) reads as "no CA" and never creates the
/// store, so a self-test leaves no side effects on an unprovisioned node.
fn ca_rows_present(db_path: &Path) -> bool {
    if !db_path.exists() {
        return false;
    }
    let Ok(conn) = crate::store::open(db_path) else {
        return false;
    };
    conn.query_row("SELECT COUNT(*) FROM nebula_ca", [], |r| r.get::<_, i64>(0))
        .is_ok_and(|n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes(kvm: (usize, usize), peers: usize, id: bool, ca: bool) -> Probes {
        Probes {
            node_id: "node-a".to_string(),
            kvm_active: kvm.0,
            kvm_total: kvm.1,
            mesh_peers: peers,
            identity_present: id,
            ca_present: ca,
        }
    }

    fn check<'a>(r: &'a SelfTestReport, id: &str) -> &'a Check {
        r.checks.iter().find(|c| c.id == id).expect("check present")
    }

    #[test]
    fn healthy_node_passes_with_exit_zero() {
        let r = assemble(&probes((6, 6), 3, true, true));
        assert!(r.ok);
        assert_eq!(r.exit_code(), 0);
        assert_eq!(check(&r, "kvm").status, CheckStatus::Pass);
        assert_eq!(check(&r, "mesh").status, CheckStatus::Pass);
        assert_eq!(check(&r, "identity").status, CheckStatus::Pass);
        assert_eq!(check(&r, "ca").status, CheckStatus::Pass);
        // Report names the node + every check.
        assert_eq!(r.node_id, "node-a");
        assert_eq!(r.checks.len(), 4);
    }

    #[test]
    fn missing_identity_is_a_critical_fail() {
        // Identity absent → critical Fail → non-zero exit, even with everything
        // else green.
        let r = assemble(&probes((6, 6), 3, false, true));
        assert!(!r.ok);
        assert_eq!(r.exit_code(), 1);
        let idc = check(&r, "identity");
        assert_eq!(idc.status, CheckStatus::Fail);
        assert!(idc.critical);
        assert!(idc.detail.contains(crate::node_key::DEFAULT_KEY_PATH));
    }

    #[test]
    fn empty_directory_is_a_critical_fail() {
        // Zero peers (not even self) → the mesh substrate is unreadable → critical.
        let r = assemble(&probes((6, 6), 0, true, true));
        assert!(!r.ok);
        assert_eq!(r.exit_code(), 1);
        let mesh = check(&r, "mesh");
        assert_eq!(mesh.status, CheckStatus::Fail);
        assert!(mesh.critical);
    }

    #[test]
    fn isolated_node_warns_but_still_passes() {
        // A mesh of one (only self visible) is a warning, not a failure.
        let r = assemble(&probes((6, 6), 1, true, true));
        assert!(r.ok);
        assert_eq!(r.exit_code(), 0);
        assert_eq!(check(&r, "mesh").status, CheckStatus::Warn);
    }

    #[test]
    fn degraded_kvm_and_absent_ca_warn_without_failing() {
        // Both are non-critical: a degraded KVM stack + a peer that holds no CA
        // still pass (exit 0) with warnings surfaced.
        let r = assemble(&probes((4, 6), 3, true, false));
        assert!(r.ok);
        assert_eq!(r.exit_code(), 0);
        assert_eq!(check(&r, "kvm").status, CheckStatus::Warn);
        assert_eq!(check(&r, "kvm").detail, "4/6 services active");
        assert_eq!(check(&r, "ca").status, CheckStatus::Warn);
        assert!(!check(&r, "kvm").critical);
        assert!(!check(&r, "ca").critical);
    }

    #[test]
    fn empty_kvm_catalog_warns_not_passes() {
        // A 0/0 KVM reading must not report Pass (there is nothing healthy).
        let r = assemble(&probes((0, 0), 3, true, true));
        assert_eq!(check(&r, "kvm").status, CheckStatus::Warn);
        // ...but KVM is non-critical, so the run still passes.
        assert!(r.ok);
    }

    #[test]
    fn json_and_human_render_the_verdict() {
        let r = assemble(&probes((6, 6), 0, true, true));
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(json["ok"], false);
        assert_eq!(json["node_id"], "node-a");
        assert_eq!(json["checks"][1]["status"], "fail"); // mesh, lowercased
        let human = r.human();
        assert!(human.contains("node node-a — FAILED"));
        assert!(human.contains("[FAIL] Mesh peer directory"));
        assert!(human.contains("(critical)"));
    }

    #[test]
    fn status_tags_are_stable() {
        assert_eq!(CheckStatus::Pass.tag(), "ok");
        assert_eq!(CheckStatus::Warn.tag(), "warn");
        assert_eq!(CheckStatus::Fail.tag(), "FAIL");
    }
}
