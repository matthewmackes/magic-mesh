//! Credential-free privacy-broker readiness for WL-UX-011.
//!
//! The desktop portal mediates application requests, polkit mediates privileged
//! host operations, and SELinux provides the kernel enforcement boundary. This
//! provider cross-checks those three authorities but publishes only a bounded
//! readiness projection. Application identities, grants, devices, labels,
//! credentials, logs, and command output never cross this boundary.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OBSERVATION_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacySnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub observed_unix_ms: u64,
    pub readiness: PrivacyReadiness,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceFact {
    Active,
    Inactive,
    Disabled,
}

fn parse_service(raw: &str) -> Option<ServiceFact> {
    if raw.is_empty() || raw.len() > MAX_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    let mut active = None;
    let mut unit_file = None;
    for line in raw.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "ActiveState" if active.replace(value).is_none() => {}
            "UnitFileState" if unit_file.replace(value).is_none() => {}
            _ => return None,
        }
    }
    match (active?, unit_file?) {
        ("active", "enabled" | "static" | "indirect" | "generated") => Some(ServiceFact::Active),
        ("inactive" | "failed", "enabled" | "static" | "indirect" | "generated") => {
            Some(ServiceFact::Inactive)
        }
        ("inactive" | "failed", "disabled" | "masked") => Some(ServiceFact::Disabled),
        _ => None,
    }
}

