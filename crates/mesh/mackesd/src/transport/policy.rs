//! KDC2-1.11 — policy.toml loader.
//!
//! Parses `/etc/mde/connect/policy.toml` (system default) and
//! `~/.config/mde/connect/policy.toml` (operator override) into
//! `mackes_transport::scorer::Policy`. The merge is shallow per
//! top-level section — a user file that omits `[weights]`
//! inherits the system defaults for that section unchanged.
//!
//! Hot reload via inotify is a follow-up (KDC2-1.11.a).

use std::collections::BTreeMap;
use std::path::Path;

use mackes_transport::scorer::{ClassWeights, Policy};
use mackes_transport::TransportKind;
use serde::Deserialize;

/// Parse errors a caller can act on. Stable variants so the
/// audit chain can log a `family` token.
#[derive(Debug)]
pub enum PolicyError {
    /// File present but TOML parse failed.
    InvalidToml(toml::de::Error),
    /// File present + parsed, but a TransportKind token didn't
    /// match any known variant.
    UnknownTransportKind(String),
    /// I/O failure reading the policy file.
    Io(std::io::Error),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::InvalidToml(e) => write!(f, "invalid_toml: {e}"),
            PolicyError::UnknownTransportKind(s) => {
                write!(f, "unknown_transport_kind: {s}")
            }
            PolicyError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for PolicyError {}

// ────────────────────────────────────────────────────────────────
// On-disk TOML schema. Every field optional so partial files
// merge cleanly onto the baseline.
// ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct PolicyFile {
    weights: Option<WeightsSection>,
    flap_penalty: Option<f32>,
    pinned_primary: Option<Vec<String>>,
    denylist: Option<Vec<String>>,
    plugins: Option<PluginsSection>,
    /// CV-1 — content-class encryption floor token
    /// (`"aes256_gcm"` / `"chacha20_poly1305"` / `"aes128_gcm"` /
    /// `"none"`). Absent → the AES-256-class baseline.
    min_content_encryption: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WeightsSection {
    latency: Option<f32>,
    throughput: Option<f32>,
    reliability: Option<f32>,
}

#[derive(Debug, Default, Deserialize)]
struct PluginsSection {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    /// KDC2-3.11.a — per-plugin sub-tables. The TOML form is
    /// `[plugins.<name>]` (e.g. `[plugins.run_command]`) with
    /// an `allow_devices = [...]` array inside. Captured as a
    /// flat map keyed by plugin name so the loader can extract
    /// `allow_devices` regardless of which plugins the operator
    /// listed.
    #[serde(default, flatten)]
    per_plugin: BTreeMap<String, PerPluginSection>,
}

#[derive(Debug, Default, Deserialize)]
struct PerPluginSection {
    /// Device ids that may invoke this plugin. Empty / absent
    /// means "fall through to the top-level allow/deny" —
    /// non-empty narrows the allow set to exactly these ids.
    #[serde(default)]
    allow_devices: Vec<String>,
}

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

/// Loaded policy with both the scorer-facing `Policy` and the
/// plugin allow/deny lists the host integration (KDC2-3.11)
/// enforces.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedPolicy {
    /// Scorer policy consumed by the mesh-router.
    pub scorer: Policy,
    /// Plugins explicitly allowed. Empty = "allow every plugin
    /// not in `deny`."
    pub plugin_allow: Vec<String>,
    /// Plugins explicitly denied. Wins over `allow`.
    pub plugin_deny: Vec<String>,
    /// KDC2-3.11.a — per-plugin device allowlists. Keyed by
    /// plugin token (e.g. `"run_command"`). When a plugin has
    /// a non-empty entry, only the listed device ids may invoke
    /// it (overrides both `plugin_allow` and `plugin_deny`).
    /// Absent / empty entries fall through to the top-level
    /// allow/deny lists unchanged.
    pub plugin_per_device_allow: BTreeMap<String, Vec<String>>,
}

impl LoadedPolicy {
    /// Baseline + denies `run_command` by default (matches the
    /// shipped system policy.toml). Used when neither system
    /// nor user file is readable — keep the daemon running
    /// rather than refusing to start.
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            scorer: Policy::baseline(),
            plugin_allow: Vec::new(),
            plugin_deny: vec!["run_command".to_string()],
            plugin_per_device_allow: BTreeMap::new(),
        }
    }

    /// KDC2-3.11.a — per-device gating decision. When the
    /// plugin has an entry in `plugin_per_device_allow` and
    /// the list is non-empty, only the listed device ids are
    /// allowed (overriding `plugin_allow`/`plugin_deny`).
    /// Falls through to [`plugin_allowed`] otherwise.
    #[must_use]
    pub fn plugin_allowed_for_device(&self, name: &str, device_id: &str) -> bool {
        if let Some(allow_devices) = self.plugin_per_device_allow.get(name) {
            if !allow_devices.is_empty() {
                return allow_devices.iter().any(|d| d == device_id);
            }
        }
        self.plugin_allowed(name)
    }

    /// Plugin policy decision for a given plugin token (e.g.
    /// "clipboard", "run_command"). Returns true when the plugin
    /// is allowed.
    ///
    /// Deny wins. If `plugin_allow` is non-empty, only plugins
    /// in `plugin_allow` are allowed. Empty `plugin_allow` means
    /// "everything not denied is allowed."
    #[must_use]
    pub fn plugin_allowed(&self, name: &str) -> bool {
        if self.plugin_deny.iter().any(|n| n == name) {
            return false;
        }
        if self.plugin_allow.is_empty() {
            return true;
        }
        self.plugin_allow.iter().any(|n| n == name)
    }
}

