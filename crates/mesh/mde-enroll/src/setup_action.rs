//! SETUP-2/3/5 — the `magic-setup` action layer: pure argv builders for the
//! lifecycle verbs the wizard shells, plus a streaming runner.
//!
//! Builders are pure (no I/O) so the exact command the wizard runs is
//! unit-tested. The verbs themselves (`mackesd found`/`join`/`peers`) already
//! do the heavy lifting — incl. provisioning the substrate (etcd + Syncthing,
//! via setup-etcd/setup-syncthing) at enroll —
//! so the wizard is a thin, narrated UX layer over them (design lock 3:
//! imperative verbs bootstrap, Ansible converges).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// A deployment role the wizard can found/join as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRole {
    /// Public lighthouse (LH1 founds; LH2/3 join as lighthouse).
    Lighthouse,
    /// Full workstation behind NAT. A headless box is a Workstation without a
    /// local display.
    Workstation,
}

impl SetupRole {
    /// The role string the `mackesd` verbs accept.
    #[must_use]
    pub fn as_arg(self) -> &'static str {
        match self {
            SetupRole::Lighthouse => "lighthouse",
            SetupRole::Workstation => "workstation",
        }
    }
}

/// `mackesd found <mesh-id> --external-addr <addr> --role <role>` — found a new
/// mesh. `external_addr` is `auto` (detect) or an explicit public IP.
/// `found` provisions the substrate (etcd + Syncthing) via setup-etcd/setup-syncthing.
#[must_use]
pub fn found_argv(mesh_id: &str, external_addr: &str, role: SetupRole) -> Vec<String> {
    vec![
        "mackesd".to_owned(),
        "found".to_owned(),
        mesh_id.to_owned(),
        "--external-addr".to_owned(),
        external_addr.to_owned(),
        "--role".to_owned(),
        role.as_arg().to_owned(),
    ]
}

/// `mackesd join <token> --role <role>` — join an existing mesh (fp-pinned
/// network enroll + role-aware QNM-Shared via BIRTHRIGHT-1).
#[must_use]
pub fn join_argv(token: &str, role: SetupRole) -> Vec<String> {
    vec![
        "mackesd".to_owned(),
        "join".to_owned(),
        token.to_owned(),
        "--role".to_owned(),
        role.as_arg().to_owned(),
    ]
}

/// `mackesd peers` — list the enrolled directory (SETUP-5 Manage screen).
#[must_use]
pub fn peers_argv() -> Vec<String> {
    vec!["mackesd".to_owned(), "peers".to_owned()]
}

/// `mackesd add-peer --role <role>` — mint a single-use v3 join token for a new
/// peer (SETUP-5) or a second/third lighthouse (SETUP-4, `--role lighthouse`).
/// mesh-id + the endpoint fingerprint are sourced by the verb itself.
#[must_use]
pub fn add_peer_argv(role: SetupRole) -> Vec<String> {
    vec![
        "mackesd".to_owned(),
        "add-peer".to_owned(),
        "--role".to_owned(),
        role.as_arg().to_owned(),
    ]
}

/// `mackesd remove-peer <node-id>` — decommission + revoke + ban a peer (SETUP-5).
#[must_use]
pub fn remove_peer_argv(node_id: &str) -> Vec<String> {
    vec![
        "mackesd".to_owned(),
        "remove-peer".to_owned(),
        node_id.to_owned(),
    ]
}

/// `systemctl is-active <unit>` — used by the Status screen to report each
/// service's state without a privileged call.
#[must_use]
pub fn is_active_argv(unit: &str) -> Vec<String> {
    vec![
        "systemctl".to_owned(),
        "is-active".to_owned(),
        unit.to_owned(),
    ]
}

/// `mackesd onboard self-test` — the node self-diagnostic (design §47: overlay
/// reachable + role daemons active + CA-signed + lighthouse pingable). The
/// wizard runs this after a successful Create/Join and narrates the human report
/// into the log so the operator sees a per-item green/red verdict.
#[must_use]
pub fn self_test_argv() -> Vec<String> {
    vec![
        "mackesd".to_owned(),
        "onboard".to_owned(),
        "self-test".to_owned(),
    ]
}

/// The service set the wizard reports/guarantees (design §"Service set").
pub const WIZARD_SERVICES: [&str; 5] = [
    "nebula.service",
    "mackesd.service",
    "mesh-health.timer",
    "etcd.service",
    "syncthing.service",
];

/// Run `argv` and stream each stdout/stderr line to `on_line`, returning the
/// process success flag. The wizard pumps `on_line` into its live-log pane so
/// every step is narrated in real time. An un-spawnable program (verb not on
/// PATH) reports the error through `on_line` and returns `false`.
pub fn run_streaming<F: FnMut(String)>(argv: &[String], mut on_line: F) -> bool {
    let Some((prog, args)) = argv.split_first() else {
        on_line("(empty command)".to_string());
        return false;
    };
    let child = Command::new(prog)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            on_line(format!("cannot run {prog}: {e}"));
            return false;
        }
    };
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            on_line(line);
        }
    }
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            on_line(line);
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_argv_shape() {
        assert_eq!(
            found_argv("home", "auto", SetupRole::Lighthouse),
            vec![
                "mackesd",
                "found",
                "home",
                "--external-addr",
                "auto",
                "--role",
                "lighthouse"
            ]
        );
    }

    #[test]
    fn join_argv_carries_role_flag() {
        let argv = join_argv("mesh:home@1.2.3.4:4243#b?fp=x", SetupRole::Workstation);
        assert_eq!(argv[0], "mackesd");
        assert_eq!(argv[1], "join");
        assert_eq!(argv[3], "--role");
        assert_eq!(argv[4], "workstation");
    }

    #[test]
    fn peers_shape() {
        assert_eq!(peers_argv(), vec!["mackesd", "peers"]);
    }

    #[test]
    fn add_and_remove_peer_shapes() {
        assert_eq!(
            add_peer_argv(SetupRole::Lighthouse),
            vec!["mackesd", "add-peer", "--role", "lighthouse"]
        );
        assert_eq!(
            remove_peer_argv("peer:anvil"),
            vec!["mackesd", "remove-peer", "peer:anvil"]
        );
    }

    #[test]
    fn is_active_targets_the_unit() {
        assert_eq!(
            is_active_argv("syncthing.service"),
            vec!["systemctl", "is-active", "syncthing.service"]
        );
    }

    #[test]
    fn self_test_shape() {
        assert_eq!(self_test_argv(), vec!["mackesd", "onboard", "self-test"]);
    }

    #[test]
    fn role_args_match_mackesd_vocabulary() {
        assert_eq!(SetupRole::Lighthouse.as_arg(), "lighthouse");
        assert_eq!(SetupRole::Workstation.as_arg(), "workstation");
    }

    #[test]
    fn run_streaming_reports_unspawnable_program() {
        let mut lines = Vec::new();
        let ok = run_streaming(&["definitely-not-a-real-binary-xyzzy".to_owned()], |l| {
            lines.push(l)
        });
        assert!(!ok);
        assert!(lines.iter().any(|l| l.contains("cannot run")));
    }

    #[test]
    fn run_streaming_captures_output_and_success() {
        let mut lines = Vec::new();
        let ok = run_streaming(
            &[
                "sh".to_owned(),
                "-c".to_owned(),
                "echo hello; echo err 1>&2".to_owned(),
            ],
            |l| lines.push(l),
        );
        assert!(ok);
        assert!(lines.iter().any(|l| l == "hello"));
        assert!(lines.iter().any(|l| l == "err"));
    }
}
