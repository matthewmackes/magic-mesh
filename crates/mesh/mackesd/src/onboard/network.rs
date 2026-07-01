//! OW-5 — `mackesd onboard network`: bring up the primary LAN interface *before*
//! the overlay, even on a static-only, no-DHCP network.
//!
//! The shape mirrors [`crate::onboard::self_test`]: an impure probe seam
//! ([`gather`]) collects live facts off the node, a pure fold ([`plan_network`])
//! turns them into a [`NetworkPlan`], and a second pure renderer
//! ([`render_keyfile`]) turns the plan into the NetworkManager keyfile. The two
//! pure functions are what the unit tests pin (no real `ip` / NetworkManager ever
//! appears in a test); the apply step goes through an injectable [`KeyfileSink`] so
//! the write + reload can be faked in tests and is idempotent in production.
//!
//! # Why this unit exists — the "cloud-init NM-keyfile fix"
//! A fresh box whose LAN serves no DHCP never gets a lease, so cloud-init's default
//! DHCP-only config leaves it unreachable — the overlay (which rides the LAN) then
//! can't come up either. This verb detects that case and writes a correct *static*
//! `.nmconnection` derived from the detected subnet, so the box reaches its LAN
//! first. When DHCP *is* present it writes a plain `method=auto` keyfile instead.
//!
//! # DHCP-vs-static detection reuses [`crate::router_discovery`]
//! The DHCP signal is "is there a default-route gateway?" — exactly what
//! [`crate::router_discovery::primary_default_gateway`] already parses out of
//! `ip route show default` (the same shell [`crate::router_discovery::discover_primary`]
//! runs). A discoverable gateway ⇒ a working DHCP-served / routed LAN ⇒ use DHCP;
//! no gateway ⇒ no lease ⇒ derive a static config from the interface's detected
//! subnet. We do not reimplement route detection — we fold its result.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// The system-connections directory NetworkManager reads keyfiles from.
pub const SYSTEM_CONNECTIONS_DIR: &str = "/etc/NetworkManager/system-connections";

/// The connection id (and keyfile stem) this verb owns. Stable so re-running is a
/// no-op on an unchanged keyfile rather than piling up new connections.
pub const CONNECTION_ID: &str = "mesh-lan";

/// The raw facts [`gather`] collects off the live node — the seam between the
/// (impure) `ip` probes and the pure [`plan_network`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkFacts {
    /// The primary LAN interface name (e.g. `eth0`); `None` when no physical NIC
    /// could be found at all.
    pub interface: Option<String>,
    /// The default-route gateway discovered via
    /// [`crate::router_discovery::primary_default_gateway`]. `Some` ⇒ a routed /
    /// DHCP-served LAN is already up (use DHCP); `None` ⇒ no lease/route (go static).
    pub gateway: Option<String>,
    /// The interface's current IPv4 in CIDR form (e.g. `172.20.0.50/24`), if it
    /// already carries a global address — the subnet a static config is derived
    /// from. `None` on a blank NIC (the classic no-DHCP box).
    pub cidr: Option<String>,
}

/// A resolved bring-up plan for the primary LAN interface — the headless body the
/// CLI prints and [`render_keyfile`] turns into a keyfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPlan {
    /// A working routed LAN was found → let NetworkManager keep using DHCP
    /// (`method=auto`). `gateway` is carried for the human report only.
    Dhcp {
        /// The interface the keyfile binds.
        interface: String,
        /// The discovered default gateway (report context; not written for DHCP).
        gateway: String,
    },
    /// No lease/route → a derived static config (the cloud-init NM-keyfile fix).
    Static {
        /// The interface the keyfile binds.
        interface: String,
        /// The host address, e.g. `172.20.0.50`.
        address: String,
        /// The CIDR prefix length, e.g. `24`.
        prefix: u8,
        /// The gateway — the first usable host of the detected subnet.
        gateway: String,
    },
}

