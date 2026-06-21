//! VPN-GW-1 — the per-node commercial-VPN tunnel engine (WireGuard/OpenVPN
//! baseline).
//!
//! Brings named exit tunnels up/down on this node: WireGuard via `wg-quick` and
//! OpenVPN via `openvpn`, each on its own interface `mvpn-<id>`. A node can run
//! N tunnels concurrently — different providers, or the same provider more than
//! once (distinct ids → distinct interfaces). Tunnel definitions persist
//! node-locally (`<state_dir>/tunnels.json`) so they survive a `mackesd`
//! restart, and each tunnel's config text lands in a `0600` file the engine
//! hands to `wg-quick`/`openvpn`.
//!
//! Scope of VPN-GW-1: the create/destroy/up/down/status lifecycle + persistence.
//! The selective policy-routing + NAT + kill-switch egress layer is VPN-GW-3;
//! age-encrypting the configs at rest + leader distribution is VPN-GW-2; the
//! `action/vpn/*` bus surface + worker-pool spawn are the wiring follow-ons.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Node-local dir holding the tunnel registry + per-tunnel config files. Mirrors
/// the `/var/lib/mackesd/<feature>` local-state convention (CONNECT, CA, creds).
pub const VPN_STATE_DIR: &str = "/var/lib/mackesd/vpn";

/// Interface-name prefix for every engine-managed tunnel: `mvpn-<id>`. Five
/// bytes, leaving 10 of the 15-byte Linux `IFNAMSIZ-1` budget for the id.
pub const IFACE_PREFIX: &str = "mvpn-";

/// Max id length so `mvpn-<id>` fits the 15-byte interface-name limit losslessly
/// (no two valid ids can collide after truncation).
pub const MAX_TUNNEL_ID_LEN: usize = 10;

/// The tunnel transport. VPN-GW-1 ships the two self-hostable baselines; the
/// provider-CLI/API methods (Mullvad/Proton/Nord…) land with VPN-GW-5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelMethod {
    /// WireGuard, brought up with `wg-quick`.
    Wg,
    /// OpenVPN, brought up with `openvpn`.
    Ovpn,
}

impl TunnelMethod {
    /// The config-file extension this method's tool expects.
    #[must_use]
    pub const fn config_ext(self) -> &'static str {
        match self {
            TunnelMethod::Wg => "conf",
            TunnelMethod::Ovpn => "ovpn",
        }
    }
}

/// A named VPN tunnel definition. `id` is the stable handle (the live interface
/// is `mvpn-<id>`); `config` is the raw `wg-quick`/`.ovpn` text. The config is a
/// secret — VPN-GW-2 age-encrypts it at rest + leader-distributes it; here it
/// lands in a `0600` file. `provider`/`region` are descriptive (UI + health).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDef {
    /// Stable handle; `[a-z0-9-]`, 1..=[`MAX_TUNNEL_ID_LEN`], not edge-`-`.
    pub id: String,
    /// Descriptive provider name (e.g. `mullvad`, `generic-wg`).
    pub provider: String,
    /// Transport baseline.
    pub method: TunnelMethod,
    /// Optional server/region label (descriptive).
    #[serde(default)]
    pub region: String,
    /// Raw tunnel config text (a `wg-quick` `.conf` or an OpenVPN `.ovpn`).
    pub config: String,
}

/// The node-local persisted tunnel registry (survives daemon restart).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelStore {
    /// Every tunnel defined on this node, by definition order.
    #[serde(default)]
    pub tunnels: Vec<TunnelDef>,
}

/// `true` if `id` is a valid tunnel id: 1..=[`MAX_TUNNEL_ID_LEN`] chars, only
/// lowercase ASCII alphanumerics or `-`, and not starting/ending with `-` (so
/// `iface_name` is lossless + `wg-quick`/`ip` never choke on it).
#[must_use]
pub fn valid_tunnel_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_TUNNEL_ID_LEN {
        return false;
    }
    if id.starts_with('-') || id.ends_with('-') {
        return false;
    }
    id.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The interface name for a tunnel id: `mvpn-<id>`. Assumes [`valid_tunnel_id`]
/// (the engine validates on `add`); for defensiveness it still sanitises to
/// `[a-z0-9-]` and truncates to the id budget so a hand-edited registry can
/// never produce an oversized/invalid interface name.
#[must_use]
pub fn iface_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .map(|c| c.to_ascii_lowercase())
        .take(MAX_TUNNEL_ID_LEN)
        .collect();
    format!("{IFACE_PREFIX}{safe}")
}

/// `wg-quick up <config-path>` argv (wg-quick derives the interface from the
/// config's basename, which the engine sets to `mvpn-<id>.conf`).
#[must_use]
pub fn wg_quick_up_argv(config_path: &Path) -> Vec<String> {
    vec!["up".into(), config_path.to_string_lossy().into_owned()]
}

