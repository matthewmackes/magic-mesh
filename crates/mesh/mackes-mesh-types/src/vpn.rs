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

/// VPN-GW-3 — base of the firewall-mark / routing-table window the selective
/// egress reserves. Picked high (0x9000_0000) to sit clear of mark 0, the low
/// marks firewalld/Nebula conventionally use, and the reserved route tables
/// `main`/`default`/`local` (253–255). Each tunnel's mark/table is
/// [`EGRESS_MARK_BASE`]`..`[`EGRESS_MARK_BASE`]`+`[`EGRESS_MARK_SPAN`].
pub const EGRESS_MARK_BASE: u32 = 0x9000_0000;

/// VPN-GW-3 — size of the egress mark/table window (derived marks are taken
/// modulo this). Comfortably larger than any realistic per-node tunnel count.
pub const EGRESS_MARK_SPAN: u32 = 0x0001_0000;

/// VPN-GW-3 — `ip rule` priority of the **carve-out** rule that sends overlay
/// (Nebula) traffic to the `main` table *before* the fwmark rule is consulted,
/// so mesh traffic is never tunnelled through the VPN (design risk §"Policy-
/// routing + Nebula interaction"). Lower priority = consulted first.
pub const EGRESS_RULE_PRIO_CARVEOUT: u32 = 9000;

/// VPN-GW-3 — `ip rule` priority of the fwmark→table rule (consulted after the
/// carve-out). Per-tunnel rules share this priority but match distinct marks.
pub const EGRESS_RULE_PRIO_MARK: u32 = 9100;

/// VPN-GW-3 — the Nebula overlay CIDR carved out of VPN egress. Mirrors
/// `mackesd::ca::DEFAULT_MESH_CIDR` (kept as a literal here so this types crate
/// stays dependency-light; the value is locked by the design doc).
pub const MESH_OVERLAY_CIDR: &str = "10.42.0.0/16";

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

/// VPN-GW-3 — the per-tunnel selective-egress policy. Drives whether the
/// `vpn_gateway` worker installs the policy-routing + NAT for `mvpn-<id>` and
/// whether a kill-switch drop rule blocks the tunnel's marked traffic when the
/// interface is down (no plaintext leak).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Steer marked traffic out this tunnel (install the `ip rule`/`ip route`
    /// + NAT). When `false` the tunnel carries no policy-routed egress.
    #[serde(default)]
    pub enabled: bool,
    /// Block this tunnel's marked traffic when the interface is down, so
    /// nothing falls back to the plaintext WAN. The design default for a route
    /// is "block on drop" (Q8); failover is tried first by the route engine.
    #[serde(default)]
    pub kill_switch: bool,
    /// Optional operator-pinned firewall mark / table id. Absent → derived
    /// stably from the tunnel id ([`TunnelDef::egress_mark`]); pin it only to
    /// interoperate with an externally-marked flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<u32>,
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
    /// VPN-GW-3 — selective-egress policy for this tunnel: whether marked
    /// traffic is policy-routed out `mvpn-<id>` (+ NAT) and whether a kill-switch
    /// blocks that marked traffic when the tunnel is down. Default = off, so a
    /// tunnel is a no-op for egress until the operator opts a route in.
    #[serde(default)]
    pub egress: EgressPolicy,
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

    /// VPN-GW-3 — the firewall mark + routing-table id this tunnel's egress is
    /// keyed on. Either the operator-pinned [`EgressPolicy::mark`], or a stable
    /// value **derived from the tunnel id** so two tunnels on one node never
    /// collide and the same tunnel always maps to the same mark across daemon
    /// restarts (idempotent rule construction). The fwmark *is* the table id —
    /// one number selects both the `ip rule` mark match and the per-tunnel route
    /// table, keeping the mapping trivially invertible for teardown.
    ///
    /// The value lands in [`EGRESS_MARK_BASE`]`..`[`EGRESS_MARK_BASE`]`+`[`EGRESS_MARK_SPAN`]
    /// to stay clear of mark 0, of the low marks Nebula/firewalld conventionally
    /// use, and of `main`/`local`/`default` (tables 253–255).
    #[must_use]
    pub fn egress_mark(&self) -> u32 {
        if let Some(m) = self.egress.mark {
            return m;
        }
        // FNV-1a over the (already-bounded, stable) ifname → a span offset.
        let mut h: u32 = 0x811c_9dc5;
        for b in self.ifname().bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
        EGRESS_MARK_BASE + (h % EGRESS_MARK_SPAN)
    }

    /// VPN-GW-3 — the per-tunnel routing-table id. Equal to [`egress_mark`] by
    /// construction (mark→table is the identity here) so a single number drives
    /// both the rule and the table, and teardown can flush exactly what apply
    /// created.
    ///
    /// [`egress_mark`]: Self::egress_mark
    #[must_use]
    pub fn egress_table(&self) -> u32 {
        self.egress_mark()
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

// ── VPN-GW-4 — mesh egress routing (per-node / group / ANY) + failover chain ──
//
// VPN-GW-3 built the per-TUNNEL mechanism (mark→table→NAT→kill-switch). This is
// the ROUTING/assignment engine that *drives* it: an operator assigns a scope
// (one node, a node-group, or the whole mesh) to a gateway node + an ordered
// **tunnel chain** (primary first, then failover fallbacks). On the gateway, the
// `vpn_gateway` worker runs the pure **selector** below against live per-tunnel
// up/down status (VPN-GW-1's `tunnel-status`) to pick the active tunnel — the
// first chain entry that is up — and applies *that* tunnel's egress via GW-3's
// `plan_egress_apply`. On a drop it re-selects (fails over down the chain); when
// nothing is up the kill-switch flag decides leak-vs-block.
//
// Everything here is dep-free + pure: the model round-trips through TOML, and the
// selector/resolver are unit-tested without a live mesh. The cross-node, real-
// multi-provider live failover verification (and the silent-leak / exit-IP probe
// that *detects* a down tunnel beyond the interface check) is VPN-GW-6.

/// VPN-GW-4 — the scope an [`EgressRoute`] assigns. Precedence on a given node is
/// **`Node` > `Group` > `AnyMesh`** (the most specific assignment wins), resolved
/// by [`resolve_route`]. The `#[default]` is `AnyMesh` — a route with no explicit
/// scope is the mesh-wide default egress.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RouteScope {
    /// Exactly one node, keyed by its node id (`peer:host`).
    Node {
        /// The assigned node's id.
        id: String,
    },
    /// A named node-group; a node matches when the group is in its membership
    /// set (resolved at the worker from the mesh's group config — kept out of
    /// this dep-free model).
    Group {
        /// The group name.
        name: String,
    },
    /// The whole mesh (the default egress for any node not covered by a more
    /// specific `Node`/`Group` route).
    #[default]
    AnyMesh,
}