impl NetworkPlan {
    /// The interface this plan brings up.
    #[must_use]
    pub fn interface(&self) -> &str {
        match self {
            Self::Dhcp { interface, .. } | Self::Static { interface, .. } => interface,
        }
    }

    /// Whether this plan uses DHCP (`method=auto`) rather than a static config.
    #[must_use]
    pub fn is_dhcp(&self) -> bool {
        matches!(self, Self::Dhcp { .. })
    }

    /// A one-line human description (DHCP vs static + the interface) for the CLI.
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::Dhcp { interface, gateway } => {
                format!("DHCP (method=auto) on {interface} — LAN gateway {gateway} reachable")
            }
            Self::Static {
                interface,
                address,
                prefix,
                gateway,
            } => format!(
                "static {address}/{prefix} gw {gateway} on {interface} \
                 (no DHCP — NetworkManager keyfile fix)"
            ),
        }
    }
}

/// Why a [`NetworkPlan`] could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPlanError {
    /// No LAN interface could be found — there is nothing to bring up.
    NoInterface,
    /// A static config is needed (no DHCP) but no subnet was detectable to derive
    /// one from (a truly blank NIC with no address and no route).
    NoStaticSubnet,
}

impl std::fmt::Display for NetworkPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInterface => write!(f, "no LAN interface found (nothing to bring up)"),
            Self::NoStaticSubnet => write!(
                f,
                "no DHCP and no detectable subnet to derive a static config from"
            ),
        }
    }
}

impl std::error::Error for NetworkPlanError {}

/// Pure fold: turn gathered [`NetworkFacts`] into a [`NetworkPlan`]. No I/O — fully
/// unit-testable. A discoverable gateway means DHCP; otherwise a static config is
/// derived from the detected subnet.
///
/// # Errors
/// [`NetworkPlanError::NoInterface`] when no LAN NIC was found;
/// [`NetworkPlanError::NoStaticSubnet`] when a static config is needed but no
/// subnet is detectable.
pub fn plan_network(f: &NetworkFacts) -> Result<NetworkPlan, NetworkPlanError> {
    let interface = f.interface.clone().ok_or(NetworkPlanError::NoInterface)?;

    // A discoverable default gateway ⇒ a working DHCP/routed LAN ⇒ use DHCP.
    if let Some(gateway) = &f.gateway {
        return Ok(NetworkPlan::Dhcp {
            interface,
            gateway: gateway.clone(),
        });
    }

    // No gateway ⇒ no lease ⇒ derive a static config from the detected subnet.
    let cidr = f.cidr.as_deref().ok_or(NetworkPlanError::NoStaticSubnet)?;
    let (address, prefix) = parse_cidr(cidr).ok_or(NetworkPlanError::NoStaticSubnet)?;
    let gateway = derive_gateway(&address, prefix).ok_or(NetworkPlanError::NoStaticSubnet)?;
    Ok(NetworkPlan::Static {
        interface,
        address,
        prefix,
        gateway,
    })
}

/// Pure renderer: turn a [`NetworkPlan`] into the NetworkManager keyfile
/// (`.nmconnection` INI). DHCP → `method=auto`; static → `method=manual` with the
/// `address1` + `gateway` keys. Deterministic (no uuid — NetworkManager generates
/// one from the keyfile), so it round-trips in tests.
#[must_use]
pub fn render_keyfile(plan: &NetworkPlan) -> String {
    let interface = plan.interface();
    let ipv4 = match plan {
        NetworkPlan::Dhcp { .. } => "method=auto\n".to_string(),
        NetworkPlan::Static {
            address,
            prefix,
            gateway,
            ..
        } => format!("method=manual\naddress1={address}/{prefix}\ngateway={gateway}\n"),
    };
    format!(
        "[connection]\n\
         id={CONNECTION_ID}\n\
         type=ethernet\n\
         interface-name={interface}\n\
         \n\
         [ipv4]\n\
         {ipv4}\
         \n\
         [ipv6]\n\
         method=auto\n"
    )
}

