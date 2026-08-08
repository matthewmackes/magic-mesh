//! Pure parsers and bounded-publish policy for local workload-adjacent probes.
//!
//! This module has no worker, Bus topic, command execution, or inventory
//! document.  It is intentionally shared only where a worker must parse a
//! command result it already obtained through its own bounded actuator/probe.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// One Podman row returned by `podman ps --format json`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PodmanContainer {
    /// Stable full container id returned by Podman.
    pub id: String,
    /// First user-facing container name returned by Podman.
    pub name: String,
    /// Runtime state returned by Podman.
    pub state: String,
    /// Immutable or tagged image reference returned by Podman.
    pub image: String,
    /// Optional sampled CPU percentage; parsers initialize it to zero.
    pub cpu_pct: f64,
    /// Optional sampled resident memory in MiB; parsers initialize it to zero.
    pub ram_mb: u64,
    /// Pod name, or empty when the container is standalone.
    #[serde(default)]
    pub pod: String,
}

/// Parse `virsh list --uuid` output into non-empty domain UUIDs.
#[must_use]
pub fn parse_virsh_uuid_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Parse the first non-CD-ROM block source from `virsh domblklist --details`.
#[must_use]
pub fn parse_virsh_domblklist(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let columns: Vec<_> = line.split_whitespace().collect();
        (columns.len() >= 4 && columns[1] == "disk" && columns[3] != "-")
            .then(|| columns[3].to_owned())
    })
}

/// Parse `podman ps --format json` without executing Podman.
#[must_use]
pub fn parse_podman_ps_json(stdout: &str) -> Vec<PodmanContainer> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| {
            let id = row.get("Id")?.as_str()?.to_owned();
            Some(PodmanContainer {
                id,
                name: row
                    .get("Names")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|names| names.first())
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                state: row
                    .get("State")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                image: row
                    .get("Image")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                cpu_pct: 0.0,
                ram_mb: 0,
                pod: row
                    .get("PodName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect()
}

/// Parse `podman stats --no-stream --format json` into per-container CPU/RAM.
#[must_use]
pub fn parse_podman_stats_json(stdout: &str) -> BTreeMap<String, (f64, u64)> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return BTreeMap::new();
    };
    rows.into_iter()
        .filter_map(|row| {
            let id = row
                .get("ContainerID")
                .or_else(|| row.get("Id"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let cpu_pct = row
                .get("CPU")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.trim_end_matches('%').trim().parse().ok())
                .unwrap_or(0.0);
            let ram_mb = row
                .get("MemUsage")
                .and_then(serde_json::Value::as_str)
                .map(parse_podman_mem_usage)
                .unwrap_or(0);
            Some((id, (cpu_pct, ram_mb)))
        })
        .collect()
}

/// Parse Podman's `MemUsage` field into MiB.
#[must_use]
pub fn parse_podman_mem_usage(value: &str) -> u64 {
    let value = value.split('/').next().unwrap_or_default().trim();
    let (number, unit) = value
        .find(|character: char| character.is_alphabetic())
        .map(|index| (&value[..index], &value[index..]))
        .unwrap_or((value, ""));
    let number: f64 = number.trim().parse().unwrap_or(0.0);
    match unit.trim().to_ascii_uppercase().as_str() {
        "KIB" | "KB" => (number / 1024.0) as u64,
        "MIB" | "MB" | "" => number as u64,
        "GIB" | "GB" => (number * 1024.0) as u64,
        "TIB" | "TB" => (number * 1024.0 * 1024.0) as u64,
        _ => number as u64,
    }
}

/// Return `true` only when the path is an actual mounted filesystem.
#[must_use]
pub fn is_meshfs_mounted(mount_path: &Path) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
        return false;
    };
    let path = mount_path.to_string_lossy();
    mounts
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .any(|mount| mount == path)
}

/// Decide whether a bounded latest-state publisher should write this tick.
#[must_use]
pub fn should_publish(
    last_body: Option<&str>,
    body: &str,
    last_publish: Option<Instant>,
    now: Instant,
    heartbeat: Duration,
) -> bool {
    match (last_body, last_publish) {
        (Some(previous), Some(at)) => previous != body || now.duration_since(at) >= heartbeat,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_do_not_need_a_live_runtime() {
        assert_eq!(
            parse_virsh_uuid_list("\na\n\nb\n"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            parse_virsh_domblklist("Type Device Target Source\nfile disk vda /pool/a.qcow2"),
            Some("/pool/a.qcow2".to_string())
        );
        let rows = parse_podman_ps_json(
            r#"[{"Id":"abc","Names":["web"],"State":"running","Image":"nginx"}]"#,
        );
        assert_eq!(rows[0].name, "web");
    }

    #[test]
    fn publish_gate_coalesces_unchanged_state() {
        let now = Instant::now();
        assert!(!should_publish(
            Some("same"),
            "same",
            Some(now),
            now,
            Duration::from_secs(1)
        ));
        assert!(should_publish(
            Some("same"),
            "changed",
            Some(now),
            now,
            Duration::from_secs(1)
        ));
    }
}