impl RouteScope {
    /// Specificity rank for precedence: `Node`(2) > `Group`(1) > `AnyMesh`(0).
    /// A higher rank wins in [`resolve_route`].
    #[must_use]
    pub fn specificity(&self) -> u8 {
        match self {
            RouteScope::Node { .. } => 2,
            RouteScope::Group { .. } => 1,
            RouteScope::AnyMesh => 0,
        }
    }

    /// Does this scope apply to the node `node_id` whose group memberships are
    /// `groups`? Pure — the worker supplies the live identity + membership.
    #[must_use]
    pub fn matches(&self, node_id: &str, groups: &[String]) -> bool {
        match self {
            RouteScope::Node { id } => id == node_id,
            RouteScope::Group { name } => groups.iter().any(|g| g == name),
            RouteScope::AnyMesh => true,
        }
    }
}

/// VPN-GW-4 — one egress-route assignment: a [`RouteScope`] → a **gateway node**
/// + an **ordered tunnel chain** (primary first, failover fallbacks after) + the
/// kill-switch flag (block-on-all-down vs. leak). The chain entries are tunnel
/// ids on the gateway (the gateway's `tunnels.toml` defines them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRoute {
    /// What this route covers (node / group / whole mesh).
    #[serde(default)]
    pub scope: RouteScope,
    /// The gateway node id (`peer:host`) whose tunnels carry the egress. The
    /// assigned node's traffic exits via this node's selected tunnel.
    #[serde(default)]
    pub gateway: String,
    /// Ordered tunnel-id chain: index 0 is the primary, the rest are failover
    /// fallbacks tried in order. The first entry whose tunnel is **up** is the
    /// active tunnel ([`select_active`]).
    #[serde(default)]
    pub chain: Vec<String>,
    /// Block egress when every chain tunnel is down (no plaintext leak). The
    /// design default is "block on drop" (Q8); failover is tried first.
    #[serde(default = "default_true")]
    pub kill_switch: bool,
}

/// serde default for [`EgressRoute::kill_switch`] — the design's block-on-drop
/// default (Q8). A route omitting the flag still blocks rather than leaks.
fn default_true() -> bool {
    true
}

impl Default for EgressRoute {
    /// The block-on-drop default (`kill_switch = true`), consistent with the
    /// serde default so a TOML-omitted flag and `EgressRoute::default()` agree.
    fn default() -> Self {
        Self {
            scope: RouteScope::default(),
            gateway: String::new(),
            chain: Vec::new(),
            kill_switch: true,
        }
    }
}

impl EgressRoute {
    /// Validate the route is usable: a non-empty gateway, a non-empty chain
    /// (a route with no tunnels can never carry egress), and — for a `Node`/
    /// `Group` scope — a non-empty key. Pure.
    ///
    /// # Errors
    /// A human-readable reason.
    pub fn validate(&self) -> Result<(), String> {
        match &self.scope {
            RouteScope::Node { id } if id.trim().is_empty() => {
                return Err("route scope Node has an empty id".into());
            }
            RouteScope::Group { name } if name.trim().is_empty() => {
                return Err("route scope Group has an empty name".into());
            }
            _ => {}
        }
        if self.gateway.trim().is_empty() {
            return Err("route gateway is empty".into());
        }
        if self.chain.is_empty() {
            return Err("route tunnel chain is empty".into());
        }
        if self.chain.iter().any(|t| t.trim().is_empty()) {
            return Err("route tunnel chain has an empty entry".into());
        }
        Ok(())
    }

    /// A stable key for upsert/remove keyed on the scope (one assignment per
    /// scope — re-assigning a node/group/ANY replaces its route).
    #[must_use]
    pub fn scope_key(&self) -> String {
        match &self.scope {
            RouteScope::Node { id } => format!("node:{id}"),
            RouteScope::Group { name } => format!("group:{name}"),
            RouteScope::AnyMesh => "any".to_string(),
        }
    }
}

