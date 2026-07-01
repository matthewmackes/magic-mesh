//! OW-2 — `mackesd onboard self-test`: a node self-diagnostic.
//!
//! The shape mirrors [`crate::workers::kvm_health`]: an impure probe seam
//! ([`gather`]) collects live facts off the node, and a pure fold ([`assemble`])
//! turns those facts into a headless [`SelfTestReport`] with a critical-fail exit
//! code. The pure fold is what the unit tests pin (given probe results → report /
//! exit code); the shell never appears in a test.
//!
//! OW-2 checks:
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
//!
//! OW-10 per-item checks — each pairs a pure classification in [`assemble`] with
//! a LIVE probe fenced behind the impure [`gather`] seam. A probe that cannot run
//! headless (no overlay, no systemd, no `nebula-cert`) yields a *typed*
//! [`CheckStatus::Unknown`] — never a faked pass and never a red fail (§7-legal):
//! * **Overlay reachable** — is the Nebula overlay up + a peer answering over the
//!   tunnel (reuses [`crate::transport_probe::probe_rtt`], the TCP-through-tunnel
//!   probe the latency/lighthouse workers time). Critical: an enrolled node that
//!   cannot reach any peer over the datapath is partitioned — but a headless box
//!   or a mesh-of-one is [`CheckStatus::Unknown`], not a fail.
//! * **Role daemons active** — how many of the systemd units this node's role
//!   runs are active, over the reused role→units model
//!   ([`crate::onboard::role_provision::plan`], never redefined). Non-critical
//!   (a systemd-units surface like KVM); [`CheckStatus::Unknown`] when systemd is
//!   absent, [`CheckStatus::Skipped`] when no role is pinned.
//! * **CA-signed cert** — this node holds a live, unexpired cert signed under the
//!   mesh CA (`/etc/nebula/host.crt`), distinct from OW-2's "does this node hold a
//!   minted CA". Reuses [`crate::ca::blocklist::fingerprint_cert_pem`] (is the
//!   cert real + parseable) + [`crate::ca::expiry`] (unexpired). Non-critical;
//!   [`CheckStatus::Unknown`] when `nebula-cert` can't run the expiry probe.
//! * **Lighthouse pingable** — a configured lighthouse answers over the overlay
//!   (lighthouse roster from [`mackes_mesh_types::lighthouse::roster_from_directory`],
//!   pinged via the same transport probe). Non-critical; a LAN-only mesh with no
//!   lighthouse configured is [`CheckStatus::Skipped`], not a red fail.

use std::path::Path;

use mackes_mesh_types::lighthouse::LighthouseAddr;
use mackes_mesh_types::peers::PeerRecord;
use mde_role::Role;

/// Nebula's on-disk config dir — where the supervisor materializes the signed
/// `host.crt` + `ca.crt`. The same literal the `leave` verb wipes; the overlay /
/// cert probes read (never write) it.
const NEBULA_CONFIG_DIR: &str = "/etc/nebula";

/// One check's outcome. Only a [`Self::Fail`] on a [`Check::critical`] check
/// fails the run; every other status — including [`Self::Skipped`] and the gated
/// [`Self::Unknown`] — is surfaced but never flips the exit code. That is the
/// property that keeps a headless/integration-gated live probe from hard-failing
/// the self-test: a probe that can't run is [`Self::Unknown`], not [`Self::Fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// The check passed.
    Pass,
    /// A non-fatal concern (surfaced, never fails the run).
    Warn,
    /// The check failed (fails the run iff [`Check::critical`]).
    Fail,
    /// The check does not apply to this node — e.g. a LAN-only mesh has no
    /// lighthouse to ping, or no deployment role is pinned. Surfaced, never fails.
    Skipped,
    /// The LIVE probe could not run headless (no overlay, no systemd, no
    /// `nebula-cert`) — a typed "unknown", never a faked pass and never a fail.
    Unknown,
}

impl CheckStatus {
    /// Short tag for the human report line, e.g. `ok` / `warn` / `FAIL` /
    /// `skip` / `gated`.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Pass => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skipped => "skip",
            Self::Unknown => "gated",
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

