//! DDNS-EGRESS — the dynamic-DNS config model + pure hostname/change logic
//! (design: `docs/design/ddns-egress.md`).
//!
//! When a VPN exit (or WAN) IP changes, DDNS rewrites a stable hostname under
//! `services.matthewmackes.com` to the new IP via the DigitalOcean DNS API. This
//! crate holds the durable `[ddns]` config (TOML on the shared substrate), the
//! record-name templating (`{node}-{provider}` → a FQDN in the zone), and the
//! **change-detection predicate** (only a real diff from the last-published value
//! triggers a DNS write — no churn). The `mackesd` `ddns` worker subscribes to
//! VPN-GW exit-IP changes + runs the WAN check, and the `DnsWriter` (DO) adapter
//! lives daemon-side; here is the pure, unit-tested core.

use serde::{Deserialize, Serialize};

/// What to do with a record when its tunnel goes down (kill-switch policy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnDown {
    /// Delete the record (no stale/leaking address).
    #[default]
    Remove,
    /// Point it at a sentinel address (a parked/unreachable IP).
    Sentinel,
    /// Leave the last value (identity record; reachability may be stale).
    Keep,
}

/// One managed DNS record definition.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordDef {
    /// Name template, e.g. `{node}-{provider}` → `eagle-mullvad`. Placeholders:
    /// `{node}`, `{provider}`, `{n}` (multi-instance index).
    pub name: String,
    /// IP source: a VPN-GW tunnel id (`tunnel:<id>`) or `wan` for the node WAN.
    pub source: String,
    /// Kill-switch behavior when the source is down.
    #[serde(default)]
    pub on_down: OnDown,
}

impl RecordDef {
    /// Resolve the template into the FQDN under `zone`, substituting `{node}` /
    /// `{provider}` / `{n}`. An absent `{n}` placeholder is fine (single
    /// instance). Pure + stable. The label is lowercased + non-DNS chars → `-`.
    #[must_use]
    pub fn fqdn(&self, node: &str, provider: &str, n: u32, zone: &str) -> String {
        let label = self
            .name
            .replace("{node}", node)
            .replace("{provider}", provider)
            .replace("{n}", &n.to_string());
        let label: String = label
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let label = label.trim_matches('-');
        format!("{label}.{zone}")
    }
}

/// The `[ddns]` durable config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DdnsConfig {
    /// Master enable.
    #[serde(default)]
    pub enabled: bool,
    /// DnsWriter adapter (`digitalocean` in v1).
    #[serde(default = "default_provider")]
    pub provider: String,
    /// The DNS zone records live under.
    #[serde(default = "default_zone")]
    pub zone: String,
    /// Reference to the age-encrypted API token in the mesh secret store.
    #[serde(default)]
    pub token_ref: String,
    /// Record TTL seconds (short, so a change propagates fast).
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    /// Managed records.
    #[serde(default)]
    pub record: Vec<RecordDef>,
}

fn default_provider() -> String {
    "digitalocean".to_string()
}
fn default_zone() -> String {
    "services.matthewmackes.com".to_string()
}
fn default_ttl() -> u32 {
    60
}

impl Default for DdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_provider(),
            zone: default_zone(),
            token_ref: String::new(),
            ttl: default_ttl(),
            record: Vec::new(),
        }
    }
}

impl DdnsConfig {
    /// Parse from TOML.
    ///
    /// # Errors
    /// A TOML parse error.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Serialize to TOML.
    ///
    /// # Errors
    /// A TOML serialize error.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

/// The change-detection predicate: should DDNS write `current` for a record
/// whose last-published value was `last`? Only a real change triggers a write —
/// `None` last (never published) or a differing value ⇒ yes; an unchanged value
/// ⇒ no (the no-churn rule). An empty `current` (no IP resolved yet) ⇒ no.
#[must_use]
pub fn needs_update(last: Option<&str>, current: &str) -> bool {
    if current.trim().is_empty() {
        return false;
    }
    last != Some(current)
}

/// The A/AAAA record type for an IP literal — `AAAA` for IPv6 (contains `:`),
/// else `A`. Pure helper for the DigitalOcean writer.
#[must_use]
pub fn record_type(ip: &str) -> &'static str {
    if ip.contains(':') {
        "AAAA"
    } else {
        "A"
    }
}