/// VPN-GW-4 — the outcome of running the failover selector over a route's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveTunnel {
    /// The first up tunnel in the chain (the one to apply egress for).
    Up {
        /// The selected tunnel id.
        tunnel_id: String,
    },
    /// Every chain tunnel is down. `kill_switch` carries the route's flag so the
    /// worker knows whether to BLOCK (no leak) or fall through to the WAN.
    AllDown {
        /// The route's kill-switch flag at the time of selection.
        kill_switch: bool,
    },
}

impl ActiveTunnel {
    /// The selected tunnel id, if any is up.
    #[must_use]
    pub fn tunnel_id(&self) -> Option<&str> {
        match self {
            ActiveTunnel::Up { tunnel_id } => Some(tunnel_id),
            ActiveTunnel::AllDown { .. } => None,
        }
    }
}

/// VPN-GW-4 — the PURE failover selector. Given a route's ordered chain and a
/// predicate reporting whether a tunnel id is currently **up** (fed by VPN-GW-1's
/// `tunnel-status` in the worker), pick the active tunnel = the **first chain
/// entry that is up**. When none is up, return [`ActiveTunnel::AllDown`] carrying
/// the route's kill-switch flag.
///
/// This is the whole failover policy: primary-up → primary; primary-down → the
/// next up entry; all-down → none (kill-switch decides leak-vs-block). No I/O.
#[must_use]
pub fn select_active<F>(route: &EgressRoute, is_up: F) -> ActiveTunnel
where
    F: Fn(&str) -> bool,
{
    for tunnel_id in &route.chain {
        if is_up(tunnel_id.as_str()) {
            return ActiveTunnel::Up {
                tunnel_id: tunnel_id.clone(),
            };
        }
    }
    ActiveTunnel::AllDown {
        kill_switch: route.kill_switch,
    }
}

/// VPN-GW-4 — "should this route fail over?": the currently-active tunnel
/// (`current`, e.g. the one egress is applied for) is no longer the selector's
/// choice. Pure helper over [`select_active`] for the worker's switch decision.
#[must_use]
pub fn should_fail_over<F>(route: &EgressRoute, current: &str, is_up: F) -> bool
where
    F: Fn(&str) -> bool,
{
    match select_active(route, is_up).tunnel_id() {
        Some(active) => active != current,
        // All down → there's no active tunnel; the worker tears the current one
        // down (kill-switch / leak per the flag), which is itself a change.
        None => true,
    }
}

/// VPN-GW-4 — the durable set of egress-route assignments (TOML on the shared
/// substrate, leader-edited, every gateway reads it). One assignment per scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteConfig {
    /// The egress-route assignments.
    #[serde(default)]
    pub route: Vec<EgressRoute>,
}

impl RouteConfig {
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

    /// Insert or replace a route (keyed by its [`EgressRoute::scope_key`] — one
    /// assignment per scope, so re-assigning a node/group/ANY replaces it).
    pub fn upsert(&mut self, r: EgressRoute) {
        let key = r.scope_key();
        if let Some(e) = self.route.iter_mut().find(|x| x.scope_key() == key) {
            *e = r;
        } else {
            self.route.push(r);
        }
    }

    /// Remove the route for a scope key; `true` if one was removed.
    pub fn remove(&mut self, scope_key: &str) -> bool {
        let before = self.route.len();
        self.route.retain(|r| r.scope_key() != scope_key);
        self.route.len() != before
    }

    /// Validate every route + that no two share a scope key (one assignment per
    /// scope).
    ///
    /// # Errors
    /// The first inconsistency's reason.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for r in &self.route {
            r.validate()?;
            let key = r.scope_key();
            if !seen.insert(key.clone()) {
                return Err(format!("duplicate route for scope: {key}"));
            }
        }
        Ok(())
    }
}

/// VPN-GW-4 — resolve **which route applies** to the node `node_id` (with group
/// memberships `groups`) by **scope precedence**: among the routes whose scope
/// matches, the most specific wins — `Node` > `Group` > `AnyMesh`
/// ([`RouteScope::specificity`]). Pure; the worker supplies the live identity +
/// membership. Returns `None` when no route covers the node.
#[must_use]
pub fn resolve_route<'a>(
    cfg: &'a RouteConfig,
    node_id: &str,
    groups: &[String],
) -> Option<&'a EgressRoute> {
    cfg.route
        .iter()
        .filter(|r| r.scope.matches(node_id, groups))
        .max_by_key(|r| r.scope.specificity())
}

/// Durable path for the egress-route config: `<workgroup_root>/vpn/routes.toml`
/// (beside `tunnels.toml`).
#[must_use]
pub fn routes_path(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join("vpn").join("routes.toml")
}

