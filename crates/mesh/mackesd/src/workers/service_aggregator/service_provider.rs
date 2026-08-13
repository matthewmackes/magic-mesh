//! Credential-free system service readiness for WL-UX-011.
//!
//! systemd remains the sole service-state authority. This provider observes a
//! fixed platform allowlist and publishes only unit identity and coarse state;
//! command lines, environment, status text, journals, and credentials never
//! cross the boundary. It grants no lifecycle or mutation authority.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const UNIT_NAMES: [&str; 6] = [
    "NetworkManager.service",
    "cups.service",
    "libvirtd.service",
    "mackesd.service",
    "podman.service",
    "sshd.service",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceObservation {
    pub unit: String,
    pub readiness: ServiceReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProviderSnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub observed_unix_ms: u64,
    pub readiness: ServiceReadiness,
    pub services: Vec<ServiceObservation>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnitFact {
    id: String,
    load: String,
    active: String,
    enabled: String,
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@'))
}

fn parse_systemd(raw: &str) -> Option<Vec<UnitFact>> {
    if raw.len() > MAX_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    let mut facts = BTreeMap::new();
    let mut current = BTreeMap::new();
    let flush = |fields: &mut BTreeMap<&str, &str>, facts: &mut BTreeMap<String, UnitFact>| {
        if fields.is_empty() {
            return Some(());
        }
        let id = (*fields.get("Id")?).to_owned();
        let fact = UnitFact {
            id: id.clone(),
            load: (*fields.get("LoadState")?).to_owned(),
            active: (*fields.get("ActiveState")?).to_owned(),
            enabled: (*fields.get("UnitFileState")?).to_owned(),
        };
        if !safe_token(&fact.id)
            || !safe_token(&fact.load)
            || !safe_token(&fact.active)
            || !safe_token(&fact.enabled)
            || facts.insert(id, fact).is_some()
        {
            return None;
        }
        fields.clear();
        Some(())
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut current, &mut facts)?;
            continue;
        }
        let (key, value) = line.split_once('=')?;
        if !matches!(key, "Id" | "LoadState" | "ActiveState" | "UnitFileState")
            || current.insert(key, value).is_some()
        {
            return None;
        }
    }
    flush(&mut current, &mut facts)?;
    Some(facts.into_values().collect())
}

fn classify_fact(fact: &UnitFact) -> ServiceReadiness {
    if fact.load == "not-found" && fact.active == "inactive" && fact.enabled == "disabled" {
        return ServiceReadiness::Disabled;
    }
    if fact.load != "loaded" {
        return ServiceReadiness::Unknown;
    }
    match (fact.active.as_str(), fact.enabled.as_str()) {
        ("active", "enabled" | "enabled-runtime" | "static" | "indirect" | "generated") => {
            ServiceReadiness::Ready
        }
        (
            "inactive" | "failed" | "deactivating",
            "enabled" | "enabled-runtime" | "static" | "indirect" | "generated",
        ) => ServiceReadiness::Disconnected,
        ("inactive", "disabled" | "masked" | "masked-runtime") => ServiceReadiness::Disabled,
        _ => ServiceReadiness::Unknown,
    }
}

fn classify(raw: Option<&str>) -> (ServiceReadiness, Vec<ServiceObservation>, &'static str) {
    let Some(facts) = raw.and_then(parse_systemd) else {
        return (
            ServiceReadiness::Unknown,
            vec![],
            "systemd facts unavailable or malformed",
        );
    };
    let by_id = facts
        .into_iter()
        .map(|fact| (fact.id.clone(), fact))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != UNIT_NAMES.len() || UNIT_NAMES.iter().any(|unit| !by_id.contains_key(*unit)) {
        return (
            ServiceReadiness::Unknown,
            vec![],
            "systemd returned an incomplete or substituted unit set",
        );
    }
    let services = UNIT_NAMES
        .iter()
        .map(|unit| ServiceObservation {
            unit: (*unit).to_owned(),
            readiness: classify_fact(&by_id[*unit]),
        })
        .collect::<Vec<_>>();
    let readiness = if services
        .iter()
        .any(|service| service.readiness == ServiceReadiness::Unknown)
    {
        ServiceReadiness::Unknown
    } else if services
        .iter()
        .any(|service| service.readiness == ServiceReadiness::Disconnected)
    {
        ServiceReadiness::Disconnected
    } else if services
        .iter()
        .all(|service| service.readiness == ServiceReadiness::Disabled)
    {
        ServiceReadiness::Disabled
    } else {
        ServiceReadiness::Ready
    };
    (readiness, services, "bounded systemd service facts")
}

fn systemd_facts() -> Option<String> {
    let mut command = std::process::Command::new("systemctl");
    command.arg("show").args(UNIT_NAMES).args([
        "--property=Id",
        "--property=LoadState",
        "--property=ActiveState",
        "--property=UnitFileState",
        "--no-pager",
    ]);
    let output = crate::workers::proc::output_with_timeout(command, COMMAND_TIMEOUT).ok()?;
    if !output.status.success() || output.stdout.len() > MAX_OBSERVATION_BYTES {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[must_use]
pub fn state_topic(node_id: &str) -> String {
    format!("state/service-provider/{node_id}")
}

#[must_use]
pub fn gather(node_id: &str) -> ServiceProviderSnapshot {
    let (readiness, services, reason) = classify(systemd_facts().as_deref());
    ServiceProviderSnapshot {
        schema_version: 1,
        node_id: node_id.to_owned(),
        observed_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        readiness,
        services,
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, load: &str, active: &str, enabled: &str) -> String {
        format!("Id={id}\nLoadState={load}\nActiveState={active}\nUnitFileState={enabled}\n\n")
    }

    fn healthy() -> String {
        UNIT_NAMES
            .iter()
            .map(|unit| row(unit, "loaded", "active", "enabled"))
            .collect()
    }

    #[test]
    fn hostile_service_facts_fail_closed() {
        let mut missing = healthy();
        missing = missing.replacen(&row(UNIT_NAMES[0], "loaded", "active", "enabled"), "", 1);
        let substituted = healthy().replace(UNIT_NAMES[0], "attacker.service");
        let duplicate = format!(
            "{}{}",
            healthy(),
            row(UNIT_NAMES[0], "loaded", "active", "enabled")
        );
        let contradictory = healthy().replace(
            &row(UNIT_NAMES[0], "loaded", "active", "enabled"),
            &row(UNIT_NAMES[0], "loaded", "active", "disabled"),
        );
        for hostile in [
            missing,
            substituted,
            duplicate,
            contradictory,
            "x".repeat(MAX_OBSERVATION_BYTES + 1),
        ] {
            let result = classify(Some(&hostile));
            assert_eq!(result.0, ServiceReadiness::Unknown);
            assert!(
                result.1.is_empty()
                    || result
                        .1
                        .iter()
                        .any(|service| service.readiness == ServiceReadiness::Unknown)
            );
        }
    }

    #[test]
    fn explicit_service_states_remain_distinct() {
        assert_eq!(classify(Some(&healthy())).0, ServiceReadiness::Ready);
        let disconnected = healthy().replace(
            &row(UNIT_NAMES[0], "loaded", "active", "enabled"),
            &row(UNIT_NAMES[0], "loaded", "failed", "enabled"),
        );
        assert_eq!(
            classify(Some(&disconnected)).0,
            ServiceReadiness::Disconnected
        );
        let disabled = UNIT_NAMES
            .iter()
            .map(|unit| row(unit, "loaded", "inactive", "disabled"))
            .collect::<String>();
        assert_eq!(classify(Some(&disabled)).0, ServiceReadiness::Disabled);
    }
}
