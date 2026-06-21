//! CONNECT-3 — managed firewalld **public-deny baseline** enforcement worker.
//!
//! The CONNECT public boundary is **default-deny** (AI_GOVERNANCE §1/§6, the
//! 3-tier posture): on every node the firewalld `public` (underlay) zone must
//! drop all inbound except the **foundational always-public layer** — Nebula
//! UDP/4242, SSH/22, and the enroll listener TCP/4243 (+ the covert TCP/443
//! fallback on lighthouses). Those are the *only* ports that ever face the
//! internet without an explicit exposure policy.
//!
//! This worker **asserts that baseline and corrects drift**. The sibling workers
//! cover adjacent concerns but none of them *enforce default-deny + re-assert the
//! foundational allows on drift*:
//!
//! * [`super::firewall_preset`] — *opens* the Nebula bootstrap ports (4242/udp
//!   everywhere, 443/tcp on lighthouses) once on first tick / role-flip, and
//!   binds the overlay to `trusted`. It never opens SSH or enroll (those rely on
//!   firewalld's stock `public` zone), never asserts the zone *target*, and never
//!   re-asserts a foundational allow that drifted out between role flips.
//! * [`super::connect_firewall`] — layers the *exposure-policy-driven* ingress
//!   openings (additive, bounded-removal) on top.
//! * [`super::firewall_monitor`] — reports *denied* traffic; it doesn't reconcile.
//!
//! ## What this worker does, each tick
//!
//! 1. Read the live `public` zone — its **target** (`--get-target`), allowed
//!    **services** (`--list-services`), and allowed **ports** (`--list-ports`).
//! 2. Compute the **drift** (pure [`compute_drift`]): is the target blanket-allow
//!    (`ACCEPT`)? are any foundational allows missing for this role?
//! 3. If drift is detected, **re-assert additively** (pure [`reassert_commands`]):
//!    `--set-target=default` (only ever tightens — never sets ACCEPT) +
//!    `--add-service`/`--add-port` for the missing foundational allows + a single
//!    `--reload`. It **NEVER removes** a foundational rule and **NEVER widens** to
//!    a blanket allow — corrections are strictly additive / tightening.
//! 4. Fire one Bus alert per drift event (`event/firewall-drift/<host>`).
//!
//! Idempotent (`firewall-cmd`'s ALREADY_ENABLED/ZONE_ALREADY_SET are no-ops) and
//! graceful: a node without `firewall-cmd` logs one warning and idles forever
//! (every tick short-circuits) — no panic. Pure rule/command construction is
//! unit-tested; the shell-out is the thin tail.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// The firewalld zone the internet-facing underlay NIC lives in — the same
/// tight `public` zone [`super::firewall_preset`] binds the underlay to (W70).
pub const PUBLIC_ZONE: &str = super::firewall_preset::UNDERLAY_ZONE;

/// firewalld zone target that means "deny unmatched inbound" (the zone's own
/// default reject/drop). The opposite — and the thing we never set and always
/// correct away from — is `ACCEPT` (blanket-allow).
pub const TARGET_DEFAULT: &str = "default";
/// The blanket-allow target the baseline must NEVER carry. Detected as drift.
pub const TARGET_ACCEPT: &str = "ACCEPT";