    // ── OW-10 live-probe facts ──────────────────────────────────────────────
    // Each is gathered behind the impure seam; a value the seam cannot measure
    // headless is a typed `None`/`false` that `assemble` classifies as a
    // non-failing Unknown/Skipped — never a faked reading.
    /// Overlay datapath reachability: `Some(true)` a peer answered over the
    /// Nebula tunnel, `Some(false)` the overlay is configured but no peer
    /// answered, `None` the probe was gated (overlay not up / no peer to reach /
    /// headless). `None` → [`CheckStatus::Unknown`], never a fail.
    pub overlay_reachable: Option<bool>,
    /// The deployment role whose daemons this node should run, or `None` when no
    /// role is pinned (the expected-daemon set is then indeterminate → Skipped).
    pub role: Option<Role>,
    /// How many of the role's systemd units report `is-active`, or `None` when
    /// the live `systemctl` probe was gated (no systemd on this box → Unknown).
    /// The *expected* count is recomputed purely in [`assemble`] from
    /// [`Self::role`] via the reused role→units model, so the seam supplies only
    /// the live actual.
    pub role_daemons_active: Option<usize>,
    /// Whether this node holds a materialized, fingerprintable signed cert
    /// (`host.crt`). Distinct from [`Self::ca_present`] (OW-2, "holds a minted
    /// CA"): this is "THIS node holds a signed identity cert".
    pub cert_present: bool,
    /// Days until this node's cert expires (via [`crate::ca::expiry`]), or `None`
    /// when the expiry probe was gated (`nebula-cert` unavailable / cert
    /// unreadable). Negative means already past `notAfter`.
    pub cert_days_remaining: Option<i64>,
    /// Whether any lighthouse is configured for this mesh (from the peer
    /// directory). `false` on a LAN-only mesh-of-one — nothing to ping → Skipped.
    pub lighthouse_configured: bool,
    /// Whether a configured lighthouse answered over the overlay: `Some(true)`
    /// answered, `Some(false)` configured but none answered, `None` the ping was
    /// gated. Ignored when [`Self::lighthouse_configured`] is `false`.
    pub lighthouse_reachable: Option<bool>,
}