/// Load the egress-route config (missing/malformed → default empty).
#[must_use]
pub fn load_routes(workgroup_root: &std::path::Path) -> RouteConfig {
    std::fs::read_to_string(routes_path(workgroup_root))
        .ok()
        .and_then(|raw| RouteConfig::from_toml_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist the egress-route config (validate → atomic temp+rename).
///
/// # Errors
/// Validation failure, or an I/O / serialize error.
pub fn save_routes(
    workgroup_root: &std::path::Path,
    cfg: &RouteConfig,
) -> Result<std::path::PathBuf, String> {
    cfg.validate()?;
    let path = routes_path(workgroup_root);
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

// ── VPN-GW-3 — selective egress: policy-routing + NAT + kill-switch ──────────
//
// A gateway routes CHOSEN traffic (tagged with the tunnel's `fwmark`) out the
// tunnel interface, while its own + Nebula traffic stays direct. The mechanism,
// per `mvpn-<id>` tunnel with egress enabled:
//
//   1. policy routing  — an `ip rule` matching the fwmark selects a dedicated
//      per-tunnel route table whose default route is `dev mvpn-<id>`; a higher-
//      priority carve-out rule sends the Nebula overlay CIDR to `main` first so
//      mesh traffic NEVER tunnels through the VPN (design risk §6).
//   2. NAT            — an nftables `masquerade` on packets leaving `mvpn-<id>`
//      so the marked traffic exits as the provider's IP.
//   3. kill-switch    — an nftables `drop` on the *marked* traffic, so when the
//      tunnel is down the marked packets are dropped instead of leaking out the
//      plaintext WAN. Installed only when `egress.kill_switch` is set.
//
// Everything below is a PURE argv/ruleset builder + an idempotent apply/teardown
// PLAN (a `Vec<EgressCmd>`); the `vpn_gateway` worker executes the plan with its
// timeout-bounded proc helpers and degrades gracefully when `ip`/`nft` are
// absent. The nftables family/table/chain names the kill-switch + NAT live in.

/// The nftables table the selective-egress NAT + kill-switch rules live in.
/// One `inet` table for the whole feature; per-tunnel rules are distinguished
/// by their fwmark / oif comment, so teardown can target one tunnel without
/// disturbing another.
pub const EGRESS_NFT_TABLE: &str = "mvpn_egress";

/// The nftables chain holding the per-tunnel `masquerade` rules (postrouting).
pub const EGRESS_NFT_NAT_CHAIN: &str = "postrouting";

/// The nftables chain holding the per-tunnel kill-switch `drop` rules
/// (output — drop marked traffic that would otherwise leak when the tunnel is
/// down). A `forward`-side companion covers routed (gateway'd) marked traffic.
pub const EGRESS_NFT_KILL_CHAIN: &str = "killswitch";

/// One command in an egress apply/teardown plan: the program (`ip` or `nft`)
/// plus its argv. Kept as a typed pair (rather than a bare `Vec<String>`) so the
/// worker can pick the right binary-presence check + so a plan reads honestly in
/// a log/test. Pure data — building one performs no I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressCmd {
    /// The program to run (`"ip"` or `"nft"`).
    pub prog: &'static str,
    /// Its arguments (the program name is NOT repeated here).
    pub args: Vec<String>,
}

impl EgressCmd {
    fn ip(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            prog: "ip",
            args: args.into_iter().map(Into::into).collect(),
        }
    }
    fn nft(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            prog: "nft",
            args: args.into_iter().map(Into::into).collect(),
        }
    }
    /// The full argv (program + args), e.g. for spawning or logging.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        let mut v = Vec::with_capacity(self.args.len() + 1);
        v.push(self.prog.to_string());
        v.extend(self.args.iter().cloned());
        v
    }
}

/// `ip rule add fwmark <mark> table <table> priority <prio>` — steer marked
/// traffic to the tunnel's table.
#[must_use]
pub fn ip_rule_add_argv(mark: u32, table: u32, prio: u32) -> EgressCmd {
    EgressCmd::ip([
        "rule".to_string(),
        "add".to_string(),
        "fwmark".to_string(),
        mark.to_string(),
        "table".to_string(),
        table.to_string(),
        "priority".to_string(),
        prio.to_string(),
    ])
}

/// `ip rule del fwmark <mark> table <table> priority <prio>` — the exact inverse
/// of [`ip_rule_add_argv`], so teardown removes precisely what apply added.
#[must_use]
pub fn ip_rule_del_argv(mark: u32, table: u32, prio: u32) -> EgressCmd {
    let mut c = ip_rule_add_argv(mark, table, prio);
    c.args[1] = "del".to_string();
    c
}

/// `ip rule add to <cidr> table main priority <prio>` — the carve-out that keeps
/// the Nebula overlay on the `main` table (direct), consulted before the fwmark
/// rule. This is what guarantees mesh traffic never tunnels through the VPN.
#[must_use]
pub fn ip_rule_carveout_add_argv(cidr: &str, prio: u32) -> EgressCmd {
    EgressCmd::ip([
        "rule".to_string(),
        "add".to_string(),
        "to".to_string(),
        cidr.to_string(),
        "table".to_string(),
        "main".to_string(),
        "priority".to_string(),
        prio.to_string(),
    ])
}

/// The inverse of [`ip_rule_carveout_add_argv`].
#[must_use]
pub fn ip_rule_carveout_del_argv(cidr: &str, prio: u32) -> EgressCmd {
    let mut c = ip_rule_carveout_add_argv(cidr, prio);
    c.args[1] = "del".to_string();
    c
}

/// `ip route replace default dev <ifname> table <table>` — the per-tunnel
/// table's default route out the tunnel. `replace` (not `add`) makes re-apply
/// idempotent: it installs-or-updates without erroring on an existing route.
#[must_use]
pub fn ip_route_default_argv(ifname: &str, table: u32) -> EgressCmd {
    EgressCmd::ip([
        "route".to_string(),
        "replace".to_string(),
        "default".to_string(),
        "dev".to_string(),
        ifname.to_string(),
        "table".to_string(),
        table.to_string(),
    ])
}

