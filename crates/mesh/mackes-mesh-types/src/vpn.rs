//! VPN-GW-1 — the VPN tunnel definition model + pure helpers (design:
//! `docs/design/vpn-gateway.md`).
//!
//! A node runs N named **tunnels**, each an internet-egress layer on top of the
//! mesh. This crate holds the durable model (TOML on the shared substrate), the
//! `mvpn-<id>` interface-name derivation (bounded to Linux's 15-char `IFNAMSIZ`),
//! and the `wg-quick` / `openvpn` argv builders — all pure + unit-tested. The
//! `mackesd` `vpn_gateway` worker brings tunnels up/down by spawning these argv
//! and serves `action/vpn/*`; the secret material (keys/.ovpn) is age-encrypted
//! in the mesh secret store, never in this config.

use serde::{Deserialize, Serialize};

/// Linux `IFNAMSIZ` is 16 incl. the NUL → 15 usable chars for an interface name.
pub const IFNAME_MAX: usize = 15;

/// How a tunnel is brought up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    /// WireGuard via `wg-quick` on a rendered config (the primary path).
    #[default]
    Wg,
    /// OpenVPN via `openvpn` on an imported `.ovpn`.
    Ovpn,
    /// A provider CLI (`mullvad`/`protonvpn-cli`/`nordvpn`).
    Cli,
    /// A provider API/config-generator (mints a WG config / picks a server).
    Api,
}

/// One named tunnel definition. Secret material is referenced by `creds_ref`
/// (an age-encrypted blob in the mesh secret store), never inlined here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDef {
    /// Operator-chosen id, unique within the node (drives `mvpn-<id>`).
    pub id: String,
    /// Provider label (`mullvad`/`proton`/…/`generic-wg`/`generic-ovpn`).
    pub provider: String,
    /// How it's brought up.
    #[serde(default)]
    pub method: Method,
    /// Server/region selector (provider-specific; may be empty for generic).
    #[serde(default)]
    pub server: String,
    /// Transport hint (`udp`/`tcp`); OpenVPN obfuscation → tcp.
    #[serde(default)]
    pub protocol: String,
    /// Reference to the age-encrypted creds in the mesh secret store.
    #[serde(default)]
    pub creds_ref: String,
}

impl TunnelDef {
    /// The dedicated interface name `mvpn-<id>`, sanitized + bounded to
    /// [`IFNAME_MAX`] (Linux refuses longer names). Non-alphanumeric id chars
    /// collapse to nothing; the `mvpn-` prefix is always kept. Pure + stable.
    #[must_use]
    pub fn ifname(&self) -> String {
        const PREFIX: &str = "mvpn-";
        let body: String = self
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(IFNAME_MAX - PREFIX.len())
            .collect();
        format!("{PREFIX}{body}")
    }

    /// Validate the definition is usable: non-empty id whose `ifname` body isn't
    /// empty after sanitizing (else two ids could collide on the bare prefix).
    ///
    /// # Errors
    /// A human-readable reason.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("tunnel id is empty".into());
        }
        if self.ifname() == "mvpn-" {
            return Err(format!(
                "tunnel id '{}' has no alphanumeric chars for the interface name",
                self.id
            ));
        }
        Ok(())
    }
}

/// The node's VPN config — the durable set of tunnel definitions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnConfig {
    /// Per-node tunnel definitions.
    #[serde(default)]
    pub tunnel: Vec<TunnelDef>,
}

impl VpnConfig {
    /// Parse from TOML (missing sections → empty).
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

    /// Look up a tunnel by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&TunnelDef> {
        self.tunnel.iter().find(|t| t.id == id)
    }

    /// Insert or replace a tunnel (keyed by id).
    pub fn upsert(&mut self, t: TunnelDef) {
        if let Some(e) = self.tunnel.iter_mut().find(|x| x.id == t.id) {
            *e = t;
        } else {
            self.tunnel.push(t);
        }
    }

    /// Remove a tunnel by id; `true` if one was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.tunnel.len();
        self.tunnel.retain(|t| t.id != id);
        self.tunnel.len() != before
    }

    /// Validate every tunnel + that interface names don't collide (two ids that
    /// sanitize to the same `mvpn-<body>` can't run concurrently).
    ///
    /// # Errors
    /// The first inconsistency's reason.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for t in &self.tunnel {
            t.validate()?;
            let ifn = t.ifname();
            if !seen.insert(ifn.clone()) {
                return Err(format!("interface name collision: {ifn}"));
            }
        }
        Ok(())
    }
}

/// Durable path for the VPN config: `<workgroup_root>/vpn/tunnels.toml`.
#[must_use]
pub fn config_path(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join("vpn").join("tunnels.toml")
}