/// The keyfile path under `dir` for this verb's connection.
#[must_use]
pub fn keyfile_path(dir: &Path) -> PathBuf {
    dir.join(format!("{CONNECTION_ID}.nmconnection"))
}

/// The result of an [`apply`] — written or a safe idempotent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The on-disk keyfile already matched — nothing was written or reloaded.
    Unchanged,
    /// The keyfile was written and NetworkManager reloaded.
    Written,
}

impl ApplyOutcome {
    /// Short lowercase tag for the human report line.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Written => "written",
        }
    }
}

/// The injectable write+reload seam. Production is [`SystemConnections`]; tests use
/// a recording fake so no real NetworkManager is touched.
pub trait KeyfileSink {
    /// The current content of the keyfile at `path`, or `None` if absent.
    fn read(&self, path: &Path) -> Option<String>;
    /// Write `content` to `path` (0600 — NetworkManager rejects world-readable
    /// keyfiles).
    ///
    /// # Errors
    /// Any underlying filesystem error.
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()>;
    /// Reload NetworkManager so it picks the keyfile up.
    ///
    /// # Errors
    /// When the reload command cannot be run or reports failure.
    fn reload(&self) -> std::io::Result<()>;
}

/// Apply `plan`'s keyfile under `dir` through `sink`. Idempotent: when the on-disk
/// keyfile already matches, it is a no-op ([`ApplyOutcome::Unchanged`]) — no write,
/// no reload.
///
/// # Errors
/// Propagates any [`KeyfileSink`] write/reload error.
pub fn apply(
    plan: &NetworkPlan,
    dir: &Path,
    sink: &dyn KeyfileSink,
) -> std::io::Result<ApplyOutcome> {
    let path = keyfile_path(dir);
    let content = render_keyfile(plan);
    if sink.read(&path).as_deref() == Some(content.as_str()) {
        return Ok(ApplyOutcome::Unchanged);
    }
    sink.write(&path, &content)?;
    sink.reload()?;
    Ok(ApplyOutcome::Written)
}

/// Production [`KeyfileSink`]: writes under `/etc/NetworkManager/system-connections`
/// at mode 0600 and runs `nmcli connection reload`.
pub struct SystemConnections;

impl KeyfileSink for SystemConnections {
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(content.as_bytes())
    }

    fn reload(&self) -> std::io::Result<()> {
        let status = std::process::Command::new("nmcli")
            .args(["connection", "reload"])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("nmcli connection reload failed"))
        }
    }
}

/// Impure probe shell: gather the live LAN facts off this node. Best effort — a
/// missing `ip` / no route degrades to `None` fields rather than erroring, so the
/// pure [`plan_network`] fold always runs and produces the real verdict.
#[must_use]
pub fn gather() -> NetworkFacts {
    // DHCP signal: reuse router_discovery's default-gateway parser (do not
    // reimplement route detection — fold its result).
    let route = run_ip(&["route", "show", "default"]).unwrap_or_default();
    let gateway = crate::router_discovery::primary_default_gateway(&route);

    let (interface, cidr) = primary_lan_iface();
    NetworkFacts {
        interface,
        gateway,
        cidr,
    }
}