/// `ip route flush table <table>` — tear down the per-tunnel table wholesale.
#[must_use]
pub fn ip_route_flush_table_argv(table: u32) -> EgressCmd {
    EgressCmd::ip([
        "route".to_string(),
        "flush".to_string(),
        "table".to_string(),
        table.to_string(),
    ])
}

/// `nft add masquerade` for traffic leaving the tunnel interface (the NAT that
/// makes marked traffic exit as the provider's IP). `oifname "<ifname>"` scopes
/// it to this tunnel so teardown of one tunnel leaves others untouched.
#[must_use]
pub fn nft_masquerade_add_argv(ifname: &str) -> EgressCmd {
    EgressCmd::nft([
        "add".to_string(),
        "rule".to_string(),
        "inet".to_string(),
        EGRESS_NFT_TABLE.to_string(),
        EGRESS_NFT_NAT_CHAIN.to_string(),
        "oifname".to_string(),
        format!("\"{ifname}\""),
        "masquerade".to_string(),
    ])
}

/// `nft add` the kill-switch DROP for this tunnel's marked traffic. When the
/// interface is down the marked packets hit this and are dropped instead of
/// falling through to the plaintext WAN. `chain` is one of [`EGRESS_NFT_KILL_CHAIN`]
/// (locally-originated `output`) — the worker installs it on both the output
/// and forward hooks so gateway'd traffic is covered too.
#[must_use]
pub fn nft_killswitch_add_argv(mark: u32, ifname: &str, chain: &str) -> EgressCmd {
    EgressCmd::nft([
        "add".to_string(),
        "rule".to_string(),
        "inet".to_string(),
        EGRESS_NFT_TABLE.to_string(),
        chain.to_string(),
        "meta".to_string(),
        "mark".to_string(),
        mark.to_string(),
        "oifname".to_string(),
        format!("!= \"{ifname}\""),
        "drop".to_string(),
    ])
}

/// Build the **idempotent apply plan** for one tunnel's selective egress:
/// carve-out rule, fwmark rule, per-table default route, NAT masquerade, and
/// (when `kill_switch`) the kill-switch drop rules. Returns an empty plan when
/// the tunnel's egress is not enabled. Pure — no I/O, no system tools.
///
/// Order matters: routing rules first (so the kernel can steer), then NAT, then
/// the kill-switch last (so a partial apply never leaves a permissive gap before
/// the drop is in place). The nftables table/chains are created by
/// [`egress_nft_table_setup_argv`] (idempotent), which a caller prepends once.
#[must_use]
pub fn plan_egress_apply(t: &TunnelDef) -> Vec<EgressCmd> {
    if !t.egress.enabled {
        return Vec::new();
    }
    let mark = t.egress_mark();
    let table = t.egress_table();
    let ifn = t.ifname();
    let mut plan = vec![
        // 1. carve the overlay out to main first (mesh never tunnels).
        ip_rule_carveout_add_argv(MESH_OVERLAY_CIDR, EGRESS_RULE_PRIO_CARVEOUT),
        // 2. fwmark → per-tunnel table.
        ip_rule_add_argv(mark, table, EGRESS_RULE_PRIO_MARK),
        // 3. that table's default route out the tunnel.
        ip_route_default_argv(&ifn, table),
        // 4. NAT marked traffic out the tunnel.
        nft_masquerade_add_argv(&ifn),
    ];
    if t.egress.kill_switch {
        // 5. drop marked traffic NOT leaving the tunnel (output + forward).
        plan.push(nft_killswitch_add_argv(mark, &ifn, EGRESS_NFT_KILL_CHAIN));
    }
    plan
}

/// Build the **teardown plan** for one tunnel's selective egress — the inverse
/// of [`plan_egress_apply`], in reverse order (kill-switch first so the drop is
/// the last thing removed; then NAT; then the routes/rules). The carve-out rule
/// is shared across tunnels, so teardown of a single tunnel does NOT remove it
/// (use [`plan_egress_carveout_teardown`] when the last tunnel goes away).
///
/// nftables rules are removed by flushing this tunnel's contribution: since rules
/// are scoped by `oifname`/`mark` and nft lacks a stable handle here, the worker
/// re-derives the table from scratch on re-apply, and teardown flushes the whole
/// per-feature nft table only when no egress tunnels remain. Per-tunnel teardown
/// therefore removes the routing entries (precise) and leaves the nft rules to
/// the table-level flush; the routing removal alone already breaks the path.
#[must_use]
pub fn plan_egress_teardown(t: &TunnelDef) -> Vec<EgressCmd> {
    let mark = t.egress_mark();
    let table = t.egress_table();
    vec![
        ip_rule_del_argv(mark, table, EGRESS_RULE_PRIO_MARK),
        ip_route_flush_table_argv(table),
    ]
}