/// Load the VPN config (missing/malformed → default empty).
#[must_use]
pub fn load(workgroup_root: &std::path::Path) -> VpnConfig {
    std::fs::read_to_string(config_path(workgroup_root))
        .ok()
        .and_then(|raw| VpnConfig::from_toml_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the VPN config (validate → atomic temp+rename).
///
/// # Errors
/// Validation failure, or an I/O / serialize error.
pub fn save(
    workgroup_root: &std::path::Path,
    cfg: &VpnConfig,
) -> Result<std::path::PathBuf, String> {
    cfg.validate()?;
    let path = config_path(workgroup_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let toml = cfg.to_toml_string().map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    Ok(path)
}

/// The `wg-quick up <ifname>` argv (the config is written to
/// `/etc/wireguard/<ifname>.conf` by the worker from the decrypted creds).
#[must_use]
pub fn wg_quick_argv(t: &TunnelDef, up: bool) -> Vec<String> {
    vec![
        "wg-quick".into(),
        if up { "up".into() } else { "down".into() },
        t.ifname(),
    ]
}

/// The `openvpn` argv to bring a tunnel up against its `.ovpn` at `config_path`,
/// naming the device `mvpn-<id>` so it matches the egress policy routing.
#[must_use]
pub fn openvpn_argv(t: &TunnelDef, config_path: &str) -> Vec<String> {
    vec![
        "openvpn".into(),
        "--config".into(),
        config_path.into(),
        "--dev".into(),
        t.ifname(),
        "--daemon".into(),
    ]
}

// ── VPN-GW-2 — encrypted, leader-managed tunnel secrets ─────────────────────
//
// The cleartext key material (a WireGuard `[Interface]/[Peer]` config or an
// OpenVPN `.ovpn` + creds) never lives in `tunnels.toml` — only `creds_ref`
// does. The leader seals each tunnel's [`TunnelSecret`] under the mesh CA key
// and drops the `.age` blob under `secrets/vpn/<node>/` on the shared substrate
// (the XCP-7 / EFF-21 pattern); the assigned node decrypts it and materializes
// the cleartext to the bring-up path VPN-GW-1 already spawns against. The
// payload + path derivation are pure (here); the crypto lives in `mackesd`
// (`vpn_secret`) so this types crate stays dependency-light. Secret material
// never touches `ps`/logs/argv.

/// The cleartext payload sealed into a tunnel's `.age` blob. Exactly one of the
/// two config bodies is populated per the tunnel's [`Method`]; `extra` carries
/// any side files an `.ovpn` references inline-or-not (e.g. an `auth-user-pass`
/// credential file) keyed by basename so the node can lay them down beside the
/// config. Serialized as JSON inside the encrypted envelope — never on disk in
/// the clear, never logged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSecret {
    /// The full `wg-quick`-compatible WireGuard config (`[Interface]` private
    /// key + `[Peer]`). Set for [`Method::Wg`]; empty otherwise.
    #[serde(default)]
    pub wg_conf: String,
    /// The full OpenVPN `.ovpn` body (inline certs/keys, or `--config` lines).
    /// Set for [`Method::Ovpn`]; empty otherwise.
    #[serde(default)]
    pub ovpn_conf: String,
    /// Optional side files keyed by basename (e.g. `auth.txt` for an
    /// `auth-user-pass auth.txt` directive). Written 0600 beside the `.ovpn`.
    #[serde(default)]
    pub extra: std::collections::BTreeMap<String, String>,
}

impl TunnelSecret {
    /// A WireGuard secret from a `wg-quick` config body.
    #[must_use]
    pub fn wireguard(wg_conf: impl Into<String>) -> Self {
        Self {
            wg_conf: wg_conf.into(),
            ..Default::default()
        }
    }

    /// An OpenVPN secret from an `.ovpn` body.
    #[must_use]
    pub fn openvpn(ovpn_conf: impl Into<String>) -> Self {
        Self {
            ovpn_conf: ovpn_conf.into(),
            ..Default::default()
        }
    }

    /// Is this secret populated for the given method? Used to reject an
    /// empty/mismatched payload before sealing (a `Wg` tunnel with no
    /// `wg_conf` would never come up — fail loud at save, not at bring-up).
    #[must_use]
    pub fn is_populated_for(&self, method: Method) -> bool {
        match method {
            Method::Wg => !self.wg_conf.trim().is_empty(),
            Method::Ovpn => !self.ovpn_conf.trim().is_empty(),
            // CLI/API tunnels mint their own config at bring-up; the stored
            // secret carries the provider auth, so either body (or neither,
            // when the auth rides `extra`) is acceptable.
            Method::Cli | Method::Api => true,
        }
    }
}

/// The shared-substrate secret root: `<workgroup_root>/secrets/vpn`. The leader
/// owns this subtree; per-node subdirs hold only that node's assigned `.age`
/// blobs (the leader pushes a tunnel's secret only to its assigned gateways).
#[must_use]
pub fn secret_root(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join("secrets").join("vpn")
}

/// The encrypted blob path for one tunnel assigned to one node:
/// `<workgroup_root>/secrets/vpn/<node_id>/<tunnel_id>.age`. `node_id` is
/// sanitized so a `peer:host` id can't escape the subtree via `/` or `..`.
#[must_use]
pub fn secret_path(
    workgroup_root: &std::path::Path,
    node_id: &str,
    tunnel_id: &str,
) -> std::path::PathBuf {
    secret_root(workgroup_root)
        .join(sanitize_path_segment(node_id))
        .join(format!("{}.age", sanitize_path_segment(tunnel_id)))
}

/// The `creds_ref` token recorded in `tunnels.toml` for a tunnel — a stable,
/// log-safe handle (`secret://vpn/<tunnel_id>`), never the material itself.
#[must_use]
pub fn creds_ref(tunnel_id: &str) -> String {
    format!("secret://vpn/{}", sanitize_path_segment(tunnel_id))
}

/// Where the decrypted WireGuard config is materialized for `wg-quick up`:
/// `/etc/wireguard/<ifname>.conf` (the path VPN-GW-1's bring-up expects).
#[must_use]
pub fn wg_conf_path(t: &TunnelDef) -> std::path::PathBuf {
    std::path::Path::new("/etc/wireguard").join(format!("{}.conf", t.ifname()))
}

/// Where the decrypted `.ovpn` is materialized for `openvpn --config`:
/// `/etc/openvpn/client/<ifname>.ovpn` (the path VPN-GW-1's bring-up expects).
#[must_use]
pub fn ovpn_conf_path(t: &TunnelDef) -> std::path::PathBuf {
    std::path::Path::new("/etc/openvpn/client").join(format!("{}.ovpn", t.ifname()))
}

/// Sanitize one path segment to a safe `[A-Za-z0-9._-]` token: any other char
/// (incl. `/`, `:`) collapses to `_`, and any run of 2+ dots collapses to a
/// single `_` so no `.`/`..` traversal component survives. Keeps a `peer:host`
/// node-id or an operator-typed tunnel-id inside the secret subtree — no path
/// traversal off the shared root, no literal `..` left in a filename. Pure +
/// idempotent on already-clean input.
#[must_use]
fn sanitize_path_segment(s: &str) -> String {
    // First map every disallowed char to `_` (collapses `/`, `:`, etc.).
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Collapse any run of 2+ dots (the `..` / `...` traversal shapes) to a
    // single `_`; a lone `.` between other chars (e.g. a file extension) stays.
    let mut out = String::with_capacity(mapped.len());
    let mut dot_run = 0usize;
    let flush = |out: &mut String, run: usize| {
        if run == 1 {
            out.push('.');
        } else if run >= 2 {
            out.push('_');
        }
    };
    for c in mapped.chars() {
        if c == '.' {
            dot_run += 1;
        } else {
            flush(&mut out, dot_run);
            dot_run = 0;
            out.push(c);
        }
    }
    flush(&mut out, dot_run);
    // A segment that is empty or reduced to a single `.` is unusable as a
    // directory/file name — fall back to a fixed placeholder.
    if out.is_empty() || out == "." {
        "_".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tun(id: &str, method: Method) -> TunnelDef {
        TunnelDef {
            id: id.into(),
            provider: "generic-wg".into(),
            method,
            ..Default::default()
        }
    }

    #[test]
    fn ifname_is_prefixed_sanitized_and_bounded() {
        assert_eq!(tun("mullvad1", Method::Wg).ifname(), "mvpn-mullvad1");
        // Non-alnum collapses.
        assert_eq!(tun("proton-uk_2", Method::Wg).ifname(), "mvpn-protonuk2");
        // Bounded to 15 chars total (10 body chars after the 5-char prefix).
        let long = tun("abcdefghijklmnop", Method::Wg).ifname();
        assert_eq!(long, "mvpn-abcdefghij");
        assert!(long.len() <= IFNAME_MAX);
    }

    #[test]
    fn validate_rejects_empty_and_non_alnum_ids() {
        assert!(tun("", Method::Wg).validate().is_err());
        assert!(tun("___", Method::Wg).validate().is_err()); // ifname body empty
        assert!(tun("ok", Method::Wg).validate().is_ok());
    }

    #[test]
    fn config_round_trips_and_detects_ifname_collision() {
        let mut cfg = VpnConfig::default();
        cfg.upsert(tun("mullvad1", Method::Wg));
        cfg.upsert(tun("mullvad2", Method::Ovpn));
        let s = cfg.to_toml_string().unwrap();
        assert_eq!(VpnConfig::from_toml_str(&s).unwrap(), cfg);
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.tunnel.len(), 2);
        // Two ids sanitizing to the same ifname collide.
        cfg.upsert(tun("mull-vad1", Method::Wg)); // → mvpn-mullvad1, same as "mullvad1"
        assert!(cfg.validate().unwrap_err().contains("collision"));
    }

    #[test]
    fn upsert_replaces_and_remove_works() {
        let mut cfg = VpnConfig::default();
        cfg.upsert(tun("a", Method::Wg));
        let mut updated = tun("a", Method::Ovpn);
        updated.server = "us-nyc".into();
        cfg.upsert(updated);
        assert_eq!(cfg.tunnel.len(), 1);
        assert_eq!(cfg.get("a").unwrap().method, Method::Ovpn);
        assert!(cfg.remove("a"));
        assert!(!cfg.remove("a"));
    }

    #[test]
    fn load_save_round_trip_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = VpnConfig::default();
        cfg.upsert(tun("mullvad1", Method::Wg));
        save(tmp.path(), &cfg).unwrap();
        assert_eq!(load(tmp.path()), cfg);
        // Missing → default empty.
        assert_eq!(
            load(tmp.path().join("nope").as_path()),
            VpnConfig::default()
        );
    }

    #[test]
    fn argv_builders() {
        let t = tun("mullvad1", Method::Wg);
        assert_eq!(
            wg_quick_argv(&t, true),
            vec!["wg-quick", "up", "mvpn-mullvad1"]
        );
        assert_eq!(wg_quick_argv(&t, false)[1], "down");
        assert_eq!(
            openvpn_argv(&t, "/run/mvpn/mullvad1.ovpn"),
            vec![
                "openvpn",
                "--config",
                "/run/mvpn/mullvad1.ovpn",
                "--dev",
                "mvpn-mullvad1",
                "--daemon"
            ]
        );
    }

    // ── VPN-GW-2 — secret payload + path logic ──────────────────────────────

    #[test]
    fn secret_is_populated_per_method() {
        let wg = TunnelSecret::wireguard("[Interface]\nPrivateKey=abc\n");
        assert!(wg.is_populated_for(Method::Wg));
        assert!(!wg.is_populated_for(Method::Ovpn));
        let ov = TunnelSecret::openvpn("client\nremote vpn.example 1194\n");
        assert!(ov.is_populated_for(Method::Ovpn));
        assert!(!ov.is_populated_for(Method::Wg));
        // Whitespace-only body is not populated.
        assert!(!TunnelSecret::wireguard("   \n").is_populated_for(Method::Wg));
        // CLI/API tunnels mint config later → either body is acceptable.
        assert!(TunnelSecret::default().is_populated_for(Method::Cli));
        assert!(TunnelSecret::default().is_populated_for(Method::Api));
    }

    #[test]
    fn secret_path_is_under_node_subtree_and_traversal_safe() {
        let root = std::path::Path::new("/srv/share");
        let p = secret_path(root, "peer:anvil", "mullvad1");
        assert_eq!(
            p,
            std::path::Path::new("/srv/share/secrets/vpn/peer_anvil/mullvad1.age")
        );
        // A malicious id can't escape the node subtree.
        let evil = secret_path(root, "../../etc", "../../../passwd");
        assert!(evil.starts_with("/srv/share/secrets/vpn/"));
        assert!(!evil.to_string_lossy().contains(".."));
        // The secret_root anchors the subtree.
        assert_eq!(
            secret_root(root),
            std::path::Path::new("/srv/share/secrets/vpn")
        );
    }

    #[test]
    fn creds_ref_is_log_safe_and_stable() {
        assert_eq!(creds_ref("mullvad1"), "secret://vpn/mullvad1");
        // No raw material, no traversal.
        let r = creds_ref("../oops");
        assert!(r.starts_with("secret://vpn/"));
        assert!(!r.contains(".."));
    }

    #[test]
    fn materialize_paths_match_bringup_expectations() {
        let t = tun("mullvad1", Method::Wg);
        assert_eq!(
            wg_conf_path(&t),
            std::path::Path::new("/etc/wireguard/mvpn-mullvad1.conf")
        );
        assert_eq!(
            ovpn_conf_path(&t),
            std::path::Path::new("/etc/openvpn/client/mvpn-mullvad1.ovpn")
        );
    }

    #[test]
    fn secret_json_round_trips_through_serde() {
        let mut s = TunnelSecret::openvpn("client\nauth-user-pass auth.txt\n");
        s.extra.insert("auth.txt".into(), "user\npass\n".into());
        let json = serde_json::to_string(&s).unwrap();
        let back: TunnelSecret = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