/// `wg-quick down <config-path>` argv.
#[must_use]
pub fn wg_quick_down_argv(config_path: &Path) -> Vec<String> {
    vec!["down".into(), config_path.to_string_lossy().into_owned()]
}

/// `openvpn --config <path> --dev <iface> --daemon` argv — a backgrounded
/// OpenVPN bound to the engine's `mvpn-<id>` tun interface.
#[must_use]
pub fn openvpn_up_argv(config_path: &Path, iface: &str) -> Vec<String> {
    vec![
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--dev".into(),
        iface.to_string(),
        "--daemon".into(),
    ]
}

/// The per-tunnel config filename: `mvpn-<id>.<ext>` (ext per the method).
#[must_use]
pub fn config_filename(def: &TunnelDef) -> String {
    format!("{}.{}", iface_name(&def.id), def.method.config_ext())
}

/// Errors the engine surfaces to its callers (the bus actions, later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VpnError {
    /// The id failed [`valid_tunnel_id`].
    InvalidId(String),
    /// A tunnel with that id already exists (`add`).
    Duplicate(String),
    /// No tunnel with that id (`remove`/`up`/`down`/`status`).
    NotFound(String),
    /// A filesystem operation failed.
    Io(String),
}

impl std::fmt::Display for VpnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VpnError::InvalidId(s) => write!(f, "invalid tunnel id: {s:?}"),
            VpnError::Duplicate(s) => write!(f, "tunnel already exists: {s}"),
            VpnError::NotFound(s) => write!(f, "no such tunnel: {s}"),
            VpnError::Io(s) => write!(f, "vpn engine IO: {s}"),
        }
    }
}

/// The per-node tunnel engine. Holds the registry path + the tool binaries
/// (overridable for tests — an empty binary disables the shell-out so the pure
/// persistence path is exercised hermetically).
pub struct VpnGatewayWorker {
    state_dir: PathBuf,
    wg_quick_cmd: &'static str,
    openvpn_cmd: &'static str,
    ip_cmd: &'static str,
}