/// DDNS-EGRESS-2 — the DigitalOcean DNS API request to **upsert** a record: a
/// `PUT …/records/{id}` when `existing_id` is known, else a `POST …/records` to
/// create. Returns `(method, path, json_body)`; the daemon adapter attaches the
/// bearer token + executes. `name` is the bare label (the part before the zone).
/// Pure + testable — no HTTP here (keeps this lightweight crate dep-free).
#[must_use]
pub fn do_upsert_request(
    domain: &str,
    name: &str,
    ip: &str,
    ttl: u32,
    existing_id: Option<&str>,
) -> (&'static str, String, String) {
    let rtype = record_type(ip);
    let body = serde_json::json!({
        "type": rtype, "name": name, "data": ip, "ttl": ttl
    })
    .to_string();
    match existing_id {
        Some(id) => ("PUT", format!("/v2/domains/{domain}/records/{id}"), body),
        None => ("POST", format!("/v2/domains/{domain}/records"), body),
    }
}

/// DDNS-EGRESS-2 — the DigitalOcean request to **delete** a record by id
/// (`on_down = remove`). Returns `(method, path)`. Pure.
#[must_use]
pub fn do_delete_request(domain: &str, id: &str) -> (&'static str, String) {
    ("DELETE", format!("/v2/domains/{domain}/records/{id}"))
}

/// Durable path for the DDNS config: `<workgroup_root>/ddns/config.toml`.
#[must_use]
pub fn config_path(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join("ddns").join("config.toml")
}

/// Load the DDNS config (missing/malformed → default disabled).
#[must_use]
pub fn load(workgroup_root: &std::path::Path) -> DdnsConfig {
    std::fs::read_to_string(config_path(workgroup_root))
        .ok()
        .and_then(|raw| DdnsConfig::from_toml_str(&raw).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fqdn_templates_node_provider_and_sanitizes() {
        let r = RecordDef {
            name: "{node}-{provider}".into(),
            source: "tunnel:mullvad-1".into(),
            on_down: OnDown::Remove,
        };
        assert_eq!(
            r.fqdn("eagle", "mullvad", 1, "services.matthewmackes.com"),
            "eagle-mullvad.services.matthewmackes.com"
        );
        // {n} + uppercasing + non-DNS chars.
        let r2 = RecordDef {
            name: "{node}_{provider}{n}".into(),
            ..Default::default()
        };
        assert_eq!(
            r2.fqdn("Eagle", "Proton", 2, "z.example"),
            "eagle-proton2.z.example"
        );
    }

    #[test]
    fn needs_update_only_on_real_change() {
        assert!(needs_update(None, "1.2.3.4")); // never published
        assert!(needs_update(Some("1.2.3.4"), "5.6.7.8")); // changed
        assert!(!needs_update(Some("1.2.3.4"), "1.2.3.4")); // unchanged → no churn
        assert!(!needs_update(None, "")); // no IP resolved yet
        assert!(!needs_update(Some("1.2.3.4"), "  ")); // blank current
    }

    #[test]
    fn config_defaults_and_round_trip() {
        let c = DdnsConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.provider, "digitalocean");
        assert_eq!(c.zone, "services.matthewmackes.com");
        assert_eq!(c.ttl, 60);
        let mut c2 = c.clone();
        c2.enabled = true;
        c2.record.push(RecordDef {
            name: "{node}-{provider}".into(),
            source: "wan".into(),
            on_down: OnDown::Keep,
        });
        let s = c2.to_toml_string().unwrap();
        assert_eq!(DdnsConfig::from_toml_str(&s).unwrap(), c2);
    }

    #[test]
    fn do_requests_create_update_delete() {
        // No existing id → POST create.
        let (m, p, b) =
            do_upsert_request("matthewmackes.com", "eagle-mullvad", "1.2.3.4", 60, None);
        assert_eq!(m, "POST");
        assert_eq!(p, "/v2/domains/matthewmackes.com/records");
        assert!(b.contains("\"type\":\"A\"") && b.contains("\"data\":\"1.2.3.4\""));
        // Existing id → PUT update.
        let (m, p, _) = do_upsert_request(
            "matthewmackes.com",
            "eagle-mullvad",
            "1.2.3.4",
            60,
            Some("99"),
        );
        assert_eq!(m, "PUT");
        assert_eq!(p, "/v2/domains/matthewmackes.com/records/99");
        // IPv6 → AAAA.
        let (_, _, b6) = do_upsert_request("z", "n", "2001:db8::1", 60, None);
        assert!(b6.contains("\"type\":\"AAAA\""));
        // Delete.
        assert_eq!(
            do_delete_request("matthewmackes.com", "99"),
            (
                "DELETE",
                "/v2/domains/matthewmackes.com/records/99".to_string()
            )
        );
    }

    #[test]
    fn record_type_picks_a_or_aaaa() {
        assert_eq!(record_type("1.2.3.4"), "A");
        assert_eq!(record_type("2001:db8::1"), "AAAA");
    }

    #[test]
    fn load_missing_is_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()), DdnsConfig::default());
    }
}