/// Tick cadence. Drift is rare (an operator/ansible mistake, a package reset);
/// a 60 s sweep re-closes the boundary fast without a polling storm.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// A foundational always-public allow expressed as a firewalld rule. Exactly one
/// of `service`/`port` is set. These are the ONLY inbound the public zone permits
/// without an explicit exposure policy (the additive [`super::connect_firewall`]
/// openings are separate + reclaimable; these are never removed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundationalAllow {
    /// A named firewalld service (`--add-service <name>` / `--list-services`).
    Service(&'static str),
    /// A raw `(port, proto)` (`--add-port <port>/<proto>` / `--list-ports`).
    Port(u16, &'static str),
}

impl FoundationalAllow {
    /// The `--list-services` / `--list-ports` token this allow appears as when
    /// present in the live zone (so drift detection can match it).
    #[must_use]
    pub fn live_token(&self) -> String {
        match self {
            Self::Service(s) => (*s).to_string(),
            Self::Port(port, proto) => format!("{port}/{proto}"),
        }
    }

    /// The `firewall-cmd --permanent --zone <z> …` flag pair (`["--add-service",
    /// "ssh"]` / `["--add-port", "4242/udp"]`) that asserts this allow. Always an
    /// **add** — never a remove (additive-only invariant).
    #[must_use]
    pub fn add_args(&self) -> Vec<String> {
        match self {
            Self::Service(s) => vec!["--add-service".to_string(), (*s).to_string()],
            Self::Port(port, proto) => {
                vec!["--add-port".to_string(), format!("{port}/{proto}")]
            }
        }
    }
}

/// The foundational always-public baseline for a node's role. Mirrors
/// [`mackes_mesh_types::route_trace::ControlPoint::public_baseline`] (the
/// canonical CONNECT §3 lock) — SSH/22, Nebula UDP/4242, enroll TCP/4243, plus
/// the covert TCP/443 fallback on lighthouses. SSH is expressed as the firewalld
/// `ssh` *service* (its stock public-zone form) so we don't fight the distro's
/// own `ssh` service entry; the rest are raw ports. Pure.
#[must_use]
pub fn foundational_allows(is_lighthouse: bool) -> Vec<FoundationalAllow> {
    let mut out = vec![
        FoundationalAllow::Service("ssh"), // SSH/22 — never lock ourselves out
        FoundationalAllow::Port(4242, "udp"), // Nebula outer tunnel
        FoundationalAllow::Port(4243, "tcp"), // enroll listener (ONBOARD-2)
    ];
    if is_lighthouse {
        // Covert TCP/443 fallback rendezvous (NF-1) — lighthouses only.
        out.push(FoundationalAllow::Port(443, "tcp"));
    }
    out
}

/// The live `public`-zone state this worker reconciles against: the zone target
/// and the set of currently-allowed service/port tokens. Built from
/// `firewall-cmd` output (or, in tests, by hand).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveZone {
    /// The zone target string from `--get-target` (e.g. `default`, `ACCEPT`).
    pub target: String,
    /// Allowed service names (`--list-services`, whitespace-split).
    pub services: Vec<String>,
    /// Allowed port tokens like `4242/udp` (`--list-ports`, whitespace-split).
    pub ports: Vec<String>,
}

impl LiveZone {
    /// Whether a foundational allow is already present in the live zone.
    #[must_use]
    fn contains(&self, allow: &FoundationalAllow) -> bool {
        match allow {
            FoundationalAllow::Service(s) => self.services.iter().any(|x| x == s),
            FoundationalAllow::Port(..) => self.ports.iter().any(|x| *x == allow.live_token()),
        }
    }
}

/// The drift verdict for one tick: is the boundary still default-deny with every
/// foundational allow present? Pure + testable; the worker acts on this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drift {
    /// True when the live target is a blanket-allow (`ACCEPT`) — the boundary is
    /// wide open and MUST be tightened back to `default`.
    pub target_blanket_allow: bool,
    /// Foundational allows the live zone is missing (need re-asserting).
    pub missing: Vec<FoundationalAllow>,
}

impl Drift {
    /// Whether any drift was detected (the worker re-asserts + alerts iff so).
    #[must_use]
    pub fn detected(&self) -> bool {
        self.target_blanket_allow || !self.missing.is_empty()
    }

    /// A short human description for the alert/log (e.g. `target=ACCEPT
    /// missing=[ssh,4243/tcp]`). Empty when no drift.
    #[must_use]
    pub fn describe(&self) -> String {
        if !self.detected() {
            return String::new();
        }
        let mut parts = Vec::new();
        if self.target_blanket_allow {
            parts.push("target=ACCEPT(blanket-allow)".to_string());
        }
        if !self.missing.is_empty() {
            let toks: Vec<String> = self
                .missing
                .iter()
                .map(FoundationalAllow::live_token)
                .collect();
            parts.push(format!("missing=[{}]", toks.join(",")));
        }
        parts.join(" ")
    }
}