// KDC2-3.11 — `LoadedPolicy` IS the `PluginAuthority` consumed
// by the KDC dispatch check. E2.2 (2026-06-05) — the dispatch
// policy trait moved to the canonical `mde-kdc-proto` (a pure,
// always-on dep), so this impl is no longer gated on
// `async-services` and the legacy `mde-kdc` host is gone.
impl mde_kdc_proto::dispatch::PluginAuthority for LoadedPolicy {
    fn plugin_allowed(&self, name: &str) -> bool {
        LoadedPolicy::plugin_allowed(self, name)
    }

    fn plugin_allowed_for_device(&self, name: &str, device_id: &str) -> bool {
        LoadedPolicy::plugin_allowed_for_device(self, name, device_id)
    }
}

/// Parse a single TOML string into a [`LoadedPolicy`]. Used by
/// both [`load_with_paths`] (system + user merge) and unit tests
/// (in-memory string).
pub fn parse_policy(raw: &str) -> Result<LoadedPolicy, PolicyError> {
    let file: PolicyFile = toml::from_str(raw).map_err(PolicyError::InvalidToml)?;
    file.into_loaded()
}

/// Load the system + user policy files and merge into a single
/// [`LoadedPolicy`]. Either file may be absent — the loader
/// silently falls back to the baseline for missing files. Hard
/// failures (present-but-invalid TOML, unknown TransportKind)
/// surface as `Err(PolicyError)` so the daemon can decide
/// whether to refuse to start (recommended for production) or
/// fall back to baseline (recommended for dev).
pub fn load_with_paths(system: &Path, user: &Path) -> Result<LoadedPolicy, PolicyError> {
    let system_file = read_optional(system)?
        .map(|raw| toml::from_str::<PolicyFile>(&raw).map_err(PolicyError::InvalidToml))
        .transpose()?
        .unwrap_or_default();
    let user_file = read_optional(user)?
        .map(|raw| toml::from_str::<PolicyFile>(&raw).map_err(PolicyError::InvalidToml))
        .transpose()?
        .unwrap_or_default();
    merge(system_file, user_file).into_loaded()
}

fn read_optional(path: &Path) -> Result<Option<String>, PolicyError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PolicyError::Io(e)),
    }
}

/// Shallow merge: user overrides system per top-level section.
/// A section absent from `user` keeps the system value.
fn merge(system: PolicyFile, user: PolicyFile) -> PolicyFile {
    PolicyFile {
        weights: user.weights.or(system.weights),
        flap_penalty: user.flap_penalty.or(system.flap_penalty),
        pinned_primary: user.pinned_primary.or(system.pinned_primary),
        denylist: user.denylist.or(system.denylist),
        plugins: user.plugins.or(system.plugins),
        min_content_encryption: user
            .min_content_encryption
            .or(system.min_content_encryption),
    }
}

