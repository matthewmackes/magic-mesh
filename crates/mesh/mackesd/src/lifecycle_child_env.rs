//! Leftover (2): dest identity env stays off lifecycle grandchildren.
//!
//! Dest-env sources `MACKESD_BOOTSTRAP_*` for the worker only. `mackesd`
//! leave/join/found/mesh-init children (`systemctl`, `bash`, setup helpers)
//! must not inherit those names.

use std::process::Command;

/// Dest identity and join-token env must not leak into grandchildren.
pub const LIFECYCLE_CHILD_ENV_STRIP: &[&str] = &[
    "MACKESD_BOOTSTRAP_SSH_KEY",
    "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
    "JOIN_TOKEN",
];

/// Remove dest identity and join-token env from a spawned child.
pub fn strip_lifecycle_child_env(command: &mut Command) {
    for name in LIFECYCLE_CHILD_ENV_STRIP {
        command.env_remove(*name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bootstrap_dest_env() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf %s \"$MACKESD_BOOTSTRAP_SSH_KEY$MACKESD_BOOTSTRAP_KNOWN_HOSTS$JOIN_TOKEN\"",
        ]);
        command.env("MACKESD_BOOTSTRAP_SSH_KEY", "/tmp/must-not-leak");
        command.env("MACKESD_BOOTSTRAP_KNOWN_HOSTS", "/tmp/must-not-leak-hosts");
        command.env("JOIN_TOKEN", "must-not-leak-token");
        strip_lifecycle_child_env(&mut command);
        let output = command.output().expect("run stripped child");
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "lifecycle child inherited dest env: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
