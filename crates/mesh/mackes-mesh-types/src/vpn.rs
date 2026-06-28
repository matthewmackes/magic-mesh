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

/// VPN-GW-4 — the scope an egress route applies to (survey Q6: per-node,
/// node-group, or the whole mesh). Stored as a kebab-case string in TOML so a
/// `[[route]]` table round-trips cleanly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteScope {
    /// A single node, keyed by its node id / hostname (`target`).
    #[default]
    Node,
    /// A named node-group (`target` is the group name); every member egresses
    /// through this route unless it has a more-specific node route.
    Group,
    /// The whole mesh — the default egress for any node not otherwise routed
    /// (`target` ignored).
    Any,
}

impl RouteScope {
    /// The specificity rank used to resolve overlapping routes: a node route
    /// (2) beats a group route (1) beats the ANY default (0).
    #[must_use]
    pub const fn specificity(self) -> u8 {
        match self {
            Self::Node => 2,
            Self::Group => 1,
            Self::Any => 0,
        }
    }
}

/// VPN-GW-4 — one egress-routing assignment: a scope (per-node / node-group /
/// ANY) routed through a `gateway` node's primary `tunnel` with an ordered
/// `failover` chain and a per-route kill-switch (survey Q7/Q8). Durable in the
/// node's [`VpnConfig`] on the shared substrate; the `vpn_health` worker
/// (VPN-GW-6) resolves the live tunnel from the chain + the per-tunnel health.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRoute {
    /// What the route applies to.
    #[serde(default)]
    pub scope: RouteScope,
    /// The node id / group name the scope targets (empty for [`RouteScope::Any`]).
    #[serde(default)]
    pub target: String,
    /// The gateway node that runs the tunnel(s) this route egresses through.
    pub gateway: String,
    /// The primary tunnel id (on `gateway`) traffic egresses through.
    pub tunnel: String,
    /// Ordered failover chain (tunnel ids on `gateway`), tried in order after the
    /// primary; an empty chain means "no failover — kill-switch on primary drop".
    #[serde(default)]
    pub failover: Vec<String>,
    /// Block egress (no leak) when no tunnel in the chain is healthy. Default on
    /// (survey Q8: kill-switch is the per-route default; failover is tried first).
    #[serde(default = "default_kill_switch")]
    pub kill_switch: bool,
}

fn default_kill_switch() -> bool {
    true
}

impl EgressRoute {
    /// The ordered candidate tunnel ids: the primary first, then the failover
    /// chain (de-duplicated, so a chain that repeats the primary doesn't probe
    /// it twice). Pure.
    #[must_use]
    pub fn chain(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::with_capacity(1 + self.failover.len());
        let primary = self.tunnel.trim();
        if !primary.is_empty() {
            out.push(primary);
        }
        for t in &self.failover {
            let t = t.trim();
            if !t.is_empty() && !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    /// Resolve the tunnel this route should be egressing through right now:
    /// the first id in [`chain`](Self::chain) for which `is_healthy` holds. `None`
    /// means every candidate is unhealthy — the caller engages the kill-switch
    /// (when [`kill_switch`](Self::kill_switch)) rather than leaking direct. Pure
    /// + the load-bearing failover decision (VPN-GW-4/6), unit-tested.
    #[must_use]
    pub fn resolve<F>(&self, is_healthy: F) -> Option<&str>
    where
        F: Fn(&str) -> bool,
    {
        self.chain().into_iter().find(|id| is_healthy(id))
    }

    /// A stable identity key (scope + target) so an upsert replaces the matching
    /// route rather than appending a duplicate. ANY collapses to a single key.
    #[must_use]
    pub fn key(&self) -> (RouteScope, String) {
        match self.scope {
            RouteScope::Any => (RouteScope::Any, String::new()),
            s => (s, self.target.trim().to_string()),
        }
    }

    /// Validate the route is usable: a non-empty gateway + primary tunnel, and a
    /// target for the node/group scopes (ANY needs none).
    ///
    /// # Errors
    /// A human-readable reason.
    pub fn validate(&self) -> Result<(), String> {
        if self.gateway.trim().is_empty() {
            return Err("egress route gateway is empty".into());
        }
        if self.tunnel.trim().is_empty() {
            return Err("egress route primary tunnel is empty".into());
        }
        if self.scope != RouteScope::Any && self.target.trim().is_empty() {
            return Err(format!(
                "egress route scope {:?} needs a target (node id / group name)",
                self.scope
            ));
        }
        Ok(())
    }
}

/// The node's VPN config — the durable set of tunnel definitions + egress routes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnConfig {
    /// Per-node tunnel definitions.
    #[serde(default)]
    pub tunnel: Vec<TunnelDef>,
    /// VPN-GW-4 — egress-routing assignments (per-node / group / ANY).
    #[serde(default)]
    pub route: Vec<EgressRoute>,
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
    /// sanitize to the same `mvpn-<body>` can't run concurrently) + every egress
    /// route.
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
        let mut route_keys = std::collections::HashSet::new();
        for r in &self.route {
            r.validate()?;
            if !route_keys.insert(r.key()) {
                return Err(format!(
                    "duplicate egress route for scope {:?} target '{}'",
                    r.scope, r.target
                ));
            }
        }
        Ok(())
    }

