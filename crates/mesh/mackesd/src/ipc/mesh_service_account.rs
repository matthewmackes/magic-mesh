//! Joined-mesh service-account lifecycle.
//!
//! Every enrolled node exposes the same non-human SSH identity, `MDE-MESH`.
//! The account is local and password-locked; authentication is provided only by
//! the mesh-scoped key installed by [`super::mesh_ssh_key`], whose sshd stanza
//! accepts it solely from the Nebula overlay. Pre-enrollment machines never run
//! this reconciler because the daemon refuses to start without a pinned role.

use std::process::{Command, Output};

/// Exact cross-node service username selected for mesh file and transfer lanes.
pub const MESH_SERVICE_USER: &str = "MDE-MESH";
/// Least-privilege home exported by the default peer file mount.
pub const MESH_SERVICE_HOME: &str = "/var/lib/mde-mesh";
/// Interactive-capable shell required by sshfs, SFTP, and bounded rsync helpers.
pub const MESH_SERVICE_SHELL: &str = "/bin/bash";

/// Outcome of an idempotent account reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshServiceAccountOutcome {
    /// The account was created during this reconciliation.
    Created,
    /// The account already existed and its locked service shape was refreshed.
    Reconciled,
}

/// Reconcile the local password-locked mesh service account without invoking a
/// shell or transporting credential material.
///
/// # Errors
///
/// Returns an operator-readable error if account inspection, creation, policy
/// reconciliation, or home-directory ownership fails.
pub fn ensure_mesh_service_account() -> Result<MeshServiceAccountOutcome, String> {
    ensure_with(&LiveAccountCommands)
}

trait AccountCommands {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output>;
}

struct LiveAccountCommands;

impl AccountCommands for LiveAccountCommands {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output> {
        Command::new(program).args(args).output()
    }
}

fn ensure_with(commands: &dyn AccountCommands) -> Result<MeshServiceAccountOutcome, String> {
    let exists = commands
        .run("id", &["-u", MESH_SERVICE_USER])
        .map_err(|error| format!("inspect {MESH_SERVICE_USER} account: {error}"))?
        .status
        .success();
    let outcome = if exists {
        MeshServiceAccountOutcome::Reconciled
    } else {
        run_checked(
            commands,
            "useradd",
            &[
                "--badname",
                "--system",
                "--user-group",
                "--create-home",
                "--home-dir",
                MESH_SERVICE_HOME,
                "--shell",
                MESH_SERVICE_SHELL,
                "--password",
                "!",
                MESH_SERVICE_USER,
            ],
            "create mesh service account",
        )?;
        MeshServiceAccountOutcome::Created
    };

    run_checked(
        commands,
        "usermod",
        &[
            "--home",
            MESH_SERVICE_HOME,
            "--shell",
            MESH_SERVICE_SHELL,
            "--lock",
            MESH_SERVICE_USER,
        ],
        "reconcile mesh service account",
    )?;
    run_checked(
        commands,
        "install",
        &[
            "-d",
            "-m",
            "0700",
            "-o",
            MESH_SERVICE_USER,
            "-g",
            MESH_SERVICE_USER,
            MESH_SERVICE_HOME,
        ],
        "reconcile mesh service home",
    )?;
    Ok(outcome)
}

fn run_checked(
    commands: &dyn AccountCommands,
    program: &str,
    args: &[&str],
    context: &str,
) -> Result<(), String> {
    let output = commands
        .run(program, args)
        .map_err(|error| format!("{context}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(format!("{context}: {}", detail.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt as _;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeCommands {
        statuses: Mutex<VecDeque<i32>>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl FakeCommands {
        fn with_statuses(statuses: impl IntoIterator<Item = i32>) -> Self {
            Self {
                statuses: Mutex::new(statuses.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl AccountCommands for FakeCommands {
        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output> {
            self.calls.lock().expect("calls").push((
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ));
            let code = self
                .statuses
                .lock()
                .expect("statuses")
                .pop_front()
                .expect("planned status");
            Ok(Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: vec![],
                stderr: if code == 0 {
                    vec![]
                } else {
                    b"planned fault".to_vec()
                },
            })
        }
    }

    #[test]
    fn creates_exact_locked_service_identity_without_a_shell_command() {
        let commands = FakeCommands::with_statuses([1, 0, 0, 0]);
        assert_eq!(
            ensure_with(&commands),
            Ok(MeshServiceAccountOutcome::Created)
        );
        let calls = commands.calls.lock().expect("calls");
        assert_eq!(
            calls[0],
            ("id".into(), vec!["-u".into(), "MDE-MESH".into()])
        );
        assert_eq!(calls[1].0, "useradd");
        assert!(calls[1].1.contains(&"--badname".into()));
        assert!(calls[1].1.contains(&"--password".into()));
        assert!(calls[1].1.contains(&"!".into()));
        assert_eq!(calls[1].1.last().map(String::as_str), Some("MDE-MESH"));
        assert!(calls
            .iter()
            .all(|(program, _)| program != "sh" && program != "bash"));
    }

    #[test]
    fn existing_account_is_reconciled_without_recreation() {
        let commands = FakeCommands::with_statuses([0, 0, 0]);
        assert_eq!(
            ensure_with(&commands),
            Ok(MeshServiceAccountOutcome::Reconciled)
        );
        let calls = commands.calls.lock().expect("calls");
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|(program, _)| program != "useradd"));
    }

    #[test]
    fn creation_failure_is_not_reported_as_success() {
        let commands = FakeCommands::with_statuses([1, 7]);
        let error = ensure_with(&commands).expect_err("creation must fail");
        assert!(error.contains("create mesh service account"));
    }
}
