//! ENT-15 (C4) — `meshctl`, the operator facade.
//!
//! One learnable lifecycle tool over the ~50-verb `mackesd` surface. The
//! lifecycle gestures an operator actually performs — install, status,
//! doctor, mesh init, provision/join, test, logs, fleet status, repair,
//! leave/decommission — each map to a real action: most **forward** to
//! the underlying `mackesd` subcommand (so `meshctl` stays a thin facade,
//! never a re-implementation, §6), and the four that have no `mackesd`
//! verb yet (doctor, test, logs, fleet status) are implemented here
//! directly over the host tools + the replicated stores.
//!
//! The command surface is exactly the enterprise-readiness §8 list, so
//! `meshctl --help` is the operate runbook (the ENT-15 acceptance). Each
//! forwarded verb passes its remaining args straight through to
//! `mackesd`, so `meshctl` never has to track every subcommand's flags.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "meshctl",
    about = "MCNF operator facade — the learnable lifecycle tool",
    long_about = "meshctl is the operator's front door to a MCNF node and its \
                  fleet. It wraps the mackesd daemon's lifecycle gestures behind one \
                  learnable command set: install, status, doctor, mesh init, \
                  provision/join, test, logs, fleet status, repair, leave/decommission.",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Pin this node's deployment role and preflight its prerequisites.
    Install {
        /// `lighthouse` | `server` | `workstation`.
        #[arg(long)]
        role: String,
    },
    /// Show this node + fleet status (wraps `mackesd status`).
    Status {
        /// Extra args forwarded verbatim to `mackesd status`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Health checks: required binaries, services, and the overlay link.
    Doctor,
    /// Mesh lifecycle bootstrap (`meshctl mesh init`).
    #[command(subcommand)]
    Mesh(MeshCmd),
    /// Provision/enroll this node into an existing mesh with a join token.
    Provision {
        /// Deployment role to pin after enrolling.
        #[arg(long)]
        role: Option<String>,
        /// Single-use enrollment token from the lighthouse.
        #[arg(long)]
        token: String,
    },
    /// Join an existing mesh with a token (alias of `provision`).
    Join {
        /// Single-use enrollment token from the lighthouse.
        #[arg(long)]
        token: String,
    },
    /// Run a mesh self-test (`meshctl test connectivity|dns|firewall`).
    #[command(subcommand)]
    Test(TestCmd),
    /// Tail the mesh daemon's journal (`journalctl -u mackesd`).
    Logs {
        /// journald `--since` window, e.g. `1h`, `today`.
        #[arg(long)]
        since: Option<String>,
        /// Follow (stream) new log lines.
        #[arg(long, short)]
        follow: bool,
    },
    /// Fleet-wide views (`meshctl fleet status`).
    #[command(subcommand)]
    Fleet(FleetCmd),
    /// Reconcile this node to the elected fleet baseline (heal/repair).
    Repair {
        /// Extra args forwarded to `mackesd reconcile`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Leave the mesh from this node (wraps `mackesd leave`).
    Leave {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Decommission another node by id (wraps `mackesd decommission`).
    Decommission {
        /// Node id to decommission, e.g. `peer:anvil`.
        node_id: String,
        /// Force even if the node is unreachable.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum MeshCmd {
    /// Bootstrap a brand-new mesh as the first lighthouse (ENT-4).
    Init {
        /// Extra args forwarded to `mackesd mesh-init`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum TestCmd {
    /// Overlay reachability across the fleet (ENT-10 / PLANES-19).
    Connectivity,
    /// Mesh DNS resolution (`<host>.mesh`) is wired locally.
    Dns,
    /// The firewalld zone policy is applied (overlay trusted, PLANES-16).
    Firewall,
}

#[derive(Subcommand)]
enum FleetCmd {
    /// List every enrolled node (wraps `mackesd nodes`).
    Status {
        /// Extra args forwarded to `mackesd nodes`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Verb::Install { role } => install(&role),
        Verb::Status { args } => forward("status", &args),
        Verb::Doctor => doctor(),
        Verb::Mesh(MeshCmd::Init { args }) => forward("mesh-init", &args),
        Verb::Provision { role, token } => provision(role.as_deref(), &token),
        Verb::Join { token } => provision(None, &token),
        Verb::Test(t) => test(&t),
        Verb::Logs { since, follow } => logs(since.as_deref(), follow),
        Verb::Fleet(FleetCmd::Status { args }) => forward("nodes", &args),
        Verb::Repair { args } => forward("reconcile", &args),
        Verb::Leave { yes } => {
            let mut a = Vec::new();
            if yes {
                a.push("--yes".to_string());
            }
            forward("leave", &a)
        }
        Verb::Decommission { node_id, force } => {
            let mut a = vec!["--node-id".to_string(), node_id];
            if force {
                a.push("--force".to_string());
            }
            forward("decommission", &a)
        }
    }
}

/// Locate the sibling `mackesd` binary: prefer the one next to this
/// `meshctl` (installed together), else fall back to `mackesd` on `$PATH`.
fn mackesd_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("mackesd");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("mackesd")
}

/// Forward to `mackesd <subcommand> <args...>`, inheriting stdio, and
/// propagate its exit code.
fn forward(subcommand: &str, args: &[String]) -> ExitCode {
    let status = Command::new(mackesd_bin())
        .arg(subcommand)
        .args(args)
        .status();
    match status {
        Ok(s) => ExitCode::from(u8::try_from(s.code().unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!("meshctl: could not run mackesd {subcommand}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn provision(role: Option<&str>, token: &str) -> ExitCode {
    // DAR-19 follow-up — `mackesd enroll --token` is the legacy CSR-publish-
    // and-poll path (NF-3.6.a): it needs the joining box already co-located
    // on QNM-Shared over the overlay, which a genuinely fresh box never is.
    // `mackesd join <token>` is the ONBOARD-2 network path that honors the
    // token's `?fp=` cert-pinned `/enroll` endpoint and is the one that
    // actually reaches a fresh, not-yet-enrolled box. Join first; pin the
    // role afterwards if one was requested (idempotent — `join` already
    // pinned it when unpinned, `mde_role::pin`'s same-rank rewrite is a
    // no-op `Unchanged`).
    let (subcommand, args) = join_forward(token, role);
    let join = forward(subcommand, &args);
    if join != ExitCode::SUCCESS {
        return join;
    }
    if let Some(role) = role {
        return forward("role-pin", &[role.to_string()]);
    }
    ExitCode::SUCCESS
}

/// The `mackesd <subcommand> <args...>` invocation `provision` forwards to —
/// factored out as pure data so the DAR-19 follow-up fix (route through the
/// working `join` network path, never the legacy `enroll --token`
/// CSR-publish-and-poll path) is unit-testable without spawning `mackesd`.
///
/// The token rides POSITIONALLY: `mackesd`'s `Cmd::Join { token: Option<String>,
/// .. }` has no `#[arg(long)]` on `token`, so `join --token <token>` would
/// fail to parse as the token — it must be `join <token>`. `--role` is
/// forwarded only when the caller requested one, so an unpinned box pins
/// straight to the requested role instead of `join`'s own `--role`
/// default (`workstation`) — which matters when the two disagree (e.g. a
/// requested `lighthouse`, a strictly lower rank than the `workstation`
/// default: pinning `workstation` first would make the caller's follow-up
/// `role-pin lighthouse` a refused downgrade).
fn join_forward(token: &str, role: Option<&str>) -> (&'static str, Vec<String>) {
    let mut args = vec![token.to_string()];
    if let Some(role) = role {
        args.push("--role".to_string());
        args.push(role.to_string());
    }
    ("join", args)
}

fn install(role: &str) -> ExitCode {
    println!("meshctl install — pinning role `{role}` and preflighting prerequisites\n");
    let pin = forward("role-pin", &[role.to_string()]);
    if pin != ExitCode::SUCCESS {
        eprintln!("meshctl: role pin failed; fix the role and re-run `meshctl install`.");
        return pin;
    }
    println!("\nRole pinned. Prerequisite check:");
    doctor()
}

/// One doctor check outcome.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
    /// A failed critical check makes `doctor` exit non-zero.
    critical: bool,
}

fn doctor() -> ExitCode {
    let mut checks = Vec::new();

    // Required binaries.
    for (bin, critical) in [
        ("nebula", true),
        ("nebula-cert", true),
        ("ansible-playbook", false),
        ("firewall-cmd", false),
        ("nmstatectl", false),
    ] {
        let found = which(bin);
        checks.push(Check {
            name: leak(format!("binary: {bin}")),
            ok: found,
            detail: if found {
                "on PATH".into()
            } else {
                "not found on PATH".into()
            },
            critical,
        });
    }

    // mackesd service.
    let (active, detail) = service_active("mackesd");
    checks.push(Check {
        name: "service: mackesd",
        ok: active,
        detail,
        critical: true,
    });

    // Overlay link.
    let overlay = overlay_ip();
    checks.push(Check {
        name: "overlay: nebula1",
        ok: overlay.is_some(),
        detail: overlay.clone().map_or_else(
            || "no overlay IP on nebula1".into(),
            |ip| format!("up, {ip}"),
        ),
        critical: true,
    });

    // Report.
    let mut failed_critical = false;
    println!("meshctl doctor — {} checks\n", checks.len());
    for c in &checks {
        let mark = if c.ok {
            "ok  "
        } else if c.critical {
            "FAIL"
        } else {
            "warn"
        };
        println!("  [{mark}] {:<22} {}", c.name, c.detail);
        if !c.ok && c.critical {
            failed_critical = true;
        }
    }
    if failed_critical {
        println!("\nDoctor found critical problems. Start mackesd and bring up the overlay, then re-run.");
        ExitCode::FAILURE
    } else {
        println!("\nNode healthy.");
        ExitCode::SUCCESS
    }
}

fn test(t: &TestCmd) -> ExitCode {
    match t {
        TestCmd::Connectivity => test_connectivity(),
        TestCmd::Dns => test_dns(),
        TestCmd::Firewall => test_firewall(),
    }
}

fn test_connectivity() -> ExitCode {
    // Request a fresh overlay-reachability run (the leader mints it), then
    // report the most recent persisted verdict (PLANES-19).
    let root = mackesd_core::default_qnm_shared_root();
    let vdir = root.join("validation");
    if std::fs::create_dir_all(&vdir).is_ok() {
        let _ = std::fs::write(vdir.join("runnow"), b"meshctl");
        println!("meshctl: requested a fresh connectivity run (the leader will mint it).");
    }
    match latest_verdict(&root) {
        Some((id, passed)) => {
            println!(
                "Most recent run {id}: {}",
                if passed {
                    "PASS — all overlay edges reachable"
                } else {
                    "FAIL — see `meshctl fleet status`"
                }
            );
            if passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        None => {
            println!("No verdict yet — re-run `meshctl test connectivity` shortly.");
            ExitCode::SUCCESS
        }
    }
}

/// The newest run id with a verdict + its pass bool.
fn latest_verdict(root: &std::path::Path) -> Option<(String, bool)> {
    let ids = magic_fleet::validation::list_run_ids(root);
    for id in ids.into_iter().rev() {
        let path = magic_fleet::validation::run_dir(root, &id).join("verdict.json");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(passed) = v.get("passed").and_then(serde_json::Value::as_bool) {
                    return Some((id, passed));
                }
            }
        }
    }
    None
}

fn test_dns() -> ExitCode {
    // The mesh-dns worker writes a managed /etc/hosts block and/or wires
    // the .mesh domain into resolved. Either present = DNS is configured.
    let hosts_ok = std::fs::read_to_string("/etc/hosts")
        .map(|h| h.contains("mde mesh-dns"))
        .unwrap_or(false);
    let resolved_ok = Command::new("resolvectl")
        .arg("domain")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("mesh"))
        .unwrap_or(false);
    if hosts_ok || resolved_ok {
        println!(
            "meshctl: mesh DNS is wired ({}).",
            if hosts_ok {
                "/etc/hosts block"
            } else {
                "resolved domain"
            }
        );
        ExitCode::SUCCESS
    } else {
        println!("meshctl: mesh DNS not yet wired (no managed /etc/hosts block, no .mesh resolved domain).");
        ExitCode::FAILURE
    }
}

fn test_firewall() -> ExitCode {
    // PLANES-16: the overlay interface must sit in the `trusted` zone.
    let out = Command::new("firewall-cmd")
        .arg("--get-zone-of-interface=nebula1")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let zone = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if zone == "trusted" {
                println!("meshctl: nebula1 is in the `trusted` zone (PLANES-16 ok).");
                ExitCode::SUCCESS
            } else {
                println!("meshctl: nebula1 is in `{zone}`, expected `trusted`.");
                ExitCode::FAILURE
            }
        }
        _ => {
            println!("meshctl: could not query firewalld (not installed, or nebula1 down).");
            ExitCode::FAILURE
        }
    }
}

fn logs(since: Option<&str>, follow: bool) -> ExitCode {
    let mut cmd = Command::new("journalctl");
    cmd.args(["-u", "mackesd"]);
    if let Some(s) = since {
        cmd.args(["--since", s]);
    }
    if follow {
        cmd.arg("-f");
    }
    match cmd.status() {
        Ok(s) => ExitCode::from(u8::try_from(s.code().unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!("meshctl: journalctl unavailable: {e}");
            ExitCode::FAILURE
        }
    }
}

// ── small host-probe helpers ───────────────────────────────────────────

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
        .unwrap_or(false)
}

/// `(active, detail)` for a systemd unit via `systemctl is-active`.
fn service_active(unit: &str) -> (bool, String) {
    match Command::new("systemctl").args(["is-active", unit]).output() {
        Ok(o) => {
            let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (
                state == "active",
                if state.is_empty() {
                    "unknown".into()
                } else {
                    state
                },
            )
        }
        Err(_) => (false, "systemctl unavailable".into()),
    }
}

/// This node's overlay IP via `ip -4 addr show nebula1`, if up.
fn overlay_ip() -> Option<String> {
    let out = Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("inet ")
            .and_then(|rest| rest.split('/').next())
            .map(str::to_string)
    })
}