/// Pure drift computation: compare the live zone against the role's foundational
/// baseline. A target that isn't a clean `default`-style deny but is the explicit
/// blanket `ACCEPT` is flagged; any foundational allow not present is collected.
///
/// Note we only flag the *blanket-allow* (`ACCEPT`) target as drift — a target
/// of `default`/`%%REJECT%%`/`DROP` all deny unmatched traffic and are fine. We
/// never treat a *stricter* target as drift (we only ever tighten, never loosen).
#[must_use]
pub fn compute_drift(live: &LiveZone, baseline: &[FoundationalAllow]) -> Drift {
    let target_blanket_allow = live.target.eq_ignore_ascii_case(TARGET_ACCEPT);
    let missing = baseline
        .iter()
        .filter(|a| !live.contains(a))
        .copied()
        .collect();
    Drift {
        target_blanket_allow,
        missing,
    }
}

/// Pure command construction: the idempotent `firewall-cmd` argument batches that
/// correct `drift` (each inner vec is one invocation, **without** the binary; the
/// `--permanent --zone <PUBLIC_ZONE>` prefix is prepended by the caller). Returns
/// empty when there's no drift. The result is **additive / tightening only**:
///
/// * a blanket-allow target → `--set-target=default` (tighten — never ACCEPT),
/// * each missing foundational allow → its `--add-service`/`--add-port`.
///
/// There is **no remove path** — by construction this can never close SSH /
/// Nebula / enroll or blanket-open the zone.
#[must_use]
pub fn reassert_commands(drift: &Drift) -> Vec<Vec<String>> {
    let mut batches = Vec::new();
    if drift.target_blanket_allow {
        batches.push(vec![format!("--set-target={TARGET_DEFAULT}")]);
    }
    for allow in &drift.missing {
        batches.push(allow.add_args());
    }
    batches
}

/// The CONNECT-3 public-deny baseline enforcement worker. Reconciles the live
/// `public` zone back to default-deny + the role's foundational allows on a tick,
/// alerting on each drift event. Holds only config — the live state is read fresh
/// each tick so an externally-applied drift is always seen.
pub struct PublicDenyWorker {
    role_marker_path: PathBuf,
    hostname: String,
    tick_interval: Duration,
    /// `firewall-cmd` binary (empty disables the shell-out — for tests).
    firewall_cmd: &'static str,
    /// `mde-bus` binary for the drift alert (empty disables — for tests).
    bus_cmd: &'static str,
    /// Set once we've logged the "no firewall-cmd" warning, so we don't spam it.
    warned_no_firewalld: bool,
}

impl PublicDenyWorker {
    /// Build the worker for `hostname`, reading the lighthouse role marker from
    /// the default path.
    #[must_use]
    pub fn new(hostname: String) -> Self {
        Self {
            role_marker_path: PathBuf::from(super::firewall_preset::DEFAULT_ROLE_MARKER_PATH),
            hostname,
            tick_interval: DEFAULT_TICK_INTERVAL,
            firewall_cmd: "firewall-cmd",
            bus_cmd: "mde-bus",
            warned_no_firewalld: false,
        }
    }

    /// Override the role-marker path (tests).
    #[must_use]
    pub fn with_role_marker_path(mut self, path: PathBuf) -> Self {
        self.role_marker_path = path;
        self
    }

    /// Override the tick cadence (tests).
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Disable the `firewall-cmd` shell-out (tests drive the pure plan).
    #[must_use]
    pub fn without_firewall_cmd(mut self) -> Self {
        self.firewall_cmd = "";
        self
    }

    /// Disable the `mde-bus` alert shell-out (tests).
    #[must_use]
    pub fn without_bus(mut self) -> Self {
        self.bus_cmd = "";
        self
    }

    /// Whether this node currently holds the lighthouse role marker.
    fn is_lighthouse(&self) -> bool {
        self.role_marker_path.exists()
    }