    /// Insert or replace an egress route, keyed by [`EgressRoute::key`] (so a
    /// re-assign of the same scope/target overwrites rather than duplicates).
    pub fn upsert_route(&mut self, r: EgressRoute) {
        if let Some(e) = self.route.iter_mut().find(|x| x.key() == r.key()) {
            *e = r;
        } else {
            self.route.push(r);
        }
    }

    /// Remove an egress route by scope + target; `true` if one was removed.
    pub fn remove_route(&mut self, scope: RouteScope, target: &str) -> bool {
        let key = match scope {
            RouteScope::Any => (RouteScope::Any, String::new()),
            s => (s, target.trim().to_string()),
        };
        let before = self.route.len();
        self.route.retain(|r| r.key() != key);
        self.route.len() != before
    }

    /// VPN-GW-4 — the egress route governing `node` (a member of `groups`):
    /// **most-specific wins** — an exact node route, else a group route the node
    /// belongs to, else the ANY/all-mesh default, else `None` (egress direct).
    /// On a tie within a specificity tier the first in config order is kept. Pure.
    #[must_use]
    pub fn route_for(&self, node: &str, groups: &[String]) -> Option<&EgressRoute> {
        let node = node.trim();
        // First-on-tie: fold keeping the earlier route when specificity ties, so
        // the result is deterministic in config order (a node in two grouped
        // routes resolves to the first listed).
        self.route
            .iter()
            .filter(|r| match r.scope {
                RouteScope::Node => r.target.trim() == node,
                RouteScope::Group => groups.iter().any(|g| g.trim() == r.target.trim()),
                RouteScope::Any => true,
            })
            .fold(None, |best: Option<&EgressRoute>, r| match best {
                Some(b) if b.scope.specificity() >= r.scope.specificity() => Some(b),
                _ => Some(r),
            })
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

/// VPN-GW-6 — one tunnel's live exit/health state, published by the `vpn_health`
/// worker and consumed by DDNS (`source = "tunnel:<id>"`) + the GUI's
/// "who exits where" summary. Runtime state (JSON, not the durable TOML config).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelExit {
    /// The tunnel id this state is for (`TunnelDef::id`).
    pub id: String,
    /// The `mvpn-<id>` interface name.
    pub ifname: String,
    /// Provider label (mirrors the def, for the UI without a config join).
    #[serde(default)]
    pub provider: String,
    /// The interface is present + the tunnel handshake/liveness probe passed.
    #[serde(default)]
    pub up: bool,
    /// The exit IP was confirmed to differ from the WAN IP (egress really goes
    /// out the tunnel, not direct) — the leak-proof check.
    #[serde(default)]
    pub verified: bool,
    /// A DNS-leak was detected (resolver egresses outside the tunnel).
    #[serde(default)]
    pub dns_leak: bool,
    /// The verified public exit IP (empty when unknown / down).
    #[serde(default)]
    pub exit_ip: String,
    /// When this state was last probed (unix ms).
    #[serde(default)]
    pub checked_ms: u64,
    /// Human-readable last-probe detail (honest reason on a failure).
    #[serde(default)]
    pub detail: String,
}

/// The node's published VPN exit state — one [`TunnelExit`] per probed tunnel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnExitState {
    /// Per-tunnel exit/health.
    #[serde(default)]
    pub tunnel: Vec<TunnelExit>,
}