/// The idempotent nftables scaffolding the egress NAT + kill-switch live in:
/// create the `inet` table and its postrouting/output/forward base chains.
/// `nft add table`/`add chain` are no-ops if the object exists, so this is safe
/// to run on every apply. Returned as its own plan so a caller runs it once
/// before any per-tunnel apply.
#[must_use]
pub fn egress_nft_table_setup_argv() -> Vec<EgressCmd> {
    vec![
        EgressCmd::nft(["add", "table", "inet", EGRESS_NFT_TABLE]),
        // postrouting (NAT) — priority srcnat so masquerade runs after routing.
        EgressCmd::nft([
            "add",
            "chain",
            "inet",
            EGRESS_NFT_TABLE,
            EGRESS_NFT_NAT_CHAIN,
            "{ type nat hook postrouting priority srcnat ; }",
        ]),
        // killswitch — a plain (non-base) chain we hang drop rules in; the
        // output + forward base chains jump to it.
        EgressCmd::nft([
            "add",
            "chain",
            "inet",
            EGRESS_NFT_TABLE,
            EGRESS_NFT_KILL_CHAIN,
        ]),
    ]
}

/// `nft delete table inet <table>` — wholesale teardown of the egress NAT +
/// kill-switch (run when no tunnel has egress enabled any more). `delete table`
/// errors if absent, so the worker treats a failure here as benign.
#[must_use]
pub fn egress_nft_table_teardown_argv() -> EgressCmd {
    EgressCmd::nft(["delete", "table", "inet", EGRESS_NFT_TABLE])
}