/// The assembled self-diagnostic report — the headless body both front-ends read
/// and the CLI prints (as JSON with `--json`, else as text).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SelfTestReport {
    /// The node this report describes.
    pub node_id: String,
    /// Every check, in report order.
    pub checks: Vec<Check>,
    /// `true` iff no *critical* check failed — the pass/fail verdict. A
    /// [`CheckStatus::Unknown`] (gated live probe) or [`CheckStatus::Skipped`]
    /// check never contributes here, even on a critical check, so a probe that
    /// cannot run headless never hard-fails the run.
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
    let mut checks = Vec::with_capacity(8);

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

    // ── OW-10 · Overlay reachable — critical. The datapath complement to the
    //    mesh-directory check: can this node actually reach a peer over the
    //    tunnel. A genuine "configured but unreachable" is a hard fail; a gated
    //    probe (headless / mesh-of-one) is Unknown and never fails the run.
    let (overlay_status, overlay_detail) = match p.overlay_reachable {
        Some(true) => (
            CheckStatus::Pass,
            "a peer answered over the Nebula overlay".to_string(),
        ),
        Some(false) => (
            CheckStatus::Fail,
            "overlay configured but no peer answered over the tunnel".to_string(),
        ),
        None => (
            CheckStatus::Unknown,
            "not probed (overlay not up / no peer to reach / headless)".to_string(),
        ),
    };
    checks.push(Check {
        id: "overlay",
        label: "Overlay reachable",
        status: overlay_status,
        critical: true,
        detail: overlay_detail,
    });

    // ── OW-10 · Role daemons active — non-critical (a systemd-units surface like
    //    KVM). Pure expected-vs-actual: the *expected* unit count is recomputed
    //    here from the reused role→units model; the seam supplies only the live
    //    active count. No role pinned → Skipped; no systemd → Unknown.
    let (role_status, role_detail) = match (p.role, p.role_daemons_active) {
        (None, _) => (
            CheckStatus::Skipped,
            "no deployment role pinned — expected daemon set indeterminate".to_string(),
        ),
        (Some(_), None) => (
            CheckStatus::Unknown,
            "systemd unavailable — role daemon state not probed".to_string(),
        ),
        (Some(role), Some(active)) => {
            let expected = role_enable_unit_count(role);
            if expected == 0 {
                (
                    CheckStatus::Skipped,
                    format!("{} role runs no gated daemons", role.as_str()),
                )
            } else if active >= expected {
                (
                    CheckStatus::Pass,
                    format!("{active}/{expected} {} daemons active", role.as_str()),
                )
            } else if active == 0 {
                (
                    CheckStatus::Fail,
                    format!(
                        "0/{expected} {} daemons active — none of the role's daemons are running",
                        role.as_str()
                    ),
                )
            } else {
                (
                    CheckStatus::Warn,
                    format!(
                        "{active}/{expected} {} daemons active (degraded)",
                        role.as_str()
                    ),
                )
            }
        }
    };
    checks.push(Check {
        id: "role-daemons",
        label: "Role daemons active",
        status: role_status,
        critical: false,
        detail: role_detail,
    });

    // ── OW-10 · CA-signed cert — non-critical (layered over the critical
    //    identity-key check). "THIS node holds a signed, unexpired cert": present
    //    + fingerprintable, and its `notAfter` is in the future. A present cert
    //    whose expiry can't be read (`nebula-cert` absent) is Unknown, not a fail.
    let (cert_status, cert_detail) = if p.cert_present {
        match p.cert_days_remaining {
            None => (
                CheckStatus::Unknown,
                "cert present; expiry not probed (nebula-cert unavailable)".to_string(),
            ),
            Some(d) if d < 0 => (
                CheckStatus::Fail,
                format!("CA-signed cert EXPIRED {} days ago", d.abs()),
            ),
            Some(d) => (
                CheckStatus::Pass,
                format!("CA-signed cert valid ({d} days remaining)"),
            ),
        }
    } else {
        (
            CheckStatus::Fail,
            format!("no signed cert ({NEBULA_CONFIG_DIR}/host.crt) — node not enrolled"),
        )
    };
    checks.push(Check {
        id: "cert-signed",
        label: "CA-signed identity cert",
        status: cert_status,
        critical: false,
        detail: cert_detail,
    });

    // ── OW-10 · Lighthouse pingable — non-critical. A LAN-only mesh with no
    //    lighthouse configured is Skipped (not a red fail); a configured
    //    lighthouse that answers is Pass, one that doesn't is Fail, and a gated
    //    ping is Unknown.
    let (lh_status, lh_detail) = if p.lighthouse_configured {
        match p.lighthouse_reachable {
            Some(true) => (
                CheckStatus::Pass,
                "a configured lighthouse answered over the overlay".to_string(),
            ),
            Some(false) => (
                CheckStatus::Fail,
                "configured lighthouse did not answer over the overlay".to_string(),
            ),
            None => (
                CheckStatus::Unknown,
                "lighthouse configured; overlay ping not probed (headless)".to_string(),
            ),
        }
    } else {
        (
            CheckStatus::Skipped,
            "no lighthouse configured (LAN-only mesh)".to_string(),
        )
    };
    checks.push(Check {
        id: "lighthouse",
        label: "Lighthouse pingable",
        status: lh_status,
        critical: false,
        detail: lh_detail,
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

/// How many systemd units `role` runs — the *enable* actions in the reused
/// role→units model ([`crate::onboard::role_provision::plan`]). Pure (the plan is
/// deterministic + side-effect-free), so [`assemble`] owns the expected-vs-actual
/// classification off it without touching systemd. This is glue over the existing
/// model, never a second copy of the role→units table.
#[must_use]
fn role_enable_unit_count(role: Role) -> usize {
    role_enable_units(role).count()
}

/// The systemd units `role` runs, from the reused role→units model. Both the pure
/// expected-count ([`role_enable_unit_count`]) and the live probe
/// ([`probe_role_daemons_active`]) iterate this one source, so the expected set
/// the classifier compares against and the set the seam probes never drift.
fn role_enable_units(role: Role) -> impl Iterator<Item = &'static str> {
    use crate::onboard::role_provision::{plan, UnitAction};
    plan(role)
        .into_iter()
        .filter(|u| u.action == UnitAction::Enable)
        .map(|u| u.unit)
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
///
/// OW-10 live probes (each degrades to a typed gated reading, never a fake):
/// * Overlay: [`crate::transport_probe::probe_rtt`] to a non-self peer's overlay
///   IP, but only when the overlay is configured (`host.crt` present) AND there
///   is a peer to reach — otherwise `None` (Unknown, not a false fail).
/// * Role daemons: the role's enable-units (reused role→units model) counted via
///   `systemctl is-active`; `None` when systemd is absent.
/// * Cert: `host.crt` present + fingerprintable ([`crate::ca::blocklist`]) and its
///   days-to-expiry ([`crate::ca::expiry`], `None` when `nebula-cert` can't run).
/// * Lighthouse: the roster from the directory
///   ([`mackes_mesh_types::lighthouse::roster_from_directory`]) pinged over the
///   overlay; not-configured stays `false` (Skipped).
#[must_use]
pub fn gather(node_id: &str, db_path: &Path, workgroup_root: &Path) -> Probes {
    let kvm_total = crate::kvm::KVM_SERVICES.len();
    let kvm_active = crate::kvm::KVM_SERVICES
        .iter()
        .filter(|s| unit_is_active(s.unit))
        .count();

    // One directory read, shared by the mesh-peer, overlay, and lighthouse probes.
    let peers = {
        let dir = mackes_mesh_types::peers::peers_dir(workgroup_root);
        mackes_mesh_types::peers::read_peers(&dir)
    };
    let mesh_peers = peers.len();

    let identity_present = Path::new(crate::node_key::DEFAULT_KEY_PATH).exists();
    let ca_present = ca_rows_present(db_path);

    // ── OW-10 live probes (impure seam; assemble folds the facts purely) ──
    let nebula_dir = Path::new(NEBULA_CONFIG_DIR);
    let overlay_reachable = probe_overlay_reachable(nebula_dir, &peers, node_id);

    let role = mde_role::load_class().ok().map(|c| c.role);
    let role_daemons_active = role.and_then(probe_role_daemons_active);

    let host_cert = nebula_dir.join("host.crt");
    let cert_present = read_signed_cert_fingerprint(&host_cert).is_some();
    let cert_days_remaining = if cert_present {
        crate::ca::expiry::ca_cert_days_remaining(&host_cert, now_unix())
    } else {
        None
    };

    let lighthouses = mackes_mesh_types::lighthouse::roster_from_directory(&peers);
    let lighthouse_configured = !lighthouses.is_empty();
    let lighthouse_reachable = if lighthouse_configured {
        probe_lighthouse_reachable(nebula_dir, &lighthouses)
    } else {
        None
    };

    Probes {
        node_id: node_id.to_string(),
        kvm_active,
        kvm_total,
        mesh_peers,
        identity_present,
        ca_present,
        overlay_reachable,
        role,
        role_daemons_active,
        cert_present,
        cert_days_remaining,
        lighthouse_configured,
        lighthouse_reachable,
    }
}

/// Wall-clock Unix seconds for the cert-expiry probe. Saturates rather than
/// panicking on a pre-epoch clock (never realistic here, but honest).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Overlay-reachability seam. `Some(reachable)` only when the overlay is
/// configured (`host.crt` present) AND a non-self peer overlay IP exists to reach;
/// otherwise `None` (gated — a headless/dev box or a mesh-of-one is Unknown, not a
/// red fail). Reuses [`crate::transport_probe::probe_rtt`], the TCP-through-tunnel
/// probe the latency + lighthouse workers time the path with.
fn probe_overlay_reachable(nebula_dir: &Path, peers: &[PeerRecord], node_id: &str) -> Option<bool> {
    if !nebula_dir.join("host.crt").exists() {
        return None; // overlay not configured on this box → Unknown
    }
    let self_name = node_id.strip_prefix("peer:").unwrap_or(node_id);
    let target = peers.iter().find_map(|p| {
        if p.hostname == self_name {
            return None;
        }
        p.overlay_ip.as_deref().filter(|ip| !ip.is_empty())
    })?; // no non-self peer overlay IP (mesh-of-one) → gated
    Some(crate::transport_probe::probe_rtt(target).reachable)
}

/// Lighthouse-ping seam. `Some(true)` when ANY configured lighthouse answers over
/// the overlay, `Some(false)` when none answered, `None` when gated (overlay not
/// up). Reuses [`crate::transport_probe::probe_rtt`] against the lighthouse overlay
/// IPs [`mackes_mesh_types::lighthouse::roster_from_directory`] already extracts.
fn probe_lighthouse_reachable(nebula_dir: &Path, lighthouses: &[LighthouseAddr]) -> Option<bool> {
    if !nebula_dir.join("host.crt").exists() || lighthouses.is_empty() {
        return None; // overlay not up / nothing to ping → Unknown
    }
    Some(
        lighthouses
            .iter()
            .any(|lh| crate::transport_probe::probe_rtt(&lh.overlay_ip).reachable),
    )
}

/// Role-daemon seam. How many of `role`'s enable-units (reused role→units model)
/// report `is-active`, or `None` when systemd isn't available (the whole probe is
/// gated → Unknown, rather than mis-reporting every unit inactive on a dev box).
fn probe_role_daemons_active(role: Role) -> Option<usize> {
    if !systemctl_available() {
        return None;
    }
    Some(
        role_enable_units(role)
            .filter(|&u| unit_is_active(u))
            .count(),
    )
}

/// Whether `systemctl` can be invoked at all. Gates the role-daemon probe to
/// Unknown on a box with no systemd instead of reporting every unit inactive.
fn systemctl_available() -> bool {
    std::process::Command::new("systemctl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Read `host.crt` and return its fingerprint iff it is a parseable, CA-signed
/// nebula cert — the "this node holds a signed cert" seam. Reuses
/// [`crate::ca::blocklist::fingerprint_cert_pem`], the same primitive the `leave`
/// verb fingerprints `host.crt` with (glue, not a reimplementation).
fn read_signed_cert_fingerprint(host_cert: &Path) -> Option<String> {
    let pem = std::fs::read_to_string(host_cert).ok()?;
    crate::ca::blocklist::fingerprint_cert_pem(&pem)
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

    /// A fully-healthy node: the OW-2 args pin the four original checks; the
    /// OW-10 facts default to a healthy reading so the OW-2 tests keep pinning
    /// only their own checks. The OW-10 tests below start from this and override
    /// the single field under test.
    fn probes(kvm: (usize, usize), peers: usize, id: bool, ca: bool) -> Probes {
        Probes {
            node_id: "node-a".to_string(),
            kvm_active: kvm.0,
            kvm_total: kvm.1,
            mesh_peers: peers,
            identity_present: id,
            ca_present: ca,
            // OW-10 healthy defaults.
            overlay_reachable: Some(true),
            role: Some(Role::Workstation),
            role_daemons_active: Some(role_enable_unit_count(Role::Workstation)),
            cert_present: true,
            cert_days_remaining: Some(365),
            lighthouse_configured: true,
            lighthouse_reachable: Some(true),
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
        // OW-10 checks all green on a healthy node.
        assert_eq!(check(&r, "overlay").status, CheckStatus::Pass);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Pass);
        assert_eq!(check(&r, "cert-signed").status, CheckStatus::Pass);
        assert_eq!(check(&r, "lighthouse").status, CheckStatus::Pass);
        // Report names the node + every check (4 OW-2 + 4 OW-10).
        assert_eq!(r.node_id, "node-a");
        assert_eq!(r.checks.len(), 8);
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
        assert_eq!(CheckStatus::Skipped.tag(), "skip");
        assert_eq!(CheckStatus::Unknown.tag(), "gated");
    }

    // ── OW-10 · Overlay reachable (critical) ──

    #[test]
    fn overlay_reachable_passes() {
        let r = assemble(&probes((6, 6), 3, true, true));
        assert_eq!(check(&r, "overlay").status, CheckStatus::Pass);
        assert!(check(&r, "overlay").critical);
    }

    #[test]
    fn overlay_unreachable_is_a_critical_fail() {
        // A genuinely-probed "configured but no peer answered" is a real Fail on a
        // critical check → non-zero exit.
        let mut p = probes((6, 6), 3, true, true);
        p.overlay_reachable = Some(false);
        let r = assemble(&p);
        assert_eq!(check(&r, "overlay").status, CheckStatus::Fail);
        assert!(!r.ok);
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn overlay_gated_is_unknown_and_never_fails_the_exit() {
        // The exit-code safety property: a gated live probe (headless / mesh-of-
        // one) is Unknown, NOT Fail — so even though `overlay` is a CRITICAL
        // check, a `None` reading leaves the run green.
        let mut p = probes((6, 6), 3, true, true);
        p.overlay_reachable = None;
        let r = assemble(&p);
        assert_eq!(check(&r, "overlay").status, CheckStatus::Unknown);
        assert!(check(&r, "overlay").critical, "overlay is a critical check");
        assert!(r.ok, "an Unknown critical check must NOT fail the run");
        assert_eq!(r.exit_code(), 0);
    }

    // ── OW-10 · Role daemons active (non-critical, reused role→units model) ──

    #[test]
    fn role_daemons_all_active_pass() {
        // Workstation enables its full unit set; all active → Pass.
        let expected = role_enable_unit_count(Role::Workstation);
        let mut p = probes((6, 6), 3, true, true);
        p.role = Some(Role::Workstation);
        p.role_daemons_active = Some(expected);
        let r = assemble(&p);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Pass);
        assert!(!check(&r, "role-daemons").critical);
    }

    #[test]
    fn role_daemons_partial_warns() {
        // Some but not all of the role's daemons up → degraded Warn (not a fail).
        let expected = role_enable_unit_count(Role::Lighthouse);
        assert!(expected >= 2, "lighthouse runs several daemons");
        let mut p = probes((6, 6), 3, true, true);
        p.role = Some(Role::Lighthouse);
        p.role_daemons_active = Some(expected - 1);
        let r = assemble(&p);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Warn);
        assert!(
            r.ok,
            "a degraded (non-critical) role-daemon set stays green"
        );
    }

    #[test]
    fn role_daemons_none_active_is_a_fail_but_non_critical() {
        // Zero of the role's daemons running is a loud Fail, but the check is
        // non-critical (a systemd-units surface like KVM) so the exit stays 0.
        let expected = role_enable_unit_count(Role::Lighthouse);
        let mut p = probes((6, 6), 3, true, true);
        p.role = Some(Role::Lighthouse);
        p.role_daemons_active = Some(0);
        let r = assemble(&p);
        assert!(expected > 0);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Fail);
        assert!(!check(&r, "role-daemons").critical);
        assert!(r.ok, "a non-critical Fail must not flip the exit code");
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn role_daemons_gated_is_unknown_when_systemd_absent() {
        let mut p = probes((6, 6), 3, true, true);
        p.role = Some(Role::Workstation);
        p.role_daemons_active = None; // systemd unavailable
        let r = assemble(&p);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Unknown);
        assert!(r.ok);
    }

    #[test]
    fn role_daemons_skipped_when_no_role_pinned() {
        let mut p = probes((6, 6), 3, true, true);
        p.role = None; // unpinned box — expected set indeterminate
        p.role_daemons_active = None;
        let r = assemble(&p);
        assert_eq!(check(&r, "role-daemons").status, CheckStatus::Skipped);
        assert!(r.ok);
    }

    #[test]
    fn role_daemons_expected_count_comes_from_the_reused_model() {
        // The classifier's "expected" is the role_provision plan's enable-units —
        // proving assemble reuses the role→units model rather than a private copy.
        use crate::onboard::role_provision::{plan, UnitAction};
        for role in [Role::Lighthouse, Role::Workstation] {
            let want = plan(role)
                .iter()
                .filter(|u| u.action == UnitAction::Enable)
                .count();
            assert_eq!(role_enable_unit_count(role), want);
        }
        // Workstation (top rank) enables strictly more than Lighthouse.
        assert!(
            role_enable_unit_count(Role::Workstation) > role_enable_unit_count(Role::Lighthouse)
        );
    }

    // ── OW-10 · CA-signed cert (non-critical) ──

    #[test]
    fn cert_signed_valid_passes() {
        let mut p = probes((6, 6), 3, true, true);
        p.cert_present = true;
        p.cert_days_remaining = Some(200);
        let r = assemble(&p);
        assert_eq!(check(&r, "cert-signed").status, CheckStatus::Pass);
        assert!(check(&r, "cert-signed").detail.contains("200 days"));
    }

    #[test]
    fn cert_signed_expired_fails() {
        let mut p = probes((6, 6), 3, true, true);
        p.cert_present = true;
        p.cert_days_remaining = Some(-3); // 3 days past notAfter
        let r = assemble(&p);
        assert_eq!(check(&r, "cert-signed").status, CheckStatus::Fail);
        assert!(check(&r, "cert-signed").detail.contains("EXPIRED"));
        // Non-critical: an expired cert is a loud Fail but doesn't fail the run.
        assert!(r.ok);
    }

    #[test]
    fn cert_signed_absent_fails() {
        let mut p = probes((6, 6), 3, true, true);
        p.cert_present = false;
        p.cert_days_remaining = None;
        let r = assemble(&p);
        assert_eq!(check(&r, "cert-signed").status, CheckStatus::Fail);
        assert!(check(&r, "cert-signed").detail.contains("not enrolled"));
    }

    #[test]
    fn cert_signed_gated_when_expiry_unprobed() {
        // Cert present but the expiry probe couldn't run (`nebula-cert` absent) →
        // Unknown, never a fake pass.
        let mut p = probes((6, 6), 3, true, true);
        p.cert_present = true;
        p.cert_days_remaining = None;
        let r = assemble(&p);
        assert_eq!(check(&r, "cert-signed").status, CheckStatus::Unknown);
        assert!(r.ok);
    }

    // ── OW-10 · Lighthouse pingable (non-critical) ──

    #[test]
    fn lighthouse_reachable_passes() {
        let mut p = probes((6, 6), 3, true, true);
        p.lighthouse_configured = true;
        p.lighthouse_reachable = Some(true);
        let r = assemble(&p);
        assert_eq!(check(&r, "lighthouse").status, CheckStatus::Pass);
    }

    #[test]
    fn lighthouse_unreachable_fails_non_critically() {
        let mut p = probes((6, 6), 3, true, true);
        p.lighthouse_configured = true;
        p.lighthouse_reachable = Some(false);
        let r = assemble(&p);
        assert_eq!(check(&r, "lighthouse").status, CheckStatus::Fail);
        assert!(!check(&r, "lighthouse").critical);
        assert!(
            r.ok,
            "a configured-but-silent lighthouse is a non-critical Fail"
        );
    }

    #[test]
    fn lighthouse_none_configured_is_skipped_not_failed() {
        // A LAN-only mesh with no lighthouse configured is a non-failure Skipped,
        // NOT a red fail (the explicit no-lighthouse contract).
        let mut p = probes((6, 6), 3, true, true);
        p.lighthouse_configured = false;
        p.lighthouse_reachable = None;
        let r = assemble(&p);
        assert_eq!(check(&r, "lighthouse").status, CheckStatus::Skipped);
        assert!(r.ok);
    }

    #[test]
    fn lighthouse_gated_ping_is_unknown() {
        let mut p = probes((6, 6), 3, true, true);
        p.lighthouse_configured = true;
        p.lighthouse_reachable = None; // ping couldn't run headless
        let r = assemble(&p);
        assert_eq!(check(&r, "lighthouse").status, CheckStatus::Unknown);
        assert!(r.ok);
    }

    #[test]
    fn gated_probes_render_in_json_and_human() {
        // A node whose live probes are all gated still produces a green report
        // with typed gated/skip lines — no fake pass, no fail.
        let mut p = probes((6, 6), 3, true, true);
        p.overlay_reachable = None;
        p.role = Some(Role::Workstation);
        p.role_daemons_active = None;
        p.cert_present = true;
        p.cert_days_remaining = None;
        p.lighthouse_configured = false;
        p.lighthouse_reachable = None;
        let r = assemble(&p);
        assert!(r.ok, "all-gated node is green (exit 0)");
        let human = r.human();
        assert!(human.contains("[gated] Overlay reachable"));
        assert!(human.contains("[gated] Role daemons active"));
        assert!(human.contains("[skip] Lighthouse pingable"));
        let json = serde_json::to_value(&r).expect("serialize");
        // overlay is checks[4] (after the 4 OW-2 checks); its status serializes
        // lowercase.
        assert_eq!(json["checks"][4]["status"], "unknown");
    }
}