impl VpnExitState {
    /// Look up a tunnel's exit state by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&TunnelExit> {
        self.tunnel.iter().find(|t| t.id == id)
    }

    /// Insert or replace a tunnel's exit state (keyed by id).
    pub fn upsert(&mut self, e: TunnelExit) {
        if let Some(slot) = self.tunnel.iter_mut().find(|t| t.id == e.id) {
            *slot = e;
        } else {
            self.tunnel.push(e);
        }
    }
}

/// Durable path for the published exit state: `<workgroup_root>/vpn/exit-state.json`.
#[must_use]
pub fn exit_state_path(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join("vpn").join("exit-state.json")
}

/// Load the published exit state (missing/malformed → empty).
#[must_use]
pub fn load_exit_state(workgroup_root: &std::path::Path) -> VpnExitState {
    std::fs::read_to_string(exit_state_path(workgroup_root))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the published exit state (atomic temp+rename).
///
/// # Errors
/// An I/O / serialize error.
pub fn save_exit_state(
    workgroup_root: &std::path::Path,
    state: &VpnExitState,
) -> Result<std::path::PathBuf, String> {
    let path = exit_state_path(workgroup_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
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

    // ── VPN-GW-4: egress routing model + chain resolution ──

    fn route(scope: RouteScope, target: &str, tunnel: &str, failover: &[&str]) -> EgressRoute {
        EgressRoute {
            scope,
            target: target.into(),
            gateway: "eagle".into(),
            tunnel: tunnel.into(),
            failover: failover.iter().map(|s| (*s).to_string()).collect(),
            kill_switch: true,
        }
    }

    #[test]
    fn route_chain_dedups_primary_and_blanks() {
        let r = route(
            RouteScope::Any,
            "",
            "primary",
            &["primary", "backup", "", "  ", "third"],
        );
        assert_eq!(r.chain(), vec!["primary", "backup", "third"]);
    }

    #[test]
    fn route_resolves_first_healthy_in_chain_then_none() {
        let r = route(RouteScope::Node, "pine", "mullvad1", &["proton2", "ivpn3"]);
        // Primary healthy ⇒ primary.
        assert_eq!(r.resolve(|_| true), Some("mullvad1"));
        // Primary down ⇒ first healthy failover.
        assert_eq!(r.resolve(|id| id != "mullvad1"), Some("proton2"));
        // Primary + first failover down ⇒ second failover.
        assert_eq!(
            r.resolve(|id| id == "ivpn3"),
            Some("ivpn3"),
            "fails over down the chain"
        );
        // Nothing healthy ⇒ None (caller engages the kill-switch).
        assert_eq!(r.resolve(|_| false), None);
    }

    #[test]
    fn route_for_is_most_specific_node_then_group_then_any() {
        let mut cfg = VpnConfig::default();
        cfg.upsert_route(route(RouteScope::Any, "", "any-tun", &[]));
        cfg.upsert_route(route(RouteScope::Group, "lab", "grp-tun", &[]));
        cfg.upsert_route(route(RouteScope::Node, "pine", "node-tun", &[]));
        // Node route wins for pine.
        assert_eq!(
            cfg.route_for("pine", &["lab".into()]).unwrap().tunnel,
            "node-tun"
        );
        // A grouped node with no node route → the group route.
        assert_eq!(
            cfg.route_for("oak", &["lab".into()]).unwrap().tunnel,
            "grp-tun"
        );
        // A node in no managed group → the ANY default.
        assert_eq!(cfg.route_for("elm", &[]).unwrap().tunnel, "any-tun");
    }

    #[test]
    fn route_for_none_when_no_route_and_no_any() {
        let mut cfg = VpnConfig::default();
        cfg.upsert_route(route(RouteScope::Node, "pine", "t", &[]));
        assert!(cfg.route_for("oak", &[]).is_none());
    }

    #[test]
    fn route_for_group_tie_keeps_first_in_config_order() {
        let mut cfg = VpnConfig::default();
        cfg.upsert_route(route(RouteScope::Group, "a", "first", &[]));
        cfg.upsert_route(route(RouteScope::Group, "b", "second", &[]));
        // oak is in both groups; the first listed wins deterministically.
        assert_eq!(
            cfg.route_for("oak", &["a".into(), "b".into()])
                .unwrap()
                .tunnel,
            "first"
        );
    }

    #[test]
    fn upsert_route_replaces_and_remove_route_works() {
        let mut cfg = VpnConfig::default();
        cfg.upsert_route(route(RouteScope::Node, "pine", "t1", &[]));
        cfg.upsert_route(route(RouteScope::Node, "pine", "t2", &["t1"])); // same key
        assert_eq!(cfg.route.len(), 1);
        assert_eq!(cfg.route[0].tunnel, "t2");
        assert!(cfg.remove_route(RouteScope::Node, "pine"));
        assert!(!cfg.remove_route(RouteScope::Node, "pine"));
        // ANY collapses to one key regardless of target.
        cfg.upsert_route(route(RouteScope::Any, "ignored", "a", &[]));
        cfg.upsert_route(route(RouteScope::Any, "whatever", "b", &[]));
        assert_eq!(cfg.route.len(), 1);
        assert!(cfg.remove_route(RouteScope::Any, ""));
    }

    #[test]
    fn route_validate_requires_gateway_tunnel_and_target() {
        let mut r = route(RouteScope::Node, "", "t", &[]);
        assert!(r.validate().unwrap_err().contains("needs a target"));
        r.target = "pine".into();
        assert!(r.validate().is_ok());
        r.gateway.clear();
        assert!(r.validate().unwrap_err().contains("gateway"));
        // ANY needs no target.
        let any = route(RouteScope::Any, "", "t", &[]);
        assert!(any.validate().is_ok());
    }

    #[test]
    fn config_validate_rejects_duplicate_route_keys() {
        let mut cfg = VpnConfig::default();
        cfg.route.push(route(RouteScope::Node, "pine", "a", &[]));
        cfg.route.push(route(RouteScope::Node, "pine", "b", &[]));
        assert!(cfg
            .validate()
            .unwrap_err()
            .contains("duplicate egress route"));
    }

    #[test]
    fn config_with_routes_round_trips_toml() {
        let mut cfg = VpnConfig::default();
        cfg.upsert(tun("mullvad1", Method::Wg));
        cfg.upsert_route(route(RouteScope::Node, "pine", "mullvad1", &["proton2"]));
        let s = cfg.to_toml_string().unwrap();
        assert_eq!(VpnConfig::from_toml_str(&s).unwrap(), cfg);
        assert!(cfg.validate().is_ok());
    }

    // ── VPN-GW-6: published exit state round-trip ──

    #[test]
    fn exit_state_round_trips_and_upserts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut st = VpnExitState::default();
        st.upsert(TunnelExit {
            id: "mullvad1".into(),
            ifname: "mvpn-mullvad1".into(),
            provider: "mullvad".into(),
            up: true,
            verified: true,
            exit_ip: "1.2.3.4".into(),
            checked_ms: 42,
            ..Default::default()
        });
        save_exit_state(tmp.path(), &st).unwrap();
        assert_eq!(load_exit_state(tmp.path()), st);
        assert_eq!(st.get("mullvad1").unwrap().exit_ip, "1.2.3.4");
        // Upsert replaces by id.
        st.upsert(TunnelExit {
            id: "mullvad1".into(),
            exit_ip: "5.6.7.8".into(),
            ..Default::default()
        });
        assert_eq!(st.tunnel.len(), 1);
        assert_eq!(st.get("mullvad1").unwrap().exit_ip, "5.6.7.8");
        // Missing → empty.
        assert_eq!(
            load_exit_state(tmp.path().join("nope").as_path()),
            VpnExitState::default()
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
}