    /// Read one `firewall-cmd --zone <PUBLIC_ZONE> <flag>` value (trimmed
    /// stdout), bounded. `None` on spawn/timeout/non-zero exit.
    fn fw_read(&self, flag: &str) -> Option<String> {
        let mut cmd = std::process::Command::new(self.firewall_cmd);
        cmd.args(["--zone", PUBLIC_ZONE, flag]);
        let out = crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )
        .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Read the live `public` zone (target + services + ports). `None` when any
    /// read fails (we then skip the tick rather than act on a partial view).
    fn read_live_zone(&self) -> Option<LiveZone> {
        let target = self.fw_read("--get-target")?;
        let services = self.fw_read("--list-services")?;
        let ports = self.fw_read("--list-ports")?;
        Some(LiveZone {
            target,
            services: services.split_whitespace().map(str::to_string).collect(),
            ports: ports.split_whitespace().map(str::to_string).collect(),
        })
    }

    /// Run one `firewall-cmd --permanent --zone <PUBLIC_ZONE> <args…>`, bounded,
    /// tolerating firewalld's "already in this state" non-zero exits. Returns
    /// success.
    fn fw_apply(&self, args: &[String]) -> bool {
        let mut cmd = std::process::Command::new(self.firewall_cmd);
        cmd.arg("--permanent")
            .args(["--zone", PUBLIC_ZONE])
            .args(args);
        match crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        ) {
            Ok(out) if out.status.success() => true,
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                stderr.contains("ALREADY_ENABLED") || stderr.contains("ZONE_ALREADY_SET")
            }
            Err(_) => false,
        }
    }

    /// `firewall-cmd --reload` (bounded; best-effort). Activates the permanent
    /// changes applied this tick.
    fn fw_reload(&self) {
        let mut reload = std::process::Command::new(self.firewall_cmd);
        reload.arg("--reload");
        let _ = crate::workers::proc::status_with_timeout(
            reload,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        );
    }

    /// Fire one drift alert on the Bus (best-effort; no-op when `bus_cmd` empty).
    fn alert_drift(&self, drift: &Drift) {
        if self.bus_cmd.is_empty() {
            return;
        }
        let topic = format!("event/firewall-drift/{}", self.hostname);
        // Compact JSON; the description is already alert-safe (no quotes/braces).
        let body = format!(
            r#"{{"host":"{}","drift":"{}","alert":true}}"#,
            self.hostname,
            drift.describe()
        );
        let mut cmd = std::process::Command::new(self.bus_cmd);
        cmd.args(["publish", &topic, "--body-flag", &body]);
        crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
    }

    /// One enforcement tick: read the live `public` zone, compute drift against
    /// the role's foundational baseline, and (iff drift) re-assert additively +
    /// alert. Returns the [`Drift`] observed (empty when none / firewalld absent),
    /// so the worker loop + tests can observe the outcome. Never panics.
    pub fn tick_once(&mut self) -> Drift {
        // Graceful degrade: no firewall-cmd (or test-disabled) → idle, warn once.
        if which(self.firewall_cmd).is_none() {
            if !self.warned_no_firewalld && !self.firewall_cmd.is_empty() {
                tracing::warn!(
                    target: "mackesd::public_deny",
                    cmd = self.firewall_cmd,
                    "firewall-cmd not on PATH; public-deny baseline enforcement idle until \
                     firewalld is installed"
                );
                self.warned_no_firewalld = true;
            }
            return Drift::default();
        }

        let Some(live) = self.read_live_zone() else {
            // A partial/failed read — don't act on an incomplete view; retry next tick.
            tracing::debug!(
                target: "mackesd::public_deny",
                "could not read the live public zone this tick; skipping (retry next tick)"
            );
            return Drift::default();
        };

        let baseline = foundational_allows(self.is_lighthouse());
        let drift = compute_drift(&live, &baseline);
        if !drift.detected() {
            return drift;
        }

        // Drift detected → re-assert additively (tightening only) + alert.
        tracing::warn!(
            target: "mackesd::public_deny",
            drift = %drift.describe(),
            "public-deny baseline drift detected — re-asserting default-deny + foundational allows"
        );
        let mut applied = false;
        for batch in reassert_commands(&drift) {
            if self.fw_apply(&batch) {
                applied = true;
            }
        }
        if applied {
            self.fw_reload();
            tracing::info!(
                target: "mackesd::public_deny",
                "re-asserted the public-deny baseline (default-deny + Nebula/SSH/enroll)"
            );
        }
        self.alert_drift(&drift);
        drift
    }
}