/// `ip <args>` → stdout, or `None` on a missing binary / non-zero exit. Mirrors
/// [`crate::router_discovery`]'s `ip`-shell pattern.
fn run_ip(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("ip").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Interface names we never treat as the LAN NIC (loopback + overlay/virtual): the
/// LAN verb must not pick the mesh overlay (`nebula*`) or a bridge as the primary.
const VIRTUAL_IFACE_PREFIXES: &[&str] = &[
    "nebula", "docker", "veth", "br-", "virbr", "tun", "tap", "wg",
];

/// Whether `name` is loopback or a virtual/overlay interface (skipped as the LAN).
fn is_virtual_iface(name: &str) -> bool {
    name == "lo" || VIRTUAL_IFACE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The primary LAN interface + its current CIDR. Prefers an interface already
/// carrying a global IPv4 (its subnet seeds a static derivation); falls back to the
/// first physical link with no address (the blank no-DHCP NIC).
fn primary_lan_iface() -> (Option<String>, Option<String>) {
    if let Some(addr_out) = run_ip(&["-o", "-4", "addr", "show"]) {
        if let Some((iface, cidr)) = first_global_ipv4(&addr_out) {
            return (Some(iface), Some(cidr));
        }
    }
    let iface = run_ip(&["-o", "link", "show"]).and_then(|o| first_nonlo_link(&o));
    (iface, None)
}

/// First non-loopback, non-virtual interface carrying a `scope global` IPv4, as
/// `(iface, cidr)`, parsed from `ip -o -4 addr show` lines like
/// `2: eth0    inet 172.20.0.50/24 brd 172.20.0.255 scope global eth0`.
fn first_global_ipv4(stdout: &str) -> Option<(String, String)> {
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(iface) = toks.get(1) else { continue };
        if is_virtual_iface(iface) {
            continue;
        }
        if !toks.iter().any(|t| *t == "global") {
            continue;
        }
        let Some(pos) = toks.iter().position(|t| *t == "inet") else {
            continue;
        };
        let Some(cidr) = toks.get(pos + 1) else {
            continue;
        };
        return Some(((*iface).to_string(), (*cidr).to_string()));
    }
    None
}

/// First non-loopback, non-virtual interface name from `ip -o link show` lines like
/// `2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...` (token 1 = `eth0:`).
fn first_nonlo_link(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(raw) = toks.get(1) else { continue };
        let name = raw.trim_end_matches(':');
        if name.is_empty() || is_virtual_iface(name) {
            continue;
        }
        return Some(name.to_string());
    }
    None
}

/// Split `A.B.C.D/P` into a validated `(ipv4, prefix)`; `None` if malformed, not
/// IPv4, or the prefix exceeds 32.
fn parse_cidr(cidr: &str) -> Option<(String, u8)> {
    let (addr, prefix) = cidr.split_once('/')?;
    let ip: Ipv4Addr = addr.parse().ok()?;
    let p: u8 = prefix.parse().ok()?;
    if p > 32 {
        return None;
    }
    Some((ip.to_string(), p))
}