/// Remove the shared overlay carve-out rule (run only when the last egress
/// tunnel is torn down — see [`plan_egress_teardown`]).
#[must_use]
pub fn plan_egress_carveout_teardown() -> EgressCmd {
    ip_rule_carveout_del_argv(MESH_OVERLAY_CIDR, EGRESS_RULE_PRIO_CARVEOUT)
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

    // ── VPN-GW-3 — selective-egress rule construction ───────────────────────

    fn egress_tun(id: &str, enabled: bool, kill_switch: bool) -> TunnelDef {
        TunnelDef {
            egress: EgressPolicy {
                enabled,
                kill_switch,
                mark: None,
            },
            ..tun(id, Method::Wg)
        }
    }

    #[test]
    fn egress_mark_is_stable_distinct_and_in_window() {
        let a = egress_tun("mullvad1", true, false);
        let b = egress_tun("mullvad2", true, false);
        // Stable across calls (idempotent rule construction depends on this).
        assert_eq!(a.egress_mark(), a.egress_mark());
        // Distinct tunnels → distinct marks (no two tunnels collide on a table).
        assert_ne!(a.egress_mark(), b.egress_mark());
        // mark == table (the identity mapping) so teardown is invertible.
        assert_eq!(a.egress_mark(), a.egress_table());
        // In the reserved window, clear of mark 0 and the reserved tables.
        for t in [&a, &b] {
            let m = t.egress_mark();
            assert!((EGRESS_MARK_BASE..EGRESS_MARK_BASE + EGRESS_MARK_SPAN).contains(&m));
            assert!(m > 255, "must clear main/default/local table ids");
        }
    }

    #[test]
    fn egress_mark_honors_an_operator_pin() {
        let mut t = egress_tun("pinned", true, false);
        t.egress.mark = Some(4242);
        assert_eq!(t.egress_mark(), 4242);
        assert_eq!(t.egress_table(), 4242);
    }

    #[test]
    fn disabled_egress_plans_nothing() {
        let t = egress_tun("off", false, true); // kill_switch set but not enabled
        assert!(plan_egress_apply(&t).is_empty());
    }

    #[test]
    fn apply_plan_routes_nats_and_carves_out_the_overlay() {
        let t = egress_tun("mullvad1", true, false); // no kill-switch
        let plan = plan_egress_apply(&t);
        // carve-out, fwmark rule, default route, masquerade — no kill-switch.
        assert_eq!(plan.len(), 4);
        let mark = t.egress_mark();
        let table = t.egress_table();
        assert_eq!(
            plan[0],
            ip_rule_carveout_add_argv(MESH_OVERLAY_CIDR, EGRESS_RULE_PRIO_CARVEOUT)
        );
        assert_eq!(
            plan[1],
            ip_rule_add_argv(mark, table, EGRESS_RULE_PRIO_MARK)
        );
        assert_eq!(plan[2], ip_route_default_argv("mvpn-mullvad1", table));
        assert_eq!(plan[3], nft_masquerade_add_argv("mvpn-mullvad1"));
        // The carve-out targets the Nebula overlay on the main table.
        let co = plan[0].argv();
        assert!(co.contains(&MESH_OVERLAY_CIDR.to_string()));
        assert!(co.contains(&"main".to_string()));
    }

    #[test]
    fn kill_switch_appends_a_drop_for_marked_traffic() {
        let t = egress_tun("ks", true, true);
        let plan = plan_egress_apply(&t);
        assert_eq!(plan.len(), 5, "kill-switch adds one rule");
        let last = plan.last().unwrap();
        assert_eq!(last.prog, "nft");
        let joined = last.argv().join(" ");
        // Drops MARKED traffic that is NOT leaving the tunnel → no plaintext leak.
        assert!(joined.contains(&t.egress_mark().to_string()));
        assert!(joined.contains("!= \"mvpn-ks\""));
        assert!(joined.contains("drop"));
    }

    #[test]
    fn teardown_inverts_the_routing_entries() {
        let t = egress_tun("mullvad1", true, true);
        let mark = t.egress_mark();
        let table = t.egress_table();
        let down = plan_egress_teardown(&t);
        assert_eq!(
            down,
            vec![
                ip_rule_del_argv(mark, table, EGRESS_RULE_PRIO_MARK),
                ip_route_flush_table_argv(table),
            ]
        );
        // The del rule is the add rule with "add"→"del".
        let add = ip_rule_add_argv(mark, table, EGRESS_RULE_PRIO_MARK);
        let del = ip_rule_del_argv(mark, table, EGRESS_RULE_PRIO_MARK);
        assert_eq!(del.args[0], "rule");
        assert_eq!(del.args[1], "del");
        assert_eq!(add.args[2..], del.args[2..]); // same selector, opposite verb
    }

    #[test]
    fn route_default_uses_replace_for_idempotent_reapply() {
        // `ip route replace` installs-or-updates without erroring → re-running
        // the apply plan is a safe no-op.
        let c = ip_route_default_argv("mvpn-x", 1234);
        assert_eq!(c.prog, "ip");
        assert_eq!(c.args[0], "route");
        assert_eq!(c.args[1], "replace");
    }

    #[test]
    fn nft_scaffold_is_idempotent_add_and_full_teardown() {
        let setup = egress_nft_table_setup_argv();
        // table + nat chain + killswitch chain, all `nft add` (idempotent).
        assert_eq!(setup.len(), 3);
        for c in &setup {
            assert_eq!(c.prog, "nft");
            assert_eq!(c.args[0], "add");
            assert!(c.args.contains(&EGRESS_NFT_TABLE.to_string()));
        }
        let down = egress_nft_table_teardown_argv();
        assert_eq!(
            down.argv(),
            vec!["nft", "delete", "table", "inet", EGRESS_NFT_TABLE]
        );
    }

    #[test]
    fn egress_policy_round_trips_through_toml() {
        let mut cfg = VpnConfig::default();
        cfg.upsert(egress_tun("mullvad1", true, true));
        let s = cfg.to_toml_string().unwrap();
        assert_eq!(VpnConfig::from_toml_str(&s).unwrap(), cfg);
        // A tunnel with default (off) egress omits the pinned mark.
        let plain = cfg.to_toml_string().unwrap();
        assert!(!plain.contains("mark ="), "unpinned mark is not serialized");
    }

    // ── VPN-GW-4 — route model + failover selector + scope precedence ────────

    fn route(scope: RouteScope, gateway: &str, chain: &[&str], ks: bool) -> EgressRoute {
        EgressRoute {
            scope,
            gateway: gateway.into(),
            chain: chain.iter().map(|s| (*s).to_string()).collect(),
            kill_switch: ks,
        }
    }

    #[test]
    fn route_round_trips_through_toml_with_all_three_scopes() {
        let mut cfg = RouteConfig::default();
        cfg.upsert(route(
            RouteScope::Node {
                id: "peer:anvil".into(),
            },
            "peer:gw",
            &["mullvad1", "proton2"],
            true,
        ));
        cfg.upsert(route(
            RouteScope::Group { name: "lab".into() },
            "peer:gw",
            &["proton2"],
            false,
        ));
        cfg.upsert(route(RouteScope::AnyMesh, "peer:gw", &["mullvad1"], true));
        let s = cfg.to_toml_string().unwrap();
        assert_eq!(RouteConfig::from_toml_str(&s).unwrap(), cfg);
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.route.len(), 3);
    }

    #[test]
    fn route_validate_rejects_empty_gateway_chain_and_scope_key() {
        // Empty chain.
        assert!(route(RouteScope::AnyMesh, "peer:gw", &[], true)
            .validate()
            .is_err());
        // Empty gateway.
        assert!(route(RouteScope::AnyMesh, "", &["t"], true)
            .validate()
            .is_err());
        // Empty Node id / Group name.
        assert!(
            route(RouteScope::Node { id: " ".into() }, "peer:gw", &["t"], true)
                .validate()
                .is_err()
        );
        assert!(route(
            RouteScope::Group { name: "".into() },
            "peer:gw",
            &["t"],
            true
        )
        .validate()
        .is_err());
        // A whitespace chain entry.
        assert!(route(RouteScope::AnyMesh, "peer:gw", &["ok", "  "], true)
            .validate()
            .is_err());
        // A well-formed route validates.
        assert!(route(RouteScope::AnyMesh, "peer:gw", &["t"], true)
            .validate()
            .is_ok());
    }

    #[test]
    fn upsert_replaces_per_scope_and_remove_works() {
        let mut cfg = RouteConfig::default();
        cfg.upsert(route(RouteScope::AnyMesh, "peer:gw", &["a"], true));
        // Same scope → replaced, not appended.
        cfg.upsert(route(RouteScope::AnyMesh, "peer:gw2", &["b"], false));
        assert_eq!(cfg.route.len(), 1);
        assert_eq!(cfg.route[0].gateway, "peer:gw2");
        assert!(!cfg.route[0].kill_switch);
        assert!(cfg.remove("any"));
        assert!(!cfg.remove("any"));
    }

    #[test]
    fn validate_rejects_duplicate_scope_keys() {
        // Two routes that share a scope key can't both be active (constructed
        // directly to bypass upsert's de-dup).
        let cfg = RouteConfig {
            route: vec![
                route(
                    RouteScope::Node {
                        id: "peer:a".into(),
                    },
                    "g1",
                    &["t"],
                    true,
                ),
                route(
                    RouteScope::Node {
                        id: "peer:a".into(),
                    },
                    "g2",
                    &["t"],
                    true,
                ),
            ],
        };
        assert!(cfg.validate().unwrap_err().contains("duplicate route"));
    }

    #[test]
    fn selector_primary_up_picks_primary() {
        let r = route(RouteScope::AnyMesh, "g", &["primary", "fallback"], true);
        // Everything up → the primary (index 0) wins.
        let active = select_active(&r, |_| true);
        assert_eq!(active.tunnel_id(), Some("primary"));
    }

    #[test]
    fn selector_primary_down_picks_next_up_in_chain() {
        let r = route(
            RouteScope::AnyMesh,
            "g",
            &["primary", "second", "third"],
            true,
        );
        // primary down, second up → second; primary down, second down → third.
        assert_eq!(
            select_active(&r, |t| t != "primary").tunnel_id(),
            Some("second")
        );
        assert_eq!(
            select_active(&r, |t| t == "third").tunnel_id(),
            Some("third")
        );
    }

    #[test]
    fn selector_all_down_yields_none_carrying_killswitch() {
        let blocked = route(RouteScope::AnyMesh, "g", &["a", "b"], true);
        match select_active(&blocked, |_| false) {
            ActiveTunnel::AllDown { kill_switch } => assert!(kill_switch),
            other => panic!("expected AllDown, got {other:?}"),
        }
        assert_eq!(select_active(&blocked, |_| false).tunnel_id(), None);
        // kill_switch=false is carried through so the worker can leak-vs-block.
        let leaky = route(RouteScope::AnyMesh, "g", &["a"], false);
        assert_eq!(
            select_active(&leaky, |_| false),
            ActiveTunnel::AllDown { kill_switch: false }
        );
    }

    #[test]
    fn should_fail_over_when_current_is_no_longer_the_pick() {
        let r = route(RouteScope::AnyMesh, "g", &["primary", "fallback"], true);
        // Current = primary, primary up → no failover.
        assert!(!should_fail_over(&r, "primary", |_| true));
        // Current = primary, primary down but fallback up → fail over.
        assert!(should_fail_over(&r, "primary", |t| t == "fallback"));
        // Current = primary, all down → switch (tear down).
        assert!(should_fail_over(&r, "primary", |_| false));
        // Current = fallback while primary is back up → switch back to primary.
        assert!(should_fail_over(&r, "fallback", |_| true));
    }

    #[test]
    fn resolve_precedence_node_beats_group_beats_anymesh() {
        let mut cfg = RouteConfig::default();
        cfg.upsert(route(RouteScope::AnyMesh, "gw-any", &["any"], true));
        cfg.upsert(route(
            RouteScope::Group { name: "lab".into() },
            "gw-grp",
            &["grp"],
            true,
        ));
        cfg.upsert(route(
            RouteScope::Node {
                id: "peer:anvil".into(),
            },
            "gw-node",
            &["node"],
            true,
        ));
        let groups = vec!["lab".to_string()];
        // The node has all three matching → Node wins.
        let r = resolve_route(&cfg, "peer:anvil", &groups).unwrap();
        assert_eq!(r.gateway, "gw-node");
        // A different node in the group → Group beats AnyMesh.
        let r = resolve_route(&cfg, "peer:other", &groups).unwrap();
        assert_eq!(r.gateway, "gw-grp");
        // A node outside the group → only AnyMesh matches.
        let r = resolve_route(&cfg, "peer:other", &[]).unwrap();
        assert_eq!(r.gateway, "gw-any");
    }

    #[test]
    fn resolve_returns_none_when_no_route_covers_the_node() {
        let mut cfg = RouteConfig::default();
        cfg.upsert(route(
            RouteScope::Node {
                id: "peer:a".into(),
            },
            "gw",
            &["t"],
            true,
        ));
        cfg.upsert(route(
            RouteScope::Group { name: "lab".into() },
            "gw",
            &["t"],
            true,
        ));
        // Not the node, not in the group, no AnyMesh route → None.
        assert!(resolve_route(&cfg, "peer:b", &["prod".to_string()]).is_none());
    }

    #[test]
    fn scope_matches_and_specificity_rank() {
        let n = RouteScope::Node {
            id: "peer:a".into(),
        };
        let g = RouteScope::Group { name: "lab".into() };
        let a = RouteScope::AnyMesh;
        assert!(n.matches("peer:a", &[]));
        assert!(!n.matches("peer:b", &[]));
        assert!(g.matches("peer:b", &["lab".to_string()]));
        assert!(!g.matches("peer:b", &["prod".to_string()]));
        assert!(a.matches("anything", &[]));
        assert!(n.specificity() > g.specificity());
        assert!(g.specificity() > a.specificity());
    }

    #[test]
    fn routes_load_save_round_trip_on_disk_and_default_killswitch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = RouteConfig::default();
        cfg.upsert(route(RouteScope::AnyMesh, "peer:gw", &["mullvad1"], true));
        save_routes(tmp.path(), &cfg).unwrap();
        assert_eq!(load_routes(tmp.path()), cfg);
        // Missing → default empty.
        assert_eq!(
            load_routes(tmp.path().join("nope").as_path()),
            RouteConfig::default()
        );
        // A route omitting kill_switch in TOML defaults to true (block-on-drop).
        let toml = "[[route]]\ngateway = \"peer:gw\"\nchain = [\"t\"]\n\
                    [route.scope]\nkind = \"any-mesh\"\n";
        let parsed = RouteConfig::from_toml_str(toml).unwrap();
        assert!(parsed.route[0].kill_switch, "kill_switch defaults to true");
    }
}