#[async_trait::async_trait]
impl Worker for PublicDenyWorker {
    fn name(&self) -> &'static str {
        "public_deny"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let _ = self.tick_once();
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

/// Minimal `which`-style lookup over `$PATH`. Mirrors the helper in
/// [`super::firewall_preset`] (kept local to avoid a cross-module pub surface for
/// a one-liner). Empty string → `None` (the test/disabled sentinel).
fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    if Path::new(cmd).is_absolute() {
        return Path::new(cmd).is_file().then(|| PathBuf::from(cmd));
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_public_deny() {
        let w = PublicDenyWorker::new("node-a".into());
        assert_eq!(w.name(), "public_deny");
    }

    #[test]
    fn foundational_allows_non_lighthouse_is_ssh_nebula_enroll() {
        // The §1/§6 always-public layer: SSH service + Nebula/4242 + enroll/4243.
        let allows = foundational_allows(false);
        assert_eq!(
            allows,
            vec![
                FoundationalAllow::Service("ssh"),
                FoundationalAllow::Port(4242, "udp"),
                FoundationalAllow::Port(4243, "tcp"),
            ]
        );
    }

    #[test]
    fn foundational_allows_lighthouse_adds_covert_443() {
        let allows = foundational_allows(true);
        assert!(allows.contains(&FoundationalAllow::Port(443, "tcp")));
        // …and still carries the universal three.
        assert!(allows.contains(&FoundationalAllow::Service("ssh")));
        assert!(allows.contains(&FoundationalAllow::Port(4242, "udp")));
        assert!(allows.contains(&FoundationalAllow::Port(4243, "tcp")));
    }

    #[test]
    fn allow_tokens_and_add_args_render_correctly() {
        assert_eq!(FoundationalAllow::Service("ssh").live_token(), "ssh");
        assert_eq!(
            FoundationalAllow::Port(4242, "udp").live_token(),
            "4242/udp"
        );
        assert_eq!(
            FoundationalAllow::Service("ssh").add_args(),
            vec!["--add-service", "ssh"]
        );
        assert_eq!(
            FoundationalAllow::Port(4243, "tcp").add_args(),
            vec!["--add-port", "4243/tcp"]
        );
    }

    #[test]
    fn no_drift_when_baseline_present_and_target_default() {
        // The healthy steady state: target=default + every foundational allow.
        let live = LiveZone {
            target: "default".into(),
            services: vec!["ssh".into()],
            ports: vec!["4242/udp".into(), "4243/tcp".into()],
        };
        let drift = compute_drift(&live, &foundational_allows(false));
        assert!(!drift.detected(), "{}", drift.describe());
        assert!(reassert_commands(&drift).is_empty());
    }

    #[test]
    fn blanket_allow_target_is_drift_and_tightens_only() {
        // Someone flipped the zone to ACCEPT (blanket allow) — the worst case.
        let live = LiveZone {
            target: "ACCEPT".into(),
            services: vec!["ssh".into()],
            ports: vec!["4242/udp".into(), "4243/tcp".into()],
        };
        let drift = compute_drift(&live, &foundational_allows(false));
        assert!(drift.detected());
        assert!(drift.target_blanket_allow);
        // The ONLY target command we ever emit is --set-target=default (never ACCEPT).
        let cmds = reassert_commands(&drift);
        assert_eq!(cmds, vec![vec!["--set-target=default".to_string()]]);
        // No remove/blanket-open command anywhere in the plan.
        for batch in &cmds {
            for arg in batch {
                assert!(!arg.contains("remove"), "additive-only: {arg}");
                assert!(!arg.contains("ACCEPT"), "never blanket-allow: {arg}");
            }
        }
    }

    #[test]
    fn missing_foundational_allows_are_re_added() {
        // SSH + enroll got removed (a bad ansible run); Nebula survived.
        let live = LiveZone {
            target: "default".into(),
            services: vec![],
            ports: vec!["4242/udp".into()],
        };
        let drift = compute_drift(&live, &foundational_allows(false));
        assert!(drift.detected());
        assert!(!drift.target_blanket_allow, "target was fine");
        assert_eq!(
            drift.missing,
            vec![
                FoundationalAllow::Service("ssh"),
                FoundationalAllow::Port(4243, "tcp"),
            ]
        );
        // The re-assert is exactly the two missing adds — nothing removed.
        assert_eq!(
            reassert_commands(&drift),
            vec![
                vec!["--add-service".to_string(), "ssh".to_string()],
                vec!["--add-port".to_string(), "4243/tcp".to_string()],
            ]
        );
    }

    #[test]
    fn worst_case_target_and_all_missing_tightens_and_re_adds_all() {
        // Zone wide open AND stripped — re-assert default-deny then every allow.
        let live = LiveZone {
            target: "ACCEPT".into(),
            services: vec![],
            ports: vec![],
        };
        let drift = compute_drift(&live, &foundational_allows(true)); // lighthouse
        let cmds = reassert_commands(&drift);
        // First the target tighten, then ssh + 4242/udp + 4243/tcp + 443/tcp.
        assert_eq!(cmds[0], vec!["--set-target=default".to_string()]);
        assert_eq!(cmds.len(), 5, "target + 4 foundational adds: {cmds:?}");
        // Absolutely no removal / blanket-allow anywhere.
        for batch in &cmds {
            for arg in batch {
                assert!(!arg.contains("remove"));
                assert!(!arg.to_ascii_uppercase().contains("ACCEPT"));
            }
        }
    }

    #[test]
    fn stricter_targets_are_not_treated_as_drift() {
        // %%REJECT%% / DROP both deny unmatched — we only flag the blanket ACCEPT,
        // never a stricter-than-default target (we tighten, never loosen).
        for t in ["default", "%%REJECT%%", "DROP", "Default"] {
            let live = LiveZone {
                target: t.into(),
                services: vec!["ssh".into()],
                ports: vec!["4242/udp".into(), "4243/tcp".into()],
            };
            let drift = compute_drift(&live, &foundational_allows(false));
            assert!(!drift.target_blanket_allow, "target {t} must not be drift");
        }
    }

    #[test]
    fn accept_target_match_is_case_insensitive() {
        let live = LiveZone {
            target: "accept".into(),
            services: vec!["ssh".into()],
            ports: vec!["4242/udp".into(), "4243/tcp".into()],
        };
        let drift = compute_drift(&live, &foundational_allows(false));
        assert!(
            drift.target_blanket_allow,
            "lowercase accept is still blanket"
        );
    }

    #[test]
    fn describe_is_empty_without_drift_and_informative_with() {
        assert!(Drift::default().describe().is_empty());
        let live = LiveZone {
            target: "ACCEPT".into(),
            services: vec![],
            ports: vec!["4242/udp".into()],
        };
        let d = compute_drift(&live, &foundational_allows(false));
        let desc = d.describe();
        assert!(desc.contains("ACCEPT"), "{desc}");
        assert!(desc.contains("ssh"), "{desc}");
        assert!(desc.contains("4243/tcp"), "{desc}");
    }

    #[test]
    fn tick_is_safe_noop_without_firewalld() {
        // Graceful degrade: empty firewall_cmd → which() None → idle, no panic,
        // no drift acted on. Warning is suppressed for the empty (test) sentinel.
        let mut w = PublicDenyWorker::new("node-a".into())
            .without_firewall_cmd()
            .without_bus();
        assert_eq!(w.tick_once(), Drift::default());
        // Idempotent across ticks.
        assert_eq!(w.tick_once(), Drift::default());
    }

    #[test]
    fn lighthouse_role_is_read_from_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("role.host");
        let w = PublicDenyWorker::new("lh".into()).with_role_marker_path(marker.clone());
        assert!(!w.is_lighthouse(), "no marker → not a lighthouse");
        std::fs::write(&marker, "lighthouse").unwrap();
        assert!(w.is_lighthouse(), "marker present → lighthouse");
    }

    #[test]
    fn which_handles_empty_and_missing() {
        assert!(which("").is_none());
        assert!(which("definitely-not-a-real-binary-xyz-123").is_none());
    }
}