impl Default for VpnGatewayWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl VpnGatewayWorker {
    /// Build the engine against the default node-local state dir.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state_dir: PathBuf::from(VPN_STATE_DIR),
            wg_quick_cmd: "wg-quick",
            openvpn_cmd: "openvpn",
            ip_cmd: "ip",
        }
    }

    /// Override the state dir (tests).
    #[must_use]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = dir;
        self
    }

    /// Disable the tunnel-tool shell-outs (tests drive persistence only).
    #[must_use]
    pub fn without_commands(mut self) -> Self {
        self.wg_quick_cmd = "";
        self.openvpn_cmd = "";
        self.ip_cmd = "";
        self
    }

    /// Path of the persisted tunnel registry.
    fn store_path(&self) -> PathBuf {
        self.state_dir.join("tunnels.json")
    }

    /// Per-tunnel config-file path under the engine dir.
    fn config_path(&self, def: &TunnelDef) -> PathBuf {
        self.state_dir.join(config_filename(def))
    }

    /// Load the persisted registry (empty on missing/garbage — never panics, so
    /// a corrupt file degrades to "no tunnels" rather than wedging the daemon).
    #[must_use]
    pub fn load_store(&self) -> TunnelStore {
        std::fs::read_to_string(self.store_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persist the registry (creates the state dir as needed).
    fn save_store(&self, store: &TunnelStore) -> Result<(), VpnError> {
        std::fs::create_dir_all(&self.state_dir).map_err(|e| VpnError::Io(e.to_string()))?;
        let json = serde_json::to_string_pretty(store).map_err(|e| VpnError::Io(e.to_string()))?;
        std::fs::write(self.store_path(), json).map_err(|e| VpnError::Io(e.to_string()))
    }

    /// Write a tunnel's config text to its `0600` file.
    fn write_config(&self, def: &TunnelDef) -> Result<(), VpnError> {
        std::fs::create_dir_all(&self.state_dir).map_err(|e| VpnError::Io(e.to_string()))?;
        let path = self.config_path(def);
        std::fs::write(&path, &def.config).map_err(|e| VpnError::Io(e.to_string()))?;
        Self::chmod_600(&path);
        Ok(())
    }

    /// `chmod 0600` — a tunnel config holds the provider's private key/creds.
    fn chmod_600(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    /// List every defined tunnel (from the persisted registry).
    #[must_use]
    pub fn list_tunnels(&self) -> Vec<TunnelDef> {
        self.load_store().tunnels
    }

    /// Define + persist a new tunnel and write its config file. Idempotent only
    /// in the sense that a duplicate id is rejected — re-adding is an error, not
    /// a silent overwrite (the caller `update`s instead). Does NOT bring it up.
    pub fn add_tunnel(&self, def: TunnelDef) -> Result<(), VpnError> {
        if !valid_tunnel_id(&def.id) {
            return Err(VpnError::InvalidId(def.id));
        }
        let mut store = self.load_store();
        if store.tunnels.iter().any(|t| t.id == def.id) {
            return Err(VpnError::Duplicate(def.id));
        }
        self.write_config(&def)?;
        store.tunnels.push(def);
        self.save_store(&store)
    }

    /// Bring a tunnel down (best-effort), then remove its definition + config.
    pub fn remove_tunnel(&self, id: &str) -> Result<(), VpnError> {
        let mut store = self.load_store();
        let Some(pos) = store.tunnels.iter().position(|t| t.id == id) else {
            return Err(VpnError::NotFound(id.to_string()));
        };
        let def = store.tunnels.remove(pos);
        // Best-effort teardown before the config disappears.
        let _ = self.tunnel_down(&def);
        let _ = std::fs::remove_file(self.config_path(&def));
        self.save_store(&store)
    }

    /// Look up a tunnel definition by id.
    fn find(&self, id: &str) -> Result<TunnelDef, VpnError> {
        self.load_store()
            .tunnels
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| VpnError::NotFound(id.to_string()))
    }

    /// Bring a defined tunnel up (`wg-quick up` / `openvpn --daemon`).
    pub fn tunnel_up_by_id(&self, id: &str) -> Result<bool, VpnError> {
        let def = self.find(id)?;
        Ok(self.tunnel_up(&def))
    }

    /// Bring a defined tunnel down.
    pub fn tunnel_down_by_id(&self, id: &str) -> Result<bool, VpnError> {
        let def = self.find(id)?;
        Ok(self.tunnel_down(&def))
    }

    /// Bring `def` up via its method's tool. Returns `false` (no-op) when the
    /// tool shell-out is disabled (tests). The config file is (re)written first
    /// so `wg-quick`/`openvpn` always read the current definition.
    fn tunnel_up(&self, def: &TunnelDef) -> bool {
        if self.write_config(def).is_err() {
            return false;
        }
        let path = self.config_path(def);
        match def.method {
            TunnelMethod::Wg => self.run(self.wg_quick_cmd, &wg_quick_up_argv(&path)),
            TunnelMethod::Ovpn => self.run(
                self.openvpn_cmd,
                &openvpn_up_argv(&path, &iface_name(&def.id)),
            ),
        }
    }

    /// Bring `def` down. WireGuard tears down via `wg-quick down`; OpenVPN has no
    /// config-driven down, so the interface is removed directly (`ip link del`).
    fn tunnel_down(&self, def: &TunnelDef) -> bool {
        match def.method {
            TunnelMethod::Wg => {
                let path = self.config_path(def);
                self.run(self.wg_quick_cmd, &wg_quick_down_argv(&path))
            }
            TunnelMethod::Ovpn => self.run(
                self.ip_cmd,
                &[
                    "link".into(),
                    "del".into(),
                    "dev".into(),
                    iface_name(&def.id),
                ],
            ),
        }
    }

    /// `true` if a tunnel's interface currently exists (`ip link show <iface>`
    /// succeeds). Returns `false` when the `ip` shell-out is disabled (tests).
    #[must_use]
    pub fn is_up(&self, id: &str) -> bool {
        if self.ip_cmd.is_empty() {
            return false;
        }
        self.run(self.ip_cmd, &["link".into(), "show".into(), iface_name(id)])
    }

    /// Run `bin <args>` bounded; `true` on success. An empty `bin` (tests) is a
    /// no-op returning `false` so persistence tests never touch the network.
    fn run(&self, bin: &str, args: &[String]) -> bool {
        if bin.is_empty() {
            return false;
        }
        let mut cmd = std::process::Command::new(bin);
        cmd.args(args);
        crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wg(id: &str) -> TunnelDef {
        TunnelDef {
            id: id.to_string(),
            provider: "generic-wg".into(),
            method: TunnelMethod::Wg,
            region: "test".into(),
            config: "[Interface]\nPrivateKey=SECRET\n".into(),
        }
    }

    #[test]
    fn valid_tunnel_id_enforces_charset_length_and_edges() {
        assert!(valid_tunnel_id("mullvad-se"));
        assert!(valid_tunnel_id("a"));
        assert!(valid_tunnel_id("wg01"));
        assert!(!valid_tunnel_id(""), "empty rejected");
        assert!(!valid_tunnel_id("toolongname1"), "11 chars rejected");
        assert!(!valid_tunnel_id("-lead"), "leading dash rejected");
        assert!(!valid_tunnel_id("trail-"), "trailing dash rejected");
        assert!(!valid_tunnel_id("Has_Caps"), "underscore + caps rejected");
        assert!(!valid_tunnel_id("sp ace"), "space rejected");
    }

    #[test]
    fn iface_name_fits_the_15_byte_limit_and_is_lossless_for_valid_ids() {
        assert_eq!(iface_name("mullvad-se"), "mvpn-mullvad-se");
        assert!(iface_name("mullvad-se").len() <= 15);
        // Defensive: a hand-edited oversized id is sanitised + truncated.
        assert_eq!(iface_name("WAY_TOO_LONG_NAME!!"), "mvpn-waytoolong");
        assert!(iface_name("WAY_TOO_LONG_NAME!!").len() <= 15);
    }

    #[test]
    fn argv_builders_match_the_tool_contracts() {
        let p = PathBuf::from("/var/lib/mackesd/vpn/mvpn-x.conf");
        assert_eq!(
            wg_quick_up_argv(&p),
            vec!["up", "/var/lib/mackesd/vpn/mvpn-x.conf"]
        );
        assert_eq!(
            wg_quick_down_argv(&p),
            vec!["down", "/var/lib/mackesd/vpn/mvpn-x.conf"]
        );
        assert_eq!(
            openvpn_up_argv(&p, "mvpn-x"),
            vec![
                "--config",
                "/var/lib/mackesd/vpn/mvpn-x.conf",
                "--dev",
                "mvpn-x",
                "--daemon"
            ]
        );
    }

    #[test]
    fn config_filename_uses_the_method_extension() {
        assert_eq!(config_filename(&wg("se")), "mvpn-se.conf");
        let mut ov = wg("se");
        ov.method = TunnelMethod::Ovpn;
        assert_eq!(config_filename(&ov), "mvpn-se.ovpn");
    }

    #[test]
    fn add_persists_the_def_and_writes_a_0600_config() {
        let dir = tempfile::tempdir().unwrap();
        let eng = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        eng.add_tunnel(wg("se")).unwrap();

        // Registry round-trips.
        assert_eq!(eng.list_tunnels(), vec![wg("se")]);
        // Config file written.
        let cfg = dir.path().join("mvpn-se.conf");
        assert!(cfg.exists());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), wg("se").config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "config must be private (holds the provider key)"
            );
        }
    }

    #[test]
    fn state_survives_a_fresh_engine_instance() {
        // The persistence is the "survives a daemon restart" guarantee: a brand
        // new engine over the same dir sees the tunnels.
        let dir = tempfile::tempdir().unwrap();
        VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands()
            .add_tunnel(wg("se"))
            .unwrap();
        let fresh = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        assert_eq!(fresh.list_tunnels(), vec![wg("se")]);
    }

    #[test]
    fn add_rejects_invalid_ids_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let eng = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        let mut bad = wg("se");
        bad.id = "Bad Id".into();
        assert_eq!(
            eng.add_tunnel(bad),
            Err(VpnError::InvalidId("Bad Id".into()))
        );
        eng.add_tunnel(wg("se")).unwrap();
        assert_eq!(
            eng.add_tunnel(wg("se")),
            Err(VpnError::Duplicate("se".into()))
        );
    }

    #[test]
    fn two_concurrent_tunnels_get_distinct_interfaces() {
        // The ">= 2 concurrent tunnels, incl. same provider twice" requirement:
        // distinct ids → distinct interfaces + distinct config files.
        let dir = tempfile::tempdir().unwrap();
        let eng = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        let mut a = wg("se1");
        a.provider = "mullvad".into();
        let mut b = wg("se2");
        b.provider = "mullvad".into(); // same provider, second instance
        eng.add_tunnel(a).unwrap();
        eng.add_tunnel(b).unwrap();
        assert_eq!(eng.list_tunnels().len(), 2);
        assert_ne!(iface_name("se1"), iface_name("se2"));
        assert!(dir.path().join("mvpn-se1.conf").exists());
        assert!(dir.path().join("mvpn-se2.conf").exists());
    }

    #[test]
    fn remove_drops_the_def_and_config_and_errors_on_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let eng = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        eng.add_tunnel(wg("se")).unwrap();
        eng.remove_tunnel("se").unwrap();
        assert!(eng.list_tunnels().is_empty());
        assert!(!dir.path().join("mvpn-se.conf").exists());
        assert_eq!(
            eng.remove_tunnel("ghost"),
            Err(VpnError::NotFound("ghost".into()))
        );
    }

    #[test]
    fn lifecycle_ops_on_unknown_id_error() {
        let dir = tempfile::tempdir().unwrap();
        let eng = VpnGatewayWorker::new()
            .with_state_dir(dir.path().to_path_buf())
            .without_commands();
        assert_eq!(
            eng.tunnel_up_by_id("ghost"),
            Err(VpnError::NotFound("ghost".into()))
        );
        assert_eq!(
            eng.tunnel_down_by_id("ghost"),
            Err(VpnError::NotFound("ghost".into()))
        );
    }
}