impl PolicyFile {
    fn into_loaded(self) -> Result<LoadedPolicy, PolicyError> {
        let baseline = LoadedPolicy::baseline();
        let scorer = Policy {
            weights: ClassWeights {
                latency: self
                    .weights
                    .as_ref()
                    .and_then(|w| w.latency)
                    .unwrap_or(baseline.scorer.weights.latency),
                throughput: self
                    .weights
                    .as_ref()
                    .and_then(|w| w.throughput)
                    .unwrap_or(baseline.scorer.weights.throughput),
                reliability: self
                    .weights
                    .as_ref()
                    .and_then(|w| w.reliability)
                    .unwrap_or(baseline.scorer.weights.reliability),
            },
            flap_penalty: self.flap_penalty.unwrap_or(baseline.scorer.flap_penalty),
            pinned_primary: parse_transport_kinds(self.pinned_primary.unwrap_or_default())?,
            denylist: parse_transport_kinds(self.denylist.unwrap_or_default())?,
            // CV-1 — operator-tunable content-encryption floor; an
            // unknown token is a hard parse error (never silently
            // weaken the floor on a typo).
            min_content_encryption: match self.min_content_encryption {
                None => baseline.scorer.min_content_encryption,
                Some(s) => {
                    serde_json::from_value(serde_json::Value::String(s.clone())).map_err(|_| {
                        PolicyError::UnknownTransportKind(format!("min_content_encryption: {s}"))
                    })?
                }
            },
        };
        let plugins = self.plugins.unwrap_or_default();
        let plugin_allow = plugins.allow;
        // If the file specifies a plugins section but its `deny`
        // is empty, that's an intentional "no denies" — don't
        // re-inject the baseline's run_command deny.
        let plugin_deny = if plugins.deny.is_empty() {
            // No deny key at all → inherit baseline. Empty list
            // explicitly written → respect it.
            // We can't tell the two apart from the deserialized
            // struct alone, so we err on the side of "explicit
            // file wins" — user must include the deny they want.
            plugins.deny.iter().map(|s| s.to_string()).collect()
        } else {
            plugins.deny
        };
        let plugin_per_device_allow: BTreeMap<String, Vec<String>> = plugins
            .per_plugin
            .into_iter()
            .filter(|(_, sub)| !sub.allow_devices.is_empty())
            .map(|(name, sub)| (name, sub.allow_devices))
            .collect();
        Ok(LoadedPolicy {
            scorer,
            plugin_allow,
            plugin_deny,
            plugin_per_device_allow,
        })
    }
}