/// Derive the conventional gateway (the subnet's first usable host) for
/// `addr/prefix`, e.g. `172.20.0.50/24` → `172.20.0.1`. `None` for prefixes with no
/// host room (`>= 31`).
fn derive_gateway(addr: &str, prefix: u8) -> Option<String> {
    if prefix >= 31 {
        return None;
    }
    let ip: Ipv4Addr = addr.parse().ok()?;
    let bits = u32::from(ip);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let network = bits & mask;
    Some(Ipv4Addr::from(network + 1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn facts(iface: Option<&str>, gw: Option<&str>, cidr: Option<&str>) -> NetworkFacts {
        NetworkFacts {
            interface: iface.map(str::to_string),
            gateway: gw.map(str::to_string),
            cidr: cidr.map(str::to_string),
        }
    }

    #[test]
    fn dhcp_present_uses_dhcp() {
        // A discoverable gateway ⇒ DHCP (method=auto), whatever the address is.
        let plan = plan_network(&facts(
            Some("eth0"),
            Some("172.20.0.1"),
            Some("172.20.0.50/24"),
        ))
        .expect("plan");
        assert!(plan.is_dhcp());
        assert_eq!(plan.interface(), "eth0");
        assert_eq!(
            plan,
            NetworkPlan::Dhcp {
                interface: "eth0".into(),
                gateway: "172.20.0.1".into(),
            }
        );
    }

    #[test]
    fn no_dhcp_derives_static_from_subnet() {
        // No gateway ⇒ derive a static config: address kept, gateway = subnet .1.
        let plan = plan_network(&facts(Some("eth0"), None, Some("172.20.0.50/24"))).expect("plan");
        assert_eq!(
            plan,
            NetworkPlan::Static {
                interface: "eth0".into(),
                address: "172.20.0.50".into(),
                prefix: 24,
                gateway: "172.20.0.1".into(),
            }
        );
        assert!(!plan.is_dhcp());
    }

    #[test]
    fn no_interface_is_an_error() {
        assert_eq!(
            plan_network(&facts(None, Some("172.20.0.1"), Some("172.20.0.50/24"))),
            Err(NetworkPlanError::NoInterface)
        );
    }

    #[test]
    fn no_dhcp_and_no_subnet_is_an_error() {
        // A blank NIC (no address, no route) has nothing to derive a static from.
        assert_eq!(
            plan_network(&facts(Some("eth0"), None, None)),
            Err(NetworkPlanError::NoStaticSubnet)
        );
    }

    #[test]
    fn keyfile_dhcp_is_method_auto() {
        let plan = NetworkPlan::Dhcp {
            interface: "eth0".into(),
            gateway: "172.20.0.1".into(),
        };
        let kf = render_keyfile(&plan);
        assert!(kf.contains("[connection]"));
        assert!(kf.contains("interface-name=eth0"));
        assert!(kf.contains("[ipv4]"));
        assert!(kf.contains("method=auto"));
        // DHCP must not pin an address/gateway.
        assert!(!kf.contains("method=manual"));
        assert!(!kf.contains("address1="));
        assert!(!kf.contains("gateway="));
    }

    #[test]
    fn keyfile_static_has_addresses_and_gateway() {
        let plan = NetworkPlan::Static {
            interface: "eth0".into(),
            address: "172.20.0.50".into(),
            prefix: 24,
            gateway: "172.20.0.1".into(),
        };
        let kf = render_keyfile(&plan);
        assert!(kf.contains("interface-name=eth0"));
        assert!(kf.contains("[ipv4]"));
        assert!(kf.contains("method=manual"));
        assert!(kf.contains("address1=172.20.0.50/24"));
        assert!(kf.contains("gateway=172.20.0.1"));
        // Expected-keys round-trip: parse the INI back and confirm the ipv4 keys.
        let keys = ini_keys(&kf);
        assert_eq!(keys.get("ipv4/method").map(String::as_str), Some("manual"));
        assert_eq!(
            keys.get("ipv4/address1").map(String::as_str),
            Some("172.20.0.50/24")
        );
        assert_eq!(
            keys.get("ipv4/gateway").map(String::as_str),
            Some("172.20.0.1")
        );
        assert_eq!(
            keys.get("connection/interface-name").map(String::as_str),
            Some("eth0")
        );
    }

    /// Flatten an INI keyfile into `section/key -> value` for round-trip asserts.
    fn ini_keys(kf: &str) -> HashMap<String, String> {
        let mut section = String::new();
        let mut out = HashMap::new();
        for line in kf.lines() {
            let line = line.trim();
            if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = inner.to_string();
            } else if let Some((k, v)) = line.split_once('=') {
                out.insert(format!("{section}/{k}"), v.to_string());
            }
        }
        out
    }

    /// Recording [`KeyfileSink`] fake: records writes/reloads in memory so
    /// idempotency can be asserted without a real NetworkManager.
    struct RecordingSink {
        files: RefCell<HashMap<PathBuf, String>>,
        writes: RefCell<usize>,
        reloads: RefCell<usize>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                files: RefCell::new(HashMap::new()),
                writes: RefCell::new(0),
                reloads: RefCell::new(0),
            }
        }
    }

    impl KeyfileSink for RecordingSink {
        fn read(&self, path: &Path) -> Option<String> {
            self.files.borrow().get(path).cloned()
        }
        fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
            *self.writes.borrow_mut() += 1;
            self.files
                .borrow_mut()
                .insert(path.to_path_buf(), content.to_string());
            Ok(())
        }
        fn reload(&self) -> std::io::Result<()> {
            *self.reloads.borrow_mut() += 1;
            Ok(())
        }
    }

    #[test]
    fn apply_writes_once_then_is_idempotent() {
        let plan = NetworkPlan::Static {
            interface: "eth0".into(),
            address: "172.20.0.50".into(),
            prefix: 24,
            gateway: "172.20.0.1".into(),
        };
        let dir = Path::new(SYSTEM_CONNECTIONS_DIR);
        let sink = RecordingSink::new();

        // First apply writes + reloads.
        assert_eq!(apply(&plan, dir, &sink).unwrap(), ApplyOutcome::Written);
        assert_eq!(*sink.writes.borrow(), 1);
        assert_eq!(*sink.reloads.borrow(), 1);
        // The keyfile landed at the expected path with the rendered content.
        let path = keyfile_path(dir);
        assert_eq!(
            sink.files.borrow().get(&path).map(String::as_str),
            Some(render_keyfile(&plan).as_str())
        );

        // Re-running with the same plan is a safe no-op — no second write/reload.
        assert_eq!(apply(&plan, dir, &sink).unwrap(), ApplyOutcome::Unchanged);
        assert_eq!(*sink.writes.borrow(), 1);
        assert_eq!(*sink.reloads.borrow(), 1);
    }

    #[test]
    fn apply_rewrites_when_the_plan_changes() {
        let dir = Path::new(SYSTEM_CONNECTIONS_DIR);
        let sink = RecordingSink::new();
        let dhcp = NetworkPlan::Dhcp {
            interface: "eth0".into(),
            gateway: "172.20.0.1".into(),
        };
        let stat = NetworkPlan::Static {
            interface: "eth0".into(),
            address: "172.20.0.50".into(),
            prefix: 24,
            gateway: "172.20.0.1".into(),
        };
        assert_eq!(apply(&dhcp, dir, &sink).unwrap(), ApplyOutcome::Written);
        // A different plan at the same path rewrites (keyfile no longer matches).
        assert_eq!(apply(&stat, dir, &sink).unwrap(), ApplyOutcome::Written);
        assert_eq!(*sink.writes.borrow(), 2);
    }

    #[test]
    fn derive_gateway_is_the_subnet_first_host() {
        assert_eq!(
            derive_gateway("172.20.0.50", 24).as_deref(),
            Some("172.20.0.1")
        );
        assert_eq!(derive_gateway("10.5.6.7", 8).as_deref(), Some("10.0.0.1"));
        assert_eq!(
            derive_gateway("192.168.4.9", 22).as_deref(),
            Some("192.168.4.1")
        );
        // No host room on /31 or /32.
        assert_eq!(derive_gateway("172.20.0.50", 31), None);
        assert_eq!(derive_gateway("172.20.0.50", 32), None);
    }

    #[test]
    fn parse_cidr_validates_ipv4_and_prefix() {
        assert_eq!(
            parse_cidr("172.20.0.50/24"),
            Some(("172.20.0.50".into(), 24))
        );
        assert_eq!(parse_cidr("172.20.0.50"), None); // no prefix
        assert_eq!(parse_cidr("not-an-ip/24"), None);
        assert_eq!(parse_cidr("172.20.0.50/40"), None); // prefix > 32
    }

    #[test]
    fn first_global_ipv4_skips_lo_and_overlay() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\n\
                   10: nebula1    inet 10.42.0.2/16 scope global nebula1\n\
                   2: eth0    inet 172.20.0.50/24 brd 172.20.0.255 scope global eth0\n";
        // lo (host) skipped, nebula1 (overlay) skipped → eth0 wins.
        assert_eq!(
            first_global_ipv4(out),
            Some(("eth0".into(), "172.20.0.50/24".into()))
        );
    }

    #[test]
    fn first_global_ipv4_skips_link_local_scope() {
        // A link-local (scope link) is not a real subnet → no global match.
        let out = "2: eth0    inet 169.254.3.4/16 brd 169.254.255.255 scope link eth0\n";
        assert_eq!(first_global_ipv4(out), None);
    }

    #[test]
    fn first_nonlo_link_finds_the_blank_nic() {
        let out = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 ...\n\
                   2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...\n";
        assert_eq!(first_nonlo_link(out).as_deref(), Some("eth0"));
    }
}