/// Leak a String to `&'static str` for the small fixed set of doctor
/// check names (process-lifetime; the set is bounded).
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_verdict_picks_the_newest_run_with_a_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Seed two runs; only the newer one (v-200) has a verdict.
        for (id, verdict) in [("v-100", None), ("v-200", Some(true))] {
            let dir = magic_fleet::validation::run_dir(root, id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("run.json"), "{}").unwrap();
            if let Some(passed) = verdict {
                std::fs::write(
                    dir.join("verdict.json"),
                    serde_json::json!({ "passed": passed }).to_string(),
                )
                .unwrap();
            }
        }
        assert_eq!(latest_verdict(root), Some(("v-200".to_string(), true)));
    }

    #[test]
    fn latest_verdict_is_none_without_any_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(latest_verdict(tmp.path()), None);
    }

    #[test]
    fn join_forward_never_reproduces_the_legacy_enroll_token_call_dar19_regression() {
        // DAR-19 follow-up live regression: `meshctl provision`/`meshctl join`
        // used to unconditionally forward to `mackesd enroll --token <token>`
        // — the legacy CSR-publish-and-poll path a genuinely fresh box can
        // never complete (it isn't on the overlay yet to reach QNM-Shared).
        // Assert the forwarded invocation never takes that shape again, with
        // or without a requested role.
        for role in [None, Some("server")] {
            let (subcommand, args) = join_forward("mesh:home@1.2.3.4:4243#bearer?fp=abc", role);
            assert_eq!(subcommand, "join", "must forward to `join`, not `enroll`");
            assert!(
                !args.iter().any(|a| a == "--token"),
                "must not carry a --token flag (that's the `enroll` verb's shape): {args:?}"
            );
        }
    }

    #[test]
    fn join_forward_passes_the_token_positionally() {
        // `Cmd::Join`'s `token` field in mackesd.rs is a bare positional (no
        // `#[arg(long)]`) — `mackesd join --token <token>` would fail to
        // parse. The token must be argv[0] of the forwarded args.
        let (subcommand, args) = join_forward("mesh:home@1.2.3.4:4243#bearer?fp=abc", None);
        assert_eq!(subcommand, "join");
        assert_eq!(
            args,
            vec!["mesh:home@1.2.3.4:4243#bearer?fp=abc".to_string()]
        );
    }

    #[test]
    fn join_forward_forwards_the_requested_role_as_a_flag() {
        let (_, args) = join_forward("tok", Some("lighthouse"));
        assert_eq!(
            args,
            vec![
                "tok".to_string(),
                "--role".to_string(),
                "lighthouse".to_string()
            ]
        );
    }

    #[test]
    fn join_forward_omits_role_flag_when_none_requested() {
        // `meshctl join --token <token>` (no role) must not force a
        // `--role` onto the forwarded `join` call — `join`'s own
        // "workstation" default applies, matching a direct `mackesd join
        // <token>` invocation.
        let (_, args) = join_forward("tok", None);
        assert!(!args.iter().any(|a| a == "--role"), "unexpected: {args:?}");
    }
}