fn parse_transport_kinds(names: Vec<String>) -> Result<Vec<TransportKind>, PolicyError> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let kind = match name.as_str() {
            "direct_udp" => TransportKind::NebulaDirect,
            "derp_relay" => TransportKind::NebulaLighthouseRelay,
            "https443" => TransportKind::NebulaHttps443,
            "kdc_tls" => TransportKind::KdcTls,
            other => return Err(PolicyError::UnknownTransportKind(other.to_string())),
        };
        out.push(kind);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_string_yields_baseline() {
        let p = parse_policy("").unwrap();
        assert_eq!(p.scorer, Policy::baseline());
    }

    #[test]
    fn invalid_toml_surfaces_error() {
        let p = parse_policy("this is = not [ valid toml");
        assert!(matches!(p, Err(PolicyError::InvalidToml(_))));
    }

    #[test]
    fn weights_partial_override_inherits_unspecified() {
        let raw = r#"
            [weights]
            latency = 0.9
        "#;
        let p = parse_policy(raw).unwrap();
        assert!((p.scorer.weights.latency - 0.9).abs() < 1e-6);
        // Unspecified weights inherit baseline.
        assert!((p.scorer.weights.throughput - 0.7).abs() < 1e-6);
        assert!((p.scorer.weights.reliability - 0.7).abs() < 1e-6);
    }

    #[test]
    fn flap_penalty_override() {
        let raw = "flap_penalty = 0.5";
        let p = parse_policy(raw).unwrap();
        assert!((p.scorer.flap_penalty - 0.5).abs() < 1e-6);
    }

    #[test]
    fn pinned_primary_parses_known_kinds() {
        let raw = r#"pinned_primary = ["direct_udp", "kdc_tls"]"#;
        let p = parse_policy(raw).unwrap();
        assert_eq!(
            p.scorer.pinned_primary,
            vec![TransportKind::NebulaDirect, TransportKind::KdcTls],
        );
    }

    #[test]
    fn unknown_transport_kind_errors() {
        let raw = r#"denylist = ["wormhole"]"#;
        let p = parse_policy(raw);
        match p {
            Err(PolicyError::UnknownTransportKind(s)) => assert_eq!(s, "wormhole"),
            other => panic!("expected UnknownTransportKind, got {other:?}"),
        }
    }

    #[test]
    fn plugins_section_parses_allow_deny() {
        let raw = r#"
            [plugins]
            allow = ["clipboard"]
            deny  = ["run_command", "sms"]
        "#;
        let p = parse_policy(raw).unwrap();
        assert_eq!(p.plugin_allow, vec!["clipboard"]);
        assert_eq!(p.plugin_deny, vec!["run_command", "sms"]);
    }

    #[test]
    fn plugin_allowed_honors_deny_over_allow() {
        // Even if a plugin is in allow, deny wins.
        let p = LoadedPolicy {
            scorer: Policy::baseline(),
            plugin_allow: vec!["clipboard".into()],
            plugin_deny: vec!["clipboard".into()],
            plugin_per_device_allow: BTreeMap::new(),
        };
        assert!(!p.plugin_allowed("clipboard"));
    }

    #[test]
    fn plugin_allowed_falls_through_to_default_when_allow_empty() {
        // Empty allow list = "permit everything not denied."
        let p = LoadedPolicy {
            scorer: Policy::baseline(),
            plugin_allow: vec![],
            plugin_deny: vec!["run_command".into()],
            plugin_per_device_allow: BTreeMap::new(),
        };
        assert!(p.plugin_allowed("clipboard"));
        assert!(!p.plugin_allowed("run_command"));
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-3.11.a — per-device gating
    // ─────────────────────────────────────────────────────────

    #[test]
    fn per_device_allow_overrides_deny_for_listed_device() {
        // run_command is denied globally, but device "abc-123"
        // is on the per-device allowlist → allowed.
        let mut per = BTreeMap::new();
        per.insert("run_command".to_string(), vec!["abc-123".to_string()]);
        let p = LoadedPolicy {
            scorer: Policy::baseline(),
            plugin_allow: vec![],
            plugin_deny: vec!["run_command".into()],
            plugin_per_device_allow: per,
        };
        assert!(p.plugin_allowed_for_device("run_command", "abc-123"));
        assert!(!p.plugin_allowed_for_device("run_command", "other-id"));
    }

    #[test]
    fn per_device_allow_narrows_otherwise_allowed_plugin() {
        // clipboard is allowed globally, but the per-device
        // entry narrows to a specific device only.
        let mut per = BTreeMap::new();
        per.insert("clipboard".to_string(), vec!["only-me".to_string()]);
        let p = LoadedPolicy {
            scorer: Policy::baseline(),
            plugin_allow: vec![],
            plugin_deny: vec![],
            plugin_per_device_allow: per,
        };
        assert!(p.plugin_allowed_for_device("clipboard", "only-me"));
        assert!(!p.plugin_allowed_for_device("clipboard", "someone-else"));
    }

    #[test]
    fn per_device_absent_falls_through_to_top_level_policy() {
        // No per-device entry → top-level allow/deny decides.
        let p = LoadedPolicy {
            scorer: Policy::baseline(),
            plugin_allow: vec![],
            plugin_deny: vec!["run_command".into()],
            plugin_per_device_allow: BTreeMap::new(),
        };
        assert!(p.plugin_allowed_for_device("clipboard", "any-id"));
        assert!(!p.plugin_allowed_for_device("run_command", "any-id"));
    }

    #[test]
    fn per_device_parses_from_subtable_toml() {
        let raw = r#"
            [plugins]
            deny = ["run_command"]

            [plugins.run_command]
            allow_devices = ["abc-123", "trusted-laptop"]
        "#;
        let p = parse_policy(raw).unwrap();
        assert_eq!(p.plugin_deny, vec!["run_command".to_string()]);
        let entry = p.plugin_per_device_allow.get("run_command").unwrap();
        assert_eq!(
            entry,
            &vec!["abc-123".to_string(), "trusted-laptop".to_string()]
        );
        // Behavior lock — the listed device gets through.
        assert!(p.plugin_allowed_for_device("run_command", "abc-123"));
        assert!(!p.plugin_allowed_for_device("run_command", "some-other"));
    }

    #[test]
    fn load_with_paths_missing_system_and_user_falls_back_to_baseline() {
        let tmp = tempdir().unwrap();
        let p = load_with_paths(
            &tmp.path().join("system.toml"),
            &tmp.path().join("user.toml"),
        )
        .unwrap();
        assert_eq!(p.scorer, Policy::baseline());
    }

    #[test]
    fn user_file_overrides_system_for_named_sections() {
        let tmp = tempdir().unwrap();
        let sys_path = tmp.path().join("system.toml");
        let user_path = tmp.path().join("user.toml");
        std::fs::write(
            &sys_path,
            r#"
            flap_penalty = 0.1

            [weights]
            latency = 0.5
            throughput = 0.5
            reliability = 0.5
            "#,
        )
        .unwrap();
        std::fs::write(
            &user_path,
            r#"
            [weights]
            latency = 0.9
            "#,
        )
        .unwrap();
        let p = load_with_paths(&sys_path, &user_path).unwrap();
        // user [weights] section wins (shallow merge per
        // section). The user's incomplete section means
        // throughput / reliability inherit baseline (not
        // system) — the merge is per-top-level-section, not
        // per-field. Documented as a known limitation; a
        // future per-field merge is captured under KDC2-1.11.b.
        assert!((p.scorer.weights.latency - 0.9).abs() < 1e-6);
        assert!((p.scorer.weights.throughput - 0.7).abs() < 1e-6); // baseline default
                                                                   // flap_penalty wasn't in user → inherits SYSTEM (not baseline).
        assert!((p.scorer.flap_penalty - 0.1).abs() < 1e-6);
    }

    #[test]
    fn baseline_loaded_policy_denies_run_command() {
        // Hardcoded default — even with zero policy files, the
        // baseline LoadedPolicy denies run_command. The
        // operator must explicitly opt-in.
        let p = LoadedPolicy::baseline();
        assert!(!p.plugin_allowed("run_command"));
        assert!(p.plugin_allowed("clipboard"));
    }
}