fn parse_lsm(raw: &str) -> Option<bool> {
    if raw.is_empty()
        || raw.len() > MAX_OBSERVATION_BYTES
        || raw.contains('\0')
        || raw.lines().count() != 1
    {
        return None;
    }
    let mut names = raw.trim().split(',').collect::<Vec<_>>();
    if names.is_empty()
        || names.len() > 64
        || names.iter().any(|name| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
    {
        return None;
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    Some(names.contains(&"selinux"))
}

fn parse_enforcing(raw: &str) -> Option<bool> {
    match raw {
        "1\n" | "1" => Some(true),
        "0\n" | "0" => Some(false),
        _ => None,
    }
}

fn classify(
    portal: Option<&str>,
    polkit: Option<&str>,
    lsm: Option<&str>,
    enforcing: Option<&str>,
) -> (PrivacyReadiness, &'static str) {
    let (Some(portal), Some(polkit), Some(selinux), Some(enforcing)) = (
        portal.and_then(parse_service),
        polkit.and_then(parse_service),
        lsm.and_then(parse_lsm),
        enforcing.and_then(parse_enforcing),
    ) else {
        return (
            PrivacyReadiness::Unknown,
            "privacy authority facts unavailable or malformed",
        );
    };
    if selinux != enforcing {
        return (
            PrivacyReadiness::Unknown,
            "kernel privacy authority facts contradict each other",
        );
    }
    if portal == ServiceFact::Active && polkit == ServiceFact::Active && selinux && enforcing {
        return (
            PrivacyReadiness::Ready,
            "portal, policy, and kernel privacy authorities agree",
        );
    }
    if portal == ServiceFact::Disabled && polkit == ServiceFact::Disabled && !selinux && !enforcing
    {
        return (
            PrivacyReadiness::Disabled,
            "privacy authorities are explicitly disabled",
        );
    }
    (
        PrivacyReadiness::Disconnected,
        "privacy authorities are present but not fully connected",
    )
}

fn bounded_output(command: Command) -> Option<String> {
    let output = super::proc::output_with_timeout(command, COMMAND_TIMEOUT).ok()?;
    if !output.status.success()
        || output.stdout.len() > MAX_OBSERVATION_BYTES
        || output.stderr.len() > MAX_OBSERVATION_BYTES
    {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    (!text.contains('\0')).then_some(text)
}

fn system_service(unit: &str) -> Option<String> {
    let mut command = Command::new("systemctl");
    command.args(["show", "--property=ActiveState,UnitFileState", unit]);
    bounded_output(command)
}

fn portal_service() -> Option<String> {
    let uid = bounded_output({
        let mut command = Command::new("id");
        command.args(["-u", "mm"]);
        command
    })?;
    let uid = uid.trim();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut command = Command::new("runuser");
    command.args([
        "-u",
        "mm",
        "--",
        "env",
        &format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
        "systemctl",
        "--user",
        "show",
        "--property=ActiveState,UnitFileState",
        "xdg-desktop-portal.service",
    ]);
    bounded_output(command)
}

fn gather(node_id: &str) -> PrivacySnapshot {
    let portal = portal_service();
    let polkit = system_service("polkit.service");
    let lsm = std::fs::read_to_string("/sys/kernel/security/lsm").ok();
    let enforcing = std::fs::read_to_string("/sys/fs/selinux/enforce").ok();
    let (readiness, reason) = classify(
        portal.as_deref(),
        polkit.as_deref(),
        lsm.as_deref(),
        enforcing.as_deref(),
    );
    PrivacySnapshot {
        schema_version: 1,
        node_id: node_id.to_owned(),
        observed_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        readiness,
        reason: reason.to_owned(),
    }
}

fn snapshot_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join("privacy-provider")
        .join(format!("{node_id}.json"))
}

/// Publish one current observation. This grants no mutation authority.
pub fn publish_system(workgroup_root: &Path, node_id: &str) -> std::io::Result<PathBuf> {
    if node_id.is_empty()
        || node_id.len() > 128
        || !node_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(std::io::Error::other(
            "invalid privacy-provider node identity",
        ));
    }
    let path = snapshot_path(workgroup_root, node_id);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("privacy snapshot has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{node_id}.json.tmp"));
    let bytes = serde_json::to_vec_pretty(&gather(node_id)).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: &str = "ActiveState=active\nUnitFileState=enabled\n";
    const STATIC_ACTIVE: &str = "ActiveState=active\nUnitFileState=static\n";
    const DISABLED: &str = "ActiveState=inactive\nUnitFileState=disabled\n";

    #[test]
    fn hostile_privacy_observations_fail_unknown_without_leaking_provider_data() {
        let oversized = "x".repeat(MAX_OBSERVATION_BYTES + 1);
        let hostile = [
            classify(
                Some(ACTIVE),
                Some(STATIC_ACTIVE),
                Some("selinux\n"),
                Some("0\n"),
            ),
            classify(
                Some("ActiveState=active\nActiveState=active\nUnitFileState=enabled\n"),
                Some(STATIC_ACTIVE),
                Some("selinux\n"),
                Some("1\n"),
            ),
            classify(
                Some(ACTIVE),
                Some("subject=user secret\n"),
                Some("selinux\n"),
                Some("1\n"),
            ),
            classify(
                Some(ACTIVE),
                Some(STATIC_ACTIVE),
                Some("selinux,selinux\n"),
                Some("1\n"),
            ),
            classify(
                Some(&oversized),
                Some(STATIC_ACTIVE),
                Some("selinux\n"),
                Some("1\n"),
            ),
            classify(
                Some(ACTIVE),
                Some(STATIC_ACTIVE),
                Some("selinux\0apparmor\n"),
                Some("1\n"),
            ),
        ];
        assert!(hostile
            .iter()
            .all(|(readiness, _)| *readiness == PrivacyReadiness::Unknown));
        for (_, reason) in hostile {
            assert!(!reason.contains("user"));
            assert!(!reason.contains("secret"));
            assert!(!reason.contains("device"));
        }

        assert_eq!(
            classify(
                Some(ACTIVE),
                Some(STATIC_ACTIVE),
                Some("lockdown,selinux\n"),
                Some("1\n")
            )
            .0,
            PrivacyReadiness::Ready
        );
        assert_eq!(
            classify(
                Some(DISABLED),
                Some(DISABLED),
                Some("lockdown\n"),
                Some("0\n")
            )
            .0,
            PrivacyReadiness::Disabled
        );
        assert_eq!(
            classify(
                Some(ACTIVE),
                Some(STATIC_ACTIVE),
                Some("lockdown,selinux\n"),
                Some("1\n")
            )
            .1,
            "portal, policy, and kernel privacy authorities agree"
        );
    }
}
