//! Workbench · This Node — live local-node status (WB-ThisNode).
//!
//! The first Workbench plane, wired off the SAME world-readable mesh-status
//! snapshot the chrome bar folds (`/run/mde/mesh-status.json`, written every ~30s
//! by the root `mesh-status.timer`). The desktop user can't read the root-only
//! replicated peer directory, so this JSON is the desktop tier's read path — the
//! shell leans on no `mackesd` IPC (§6). Every field here is real, live-updating
//! node reality; nothing is a stand-in (§7):
//!
//! * **Identity** — this node's hostname (the snapshot's own `self` marker), its
//!   pinned `role`, its Nebula `overlay_ip`, and the tunnel `cipher`.
//! * **Presence + heartbeat** — the node's directory `presence` tier
//!   (online/idle/offline) and the freshness of its last heartbeat, measured
//!   against the snapshot's own `generated_ms` clock (no desktop-clock skew).
//! * **Version** — the installed `mde-core` version and whether a newer one is
//!   live on the mesh (the snapshot's fleet-wide `latest_version` fold).
//! * **Node services** — this node's own daemon health (mackesd / Nebula /
//!   Syncthing / Bus / DNS / Voice / Music / KDE-Connect / Workbench), the
//!   `services` map each node publishes into its `shell-status.json`.
//! * **Mesh context** — the live peer count (online / total) and the elected mesh
//!   leader.
//!
//! What this surface honestly **cannot** show: live CPU / memory / disk
//! utilisation. Those aren't in the world-readable snapshot — they're node-local
//! telemetry (a `mackesd` / Netdata concern), and §6 keeps the shell off that
//! path. The panel renders an explicit "not published to this surface" note
//! rather than a fabricated gauge (§7).
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`ThisNodeState::poll`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar reads
/// (the desktop user can't read the root-only replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a heartbeat, a service flip, or a role change surfaces within
/// this window. Matches the chrome bar + the Fleet datacenter poll; the read is a
/// cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// Keep the world-readable mesh snapshot bounded before `serde_json` walks its
/// peer directory and service maps. The writer is local, but the desktop tier
/// treats this filesystem boundary as hostile and fails soft.
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;

/// Read one mesh-status snapshot through the descriptor that is consumed.
/// Reject the final symlink, special descriptors, oversized input, and files
/// whose size changes while they are being read before JSON materialization.
fn read_bounded_snapshot(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
        {
            return None;
        }
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.file_type().is_file() || before.len() > MAX_SNAPSHOT_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(before.len())
            .unwrap_or(MAX_SNAPSHOT_BYTES)
            .saturating_add(1),
    );
    (&file)
        .take((MAX_SNAPSHOT_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != before.len()
        || after.len() != u64::try_from(bytes.len()).ok()?
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// use, so a service dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// This node's daemon catalog: the `services` map key each node publishes into its
/// `shell-status.json`, paired with the label the plane renders. Fixed order so the
/// health list is stable frame-to-frame; a key absent from the snapshot is simply
/// not listed (never rendered as a false "down").
const SERVICE_CATALOG: [(&str, &str); 9] = [
    ("mackesd", "Mesh daemon"),
    ("nebula", "Overlay (Nebula)"),
    ("sync", "Sync (Syncthing)"),
    ("bus", "Mesh Bus"),
    ("dns", "Mesh DNS"),
    ("voice", "Voice HUD"),
    ("music", "Music"),
    ("kdc", "KDE Connect"),
    ("workbench", "Workbench"),
];

/// Keep list-valued connectivity facts small even when a faulty or newer
/// snapshot carries more entries than this surface can render usefully.
const MAX_CONNECTIVITY_FACTS: usize = 8;

const CONNECTIVITY_PROVIDER_CATALOG: [ConnectivityProvider; 5] = [
    ConnectivityProvider::Wifi,
    ConnectivityProvider::Ethernet,
    ConnectivityProvider::Cellular,
    ConnectivityProvider::Mesh,
    ConnectivityProvider::DnsLighthouse,
];

// ──────────────────────────── projected view ────────────────────────────

/// This node's live status, folded from the mesh-status snapshot. Pure data
/// (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NodeStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// `true` when this node's OWN row was found in the snapshot's directory
    /// (`nodes[]`). `false` when the snapshot is readable but this node hasn't
    /// published a heartbeat record yet — the per-node fields then render honest
    /// "not yet in the peer directory", never a fabricated value.
    in_directory: bool,
    /// This node's hostname — the snapshot's `self` marker (local hostname when the
    /// snapshot omits it).
    hostname: String,
    /// Pinned deployment role (`lighthouse` / `server` / `workstation`), when known.
    role: Option<String>,
    /// This node's Nebula overlay IP, when known.
    overlay_ip: Option<String>,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// Wall-clock ms of this node's last heartbeat (`0` when never reported).
    last_seen_ms: u64,
    /// When the snapshot was generated — the reference clock for heartbeat age (so
    /// freshness can't skew against the desktop's own clock).
    generated_ms: u64,
    /// Installed `mde-core` version, when known.
    version: Option<String>,
    /// `true` when a newer version than this node's is live on the mesh.
    update_available: bool,
    /// The newest version seen across the mesh (for the update hint).
    latest_version: Option<String>,
    /// This node's own daemon health, in catalog order (label, up).
    services: Vec<(&'static str, bool)>,
    /// Peers in the directory currently `online`.
    peers_online: u64,
    /// Peers in the directory (every node the snapshot names).
    peers_total: u64,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// The Nebula tunnel cipher label, when nebula is up.
    cipher: Option<String>,
    /// Read-only interface, route, lighthouse, and resolver facts published by
    /// the network section of mesh-status.
    connectivity: ConnectivityFacts,
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse the `services` map into the catalog-ordered (label, up) rows actually
/// present. A missing map (an older writer / a node with no `shell-status.json`)
/// yields an empty list → the view says "not yet reported" rather than a false
/// all-down.
fn parse_services(services: Option<&Value>) -> Vec<(&'static str, bool)> {
    let Some(obj) = services.and_then(Value::as_object) else {
        return Vec::new();
    };
    SERVICE_CATALOG
        .iter()
        .filter_map(|(key, label)| {
            obj.get(*key)
                .and_then(Value::as_bool)
                .map(|up| (*label, up))
        })
        .collect()
}

/// The read-only connectivity facts this node can prove from mesh-status.
/// Empty fields stay empty so the renderer can say exactly which observation
/// is unavailable instead of filling a gap with local guesses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConnectivityFacts {
    interface: Option<String>,
    cidr: Option<String>,
    default_route: Option<String>,
    lighthouses: Vec<String>,
    dns_servers: Vec<String>,
    /// Explicit underlay observations are optional because the current
    /// mesh-status writer only publishes the overlay. Never infer a provider
    /// from an interface prefix or from the presence of a default route.
    interfaces: [Option<InterfaceProviderFacts>; 3],
}

impl ConnectivityFacts {
    fn from_network(network: Option<&Value>) -> Self {
        let interface = first_network_string(network, &["overlay_if", "interface", "ifname"])
            .or_else(|| first_interface_entry_string(network, &["name", "interface", "ifname"]));
        let cidr = first_network_string(network, &["overlay_cidr", "cidr"])
            .or_else(|| first_interface_entry_string(network, &["cidr", "ip_cidr"]));
        let default_route =
            first_network_string(network, &["default_gw", "default_route", "default_gateway"]);

        Self {
            interface,
            cidr,
            default_route,
            lighthouses: network_fact_list(network, &["lighthouse_ips", "lighthouses"]),
            dns_servers: network_fact_list(
                network,
                &["dns_servers", "nameservers", "resolvers", "dns"],
            ),
            interfaces: interface_provider_facts(network),
        }
    }

    fn is_empty(&self) -> bool {
        self.interface.is_none()
            && self.cidr.is_none()
            && self.default_route.is_none()
            && self.lighthouses.is_empty()
            && self.dns_servers.is_empty()
            && self.interfaces.iter().all(Option::is_none)
    }

    fn has_underlay_observation(&self) -> bool {
        self.interfaces.iter().any(Option::is_some)
    }

    fn provider_projection(&self) -> [ConnectivityProviderProjection; 5] {
        CONNECTIVITY_PROVIDER_CATALOG.map(|provider| match provider {
            ConnectivityProvider::Wifi => {
                interface_provider_projection(provider, self.interfaces[0].as_ref())
            }
            ConnectivityProvider::Ethernet => {
                interface_provider_projection(provider, self.interfaces[1].as_ref())
            }
            ConnectivityProvider::Cellular => {
                interface_provider_projection(provider, self.interfaces[2].as_ref())
            }
            ConnectivityProvider::Mesh => ConnectivityProviderProjection {
                provider,
                availability: self.mesh_provider_availability(),
                interface: self.interface.clone(),
                cidr: self.cidr.clone(),
            },
            ConnectivityProvider::DnsLighthouse => ConnectivityProviderProjection {
                provider,
                availability: self.dns_lighthouse_availability(),
                interface: None,
                cidr: None,
            },
        })
    }

    fn mesh_provider_availability(&self) -> ConnectivityAvailability {
        if self.interface.is_none() && self.cidr.is_none() {
            return ConnectivityAvailability::Unavailable(
                "No explicit mesh overlay interface or CIDR is published.",
            );
        }
        if self.interface.is_some()
            && self.cidr.is_some()
            && (!self.lighthouses.is_empty() || !self.dns_servers.is_empty())
        {
            ConnectivityAvailability::Available(
                "Mesh overlay interface and CIDR are published with reachability evidence.",
            )
        } else {
            ConnectivityAvailability::Degraded(
                "Mesh overlay facts are partial; interface, CIDR, and reachability are not all published.",
            )
        }
    }

    fn dns_lighthouse_availability(&self) -> ConnectivityAvailability {
        match (self.dns_servers.is_empty(), self.lighthouses.is_empty()) {
            (false, false) => ConnectivityAvailability::Available(
                "Mesh DNS and lighthouse endpoints are published by the snapshot.",
            ),
            (false, true) => ConnectivityAvailability::Available(
                "Mesh DNS resolvers are published; lighthouse endpoints are not published.",
            ),
            (true, false) => ConnectivityAvailability::Available(
                "Lighthouse endpoints are published; mesh DNS resolvers are not published.",
            ),
            (true, true) => ConnectivityAvailability::Unavailable(
                "No DNS resolver or lighthouse endpoint is published.",
            ),
        }
    }
}

/// A connectivity card's state is separate from the broader capability list:
/// a readable snapshot can still have no published node-local network facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectivityAvailability {
    Available(&'static str),
    Degraded(&'static str),
    Unavailable(&'static str),
}

impl ConnectivityAvailability {
    const fn tone(self) -> Color32 {
        match self {
            Self::Available(_) => Style::OK,
            Self::Degraded(_) => Style::WARN,
            Self::Unavailable(_) => Style::TEXT_DIM,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Available(_) => "available",
            Self::Degraded(_) => "degraded",
            Self::Unavailable(_) => "unavailable",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Available(detail) | Self::Degraded(detail) | Self::Unavailable(detail) => detail,
        }
    }
}

/// Provider kinds are accepted only when the snapshot names them explicitly.
/// In particular, `wlan*`, `en*`, and `wwan*` prefixes are not evidence of a
/// backend and therefore never select a provider here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectivityProvider {
    Wifi,
    Ethernet,
    Cellular,
    Mesh,
    DnsLighthouse,
}

impl ConnectivityProvider {
    const fn label(self) -> &'static str {
        match self {
            Self::Wifi => "Wi-Fi",
            Self::Ethernet => "Ethernet",
            Self::Cellular => "Cellular",
            Self::Mesh => "Mesh overlay",
            Self::DnsLighthouse => "DNS / lighthouse",
        }
    }

    const fn index(self) -> Option<usize> {
        match self {
            Self::Wifi => Some(0),
            Self::Ethernet => Some(1),
            Self::Cellular => Some(2),
            Self::Mesh | Self::DnsLighthouse => None,
        }
    }
}

/// The only underlay state admitted into the read model. Raw NetworkManager /
/// ModemManager payloads, SSIDs, APNs, passwords, and PSKs are intentionally not
/// represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderLinkState {
    Connected,
    Degraded,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InterfaceProviderFacts {
    state: ProviderLinkState,
    interface: Option<String>,
    cidr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectivityProviderProjection {
    provider: ConnectivityProvider,
    availability: ConnectivityAvailability,
    interface: Option<String>,
    cidr: Option<String>,
}

fn first_network_string(network: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| network.and_then(|value| nonempty(value, key)))
}

fn first_interface_entry_string(network: Option<&Value>, keys: &[&str]) -> Option<String> {
    network
        .and_then(|value| value.get("interfaces"))
        .and_then(Value::as_array)
        .and_then(|interfaces| {
            interfaces
                .iter()
                .find_map(|interface| keys.iter().find_map(|key| nonempty(interface, key)))
        })
}

fn bounded_strings(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .take(MAX_CONNECTIVITY_FACTS)
            .map(str::to_string)
            .collect(),
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .take(MAX_CONNECTIVITY_FACTS)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn network_fact_list(network: Option<&Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = network.and_then(|network| network.get(*key)) else {
            continue;
        };
        let direct = bounded_strings(value);
        if !direct.is_empty() {
            return direct;
        }
        if let Some(object) = value.as_object() {
            for nested_key in ["servers", "nameservers", "resolvers", "ips"] {
                let nested = object
                    .get(nested_key)
                    .map(bounded_strings)
                    .unwrap_or_default();
                if !nested.is_empty() {
                    return nested;
                }
            }
        }
    }
    Vec::new()
}

/// Read the optional typed underlay observations from `network.interfaces[]`.
/// Only the provider kind, link state, interface name, and CIDR cross the
/// snapshot boundary. The array is bounded and duplicate provider entries are
/// ignored deterministically, so a newer writer cannot create an unbounded UI
/// surface or smuggle credentials into the read model.
fn interface_provider_facts(network: Option<&Value>) -> [Option<InterfaceProviderFacts>; 3] {
    let mut facts = [None, None, None];
    let Some(interfaces) = network
        .and_then(|network| network.get("interfaces"))
        .and_then(Value::as_array)
    else {
        return facts;
    };

    for interface in interfaces.iter().take(MAX_CONNECTIVITY_FACTS) {
        let Some(provider) = explicit_interface_provider(interface) else {
            continue;
        };
        let Some(index) = provider.index() else {
            continue;
        };
        if facts[index].is_some() {
            continue;
        }
        facts[index] = Some(InterfaceProviderFacts {
            state: interface_link_state(interface),
            interface: ["name", "interface", "ifname"]
                .iter()
                .find_map(|key| nonempty(interface, key)),
            cidr: ["cidr", "ip_cidr"]
                .iter()
                .find_map(|key| nonempty(interface, key)),
        });
    }
    facts
}

fn explicit_interface_provider(interface: &Value) -> Option<ConnectivityProvider> {
    ["provider", "kind", "type", "technology", "transport"]
        .iter()
        .filter_map(|key| nonempty(interface, key))
        .find_map(|value| match value.to_ascii_lowercase().as_str() {
            "wifi" | "wi-fi" | "wireless" => Some(ConnectivityProvider::Wifi),
            "ethernet" | "wired" => Some(ConnectivityProvider::Ethernet),
            "cellular" | "mobile" | "wwan" => Some(ConnectivityProvider::Cellular),
            _ => None,
        })
}

fn interface_link_state(interface: &Value) -> ProviderLinkState {
    if let Some(connected) = interface.get("connected").and_then(Value::as_bool) {
        return if connected {
            ProviderLinkState::Connected
        } else {
            ProviderLinkState::Disconnected
        };
    }
    let Some(state) = ["state", "status", "operstate"]
        .iter()
        .find_map(|key| nonempty(interface, key))
    else {
        return ProviderLinkState::Degraded;
    };
    match state.to_ascii_lowercase().as_str() {
        "connected" | "up" | "online" | "activated" | "ready" => ProviderLinkState::Connected,
        "disconnected" | "down" | "offline" | "unavailable" | "disabled" => {
            ProviderLinkState::Disconnected
        }
        _ => ProviderLinkState::Degraded,
    }
}

fn interface_provider_projection(
    provider: ConnectivityProvider,
    facts: Option<&InterfaceProviderFacts>,
) -> ConnectivityProviderProjection {
    let (availability, interface, cidr) = match facts {
        None => (
            match provider {
                ConnectivityProvider::Wifi => ConnectivityAvailability::Unavailable(
                    "No explicit Wi-Fi provider observation is published.",
                ),
                ConnectivityProvider::Ethernet => ConnectivityAvailability::Unavailable(
                    "No explicit Ethernet provider observation is published.",
                ),
                ConnectivityProvider::Cellular => ConnectivityAvailability::Unavailable(
                    "No explicit cellular provider observation is published.",
                ),
                ConnectivityProvider::Mesh | ConnectivityProvider::DnsLighthouse => {
                    ConnectivityAvailability::Unavailable(
                        "This provider is projected from the mesh-status overlay facts.",
                    )
                }
            },
            None,
            None,
        ),
        Some(facts) => (
            match (provider, facts.state) {
                (ConnectivityProvider::Wifi, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed Wi-Fi observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Wifi, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed Wi-Fi observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Wifi, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed Wi-Fi observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed Ethernet observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed Ethernet observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed Ethernet observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed cellular observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed cellular observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed cellular observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Mesh | ConnectivityProvider::DnsLighthouse, _) => {
                    ConnectivityAvailability::Degraded(
                        "This provider is projected from the mesh-status overlay facts.",
                    )
                }
            },
            facts.interface.clone(),
            facts.cidr.clone(),
        ),
    };
    ConnectivityProviderProjection {
        provider,
        availability,
        interface,
        cidr,
    }
}

impl NodeStatus {
    /// Fold the mesh-status snapshot into this node's status. `fallback_host` is the
    /// locally-resolved hostname, used only when the snapshot omits its `self`
    /// marker. A missing / garbage / non-mesh snapshot yields the honest unseen
    /// status (drives the connecting state), never a panic — mirroring the chrome
    /// bar's tolerance.
    fn project(snapshot: &str, fallback_host: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
            return Self::default();
        };
        let self_host = nonempty(&v, "self");
        let nodes = v.get("nodes").and_then(Value::as_array);
        // A real snapshot names at least `self` or a `nodes` array; anything else
        // (an empty object, an array, a fragment) reads as unseen.
        if self_host.is_none() && nodes.is_none() {
            return Self::default();
        }

        let hostname = self_host.unwrap_or_else(|| fallback_host.to_string());
        let network = v.get("network");
        let own = nodes.and_then(|arr| {
            arr.iter()
                .find(|n| n.get("hostname").and_then(Value::as_str) == Some(hostname.as_str()))
        });

        Self {
            seen: true,
            in_directory: own.is_some(),
            // Prefer this node's own directory-row overlay IP; fall back to the
            // network overview's locally-probed overlay address.
            overlay_ip: own
                .and_then(|n| nonempty(n, "overlay_ip"))
                .or_else(|| network.and_then(|n| nonempty(n, "overlay_ip"))),
            role: own.and_then(|n| nonempty(n, "role")),
            presence: own.and_then(|n| nonempty(n, "presence")),
            last_seen_ms: own
                .and_then(|n| n.get("last_seen_ms").and_then(Value::as_u64))
                .unwrap_or(0),
            version: own.and_then(|n| nonempty(n, "version")),
            update_available: own
                .and_then(|n| n.get("update").and_then(Value::as_bool))
                .unwrap_or(false),
            services: parse_services(own.and_then(|n| n.get("services"))),
            generated_ms: v.get("generated_ms").and_then(Value::as_u64).unwrap_or(0),
            latest_version: nonempty(&v, "latest_version"),
            peers_online: v.get("online").and_then(Value::as_u64).unwrap_or(0),
            peers_total: v.get("total").and_then(Value::as_u64).unwrap_or(0),
            leader: network.and_then(|n| nonempty(n, "leader")),
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            connectivity: ConnectivityFacts::from_network(network),
            hostname,
        }
    }

    fn connectivity_availability(&self) -> ConnectivityAvailability {
        if !self.seen {
            return ConnectivityAvailability::Unavailable(
                "Connectivity facts are unavailable until the mesh-status snapshot is read.",
            );
        }
        if self.connectivity.is_empty() {
            return ConnectivityAvailability::Unavailable(
                "No interface, route, provider, lighthouse, or DNS facts are published by mesh-status.",
            );
        }

        let providers = self.connectivity.provider_projection();
        let mesh_ready = matches!(
            providers[3].availability,
            ConnectivityAvailability::Available(_)
        );
        let underlay_ready = providers[..3].iter().any(|projection| {
            matches!(
                projection.availability,
                ConnectivityAvailability::Available(_)
            )
        }) && self.connectivity.default_route.is_some()
            && (!self.connectivity.lighthouses.is_empty()
                || !self.connectivity.dns_servers.is_empty());
        if mesh_ready || underlay_ready {
            return ConnectivityAvailability::Available(
                "A typed connectivity provider and mesh reachability or DNS facts are published.",
            );
        }
        ConnectivityAvailability::Degraded(
            "Only partial connectivity/provider facts are published; missing values are not inferred.",
        )
    }

    /// `true` when this node holds the mesh leader lease.
    fn is_leader(&self) -> bool {
        self.leader.as_deref() == Some(self.hostname.as_str())
    }

    /// A human "N ago" freshness for this node's last heartbeat, measured against
    /// the snapshot's own `generated_ms` clock. `None` when no heartbeat has been
    /// recorded yet.
    fn heartbeat_label(&self) -> Option<String> {
        if self.last_seen_ms == 0 {
            return None;
        }
        let secs = self.generated_ms.saturating_sub(self.last_seen_ms) / 1000;
        Some(if secs < 5 {
            "just now".to_string()
        } else if secs < 90 {
            format!("{secs}s ago")
        } else if secs < 90 * 60 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        })
    }

    /// Project the bounded, read-only capabilities this snapshot can support.
    ///
    /// The snapshot is an observation boundary, not a provider registry. A
    /// capability is therefore only `Available` when the corresponding fact is
    /// actually present; missing node-local providers are represented as typed
    /// unavailable states instead of becoming speculative controls.
    fn capability_projection(&self) -> [CapabilityProjection; CAPABILITY_CATALOG.len()] {
        CAPABILITY_CATALOG.map(|capability| CapabilityProjection {
            capability,
            availability: self.capability_availability(capability),
        })
    }

    fn capability_availability(&self, capability: NodeCapability) -> CapabilityAvailability {
        match capability {
            NodeCapability::MeshSnapshot => {
                if self.seen {
                    CapabilityAvailability::Available(
                        "Live world-readable mesh-status snapshot is present.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "The mesh-status snapshot has not arrived or is unreadable.",
                    )
                }
            }
            NodeCapability::NodeIdentity => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Identity is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.in_directory {
                    CapabilityAvailability::Available(
                        "Hostname, role, overlay address, and presence are live snapshot facts.",
                    )
                } else {
                    CapabilityAvailability::Degraded(
                        "The snapshot names this node, but its peer-directory row is not present.",
                    )
                }
            }
            NodeCapability::ServiceHealth => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Service health is unavailable until the mesh-status snapshot is read.",
                    )
                } else if !self.services.is_empty() {
                    CapabilityAvailability::Available(
                        "Published daemon health is available for the reported services.",
                    )
                } else if self.in_directory {
                    CapabilityAvailability::Degraded(
                        "This node is in the directory, but it has not reported service health.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "This node has no directory row from which to read service health.",
                    )
                }
            }
            NodeCapability::MeshContext => {
                if self.seen {
                    CapabilityAvailability::Available(
                        "Peer counts and leader state are read from the live snapshot.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "Mesh context is unavailable until the mesh-status snapshot is read.",
                    )
                }
            }
            NodeCapability::ConnectivityProviders => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Connectivity providers are unavailable until the mesh-status snapshot is read.",
                    )
                } else {
                    let providers = self.connectivity.provider_projection();
                    if providers.iter().any(|projection| {
                        matches!(
                            projection.availability,
                            ConnectivityAvailability::Available(_)
                        )
                    }) {
                        CapabilityAvailability::Available(
                            "Typed Wi-Fi, Ethernet, cellular, mesh, and DNS/lighthouse observations are projected read-only.",
                        )
                    } else if providers.iter().any(|projection| {
                        matches!(
                            projection.availability,
                            ConnectivityAvailability::Degraded(_)
                        )
                    }) {
                        CapabilityAvailability::Degraded(
                            "Connectivity providers are partially observed; missing backend facts remain unavailable.",
                        )
                    } else {
                        CapabilityAvailability::Unavailable(
                            "No typed connectivity provider observation is published by mesh-status.",
                        )
                    }
                }
            }
            NodeCapability::UpdateStatus => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Version posture is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.version.is_some() {
                    CapabilityAvailability::Available(
                        "Installed version and the mesh update target are read-only snapshot facts.",
                    )
                } else {
                    CapabilityAvailability::Degraded(
                        "The snapshot has no installed version for this node.",
                    )
                }
            }
            NodeCapability::LocalTelemetry => CapabilityAvailability::Unavailable(
                "CPU, memory, and disk telemetry is not published to this snapshot surface.",
            ),
            NodeCapability::MutationProviders => CapabilityAvailability::Unavailable(
                "No typed node-local mutation provider is advertised by mesh-status.",
            ),
        }
    }

    /// Project every mutation/provider action through the same fail-closed
    /// boundary. The snapshot can expose an update target or service health,
    /// but it cannot authorize a write and it does not name a provider that can
    /// execute one. Keeping these as typed rows makes that boundary visible and
    /// prevents a future button from silently turning a read model into a writer.
    fn action_projection(&self) -> [ActionProjection; ACTION_CATALOG.len()] {
        ACTION_CATALOG.map(|action| ActionProjection {
            action,
            availability: self.action_availability(action),
        })
    }

    fn action_availability(&self, action: ThisNodeAction) -> CapabilityAvailability {
        match action {
            ThisNodeAction::RestartService => {
                if self.services.is_empty() {
                    CapabilityAvailability::Unavailable(
                        "No reported service target is available for a restart request.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "Service health is read-only here; no typed service-control provider is connected.",
                    )
                }
            }
            ThisNodeAction::ApplyUpdate => {
                if self.update_available {
                    CapabilityAvailability::Unavailable(
                        "An update target is visible, but no typed update provider is connected.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "No pending update action is advertised by the live snapshot.",
                    )
                }
            }
            ThisNodeAction::ChangeConnectivity => {
                if self.connectivity.has_underlay_observation() {
                    CapabilityAvailability::Unavailable(
                        "Connectivity provider state is visible, but no typed NetworkManager/ModemManager mutation provider is connected.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "NetworkManager/ModemManager observation and mutation providers are not connected to This Node.",
                    )
                }
            }
            ThisNodeAction::ChangePowerProfile => CapabilityAvailability::Unavailable(
                "Power-profile mutation is not connected to a typed local provider.",
            ),
            ThisNodeAction::ConfigureHardware => CapabilityAvailability::Unavailable(
                "Hardware/OEM mutation is not connected to a typed, bounded provider.",
            ),
        }
    }
}

/// Fixed capability identifiers for the This Node read model. Keep this catalog
/// finite: a remote snapshot may describe services, but it cannot create an
/// unbounded set of UI capabilities or privileged operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeCapability {
    MeshSnapshot,
    NodeIdentity,
    ServiceHealth,
    MeshContext,
    ConnectivityProviders,
    UpdateStatus,
    LocalTelemetry,
    MutationProviders,
}

impl NodeCapability {
    const fn label(self) -> &'static str {
        match self {
            Self::MeshSnapshot => "Mesh status snapshot",
            Self::NodeIdentity => "Node identity",
            Self::ServiceHealth => "Service health",
            Self::MeshContext => "Mesh context",
            Self::ConnectivityProviders => "Connectivity providers",
            Self::UpdateStatus => "Version posture",
            Self::LocalTelemetry => "Node telemetry",
            Self::MutationProviders => "Mutation providers",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::MeshSnapshot => "Bounded source for this surface",
            Self::NodeIdentity => "Hostname, role, overlay, and presence",
            Self::ServiceHealth => "Published daemon health rows",
            Self::MeshContext => "Peer count and elected leader",
            Self::ConnectivityProviders => {
                "Wi-Fi, Ethernet, cellular, mesh, and DNS/lighthouse state"
            }
            Self::UpdateStatus => "Installed version and update target",
            Self::LocalTelemetry => "CPU, memory, and disk readings",
            Self::MutationProviders => "Typed local control backends",
        }
    }
}

const CAPABILITY_CATALOG: [NodeCapability; 8] = [
    NodeCapability::MeshSnapshot,
    NodeCapability::NodeIdentity,
    NodeCapability::ServiceHealth,
    NodeCapability::MeshContext,
    NodeCapability::ConnectivityProviders,
    NodeCapability::UpdateStatus,
    NodeCapability::LocalTelemetry,
    NodeCapability::MutationProviders,
];

/// A capability's honest state and the reason the UI can show to an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityAvailability {
    Available(&'static str),
    Degraded(&'static str),
    Unavailable(&'static str),
}

impl CapabilityAvailability {
    const fn tone(self) -> Color32 {
        match self {
            Self::Available(_) => Style::OK,
            Self::Degraded(_) => Style::WARN,
            Self::Unavailable(_) => Style::TEXT_DIM,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Available(_) => "available",
            Self::Degraded(_) => "degraded",
            Self::Unavailable(_) => "unavailable",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Available(detail) | Self::Degraded(detail) | Self::Unavailable(detail) => detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityProjection {
    capability: NodeCapability,
    availability: CapabilityAvailability,
}

/// Typed actions that this read-only snapshot may describe, but not execute.
/// The fixed list is intentionally small and provider-neutral; arbitrary verbs,
/// paths, shell commands, and guessed targets never enter the UI model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisNodeAction {
    RestartService,
    ApplyUpdate,
    ChangeConnectivity,
    ChangePowerProfile,
    ConfigureHardware,
}

impl ThisNodeAction {
    const fn label(self) -> &'static str {
        match self {
            Self::RestartService => "Restart a service",
            Self::ApplyUpdate => "Apply node update",
            Self::ChangeConnectivity => "Change connectivity",
            Self::ChangePowerProfile => "Change power profile",
            Self::ConfigureHardware => "Configure hardware",
        }
    }
}

const ACTION_CATALOG: [ThisNodeAction; 5] = [
    ThisNodeAction::RestartService,
    ThisNodeAction::ApplyUpdate,
    ThisNodeAction::ChangeConnectivity,
    ThisNodeAction::ChangePowerProfile,
    ThisNodeAction::ConfigureHardware,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionProjection {
    action: ThisNodeAction,
    availability: CapabilityAvailability,
}

/// Directory presence tier → tone: online is healthy, idle warns, offline is a
/// danger, anything else reads dim.
fn presence_tone(presence: &str) -> Color32 {
    match presence {
        "online" => Style::OK,
        "idle" => Style::WARN,
        "offline" => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

// ──────────────────────────── the ThisNode state ────────────────────────────

/// The This Node plane's live state: the projected status plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ThisNodeState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: NodeStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ThisNodeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: NodeStatus::default(),
            last_poll: None,
        }
    }
}

impl ThisNodeState {
    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a heartbeat / service flip
    /// surfaces without input. Cheap enough to call every frame — it self-gates. A
    /// missing / unreadable snapshot yields the unseen status, never a panic.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = read_bounded_snapshot(&self.snapshot_path).unwrap_or_default();
            self.status = NodeStatus::project(&snapshot, &self.local_host);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the plane's live content into `ui`.
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        show_status(ui, &self.status);
    }
}

/// The local hostname — `$HOSTNAME` → `/proc/sys/kernel/hostname` (what the
/// snapshot generator stamps as `self`) → `/etc/hostname` → `"localhost"`. Only a
/// fallback: the snapshot's own `self` marker is preferred.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ──────────────────────────── render ────────────────────────────

/// Render this node's live status: the connecting state before the first snapshot,
/// else the identity / services / mesh cards over an honest telemetry note.
fn show_status(ui: &mut egui::Ui, status: &NodeStatus) {
    if !status.seen {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, "Reading this node's status…");
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(
                "This node's role, overlay address, and daemon health fold from the \
                 world-readable mesh-status snapshot.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        show_capability_surface(ui, status);
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| show_identity(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Connectivity")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_connectivity(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Node services")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_services(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Mesh")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.group(|ui| show_mesh(ui, status));
            ui.add_space(Style::SP_S);

            // Honest boundary (§6/§7): node-local hardware telemetry isn't on this
            // world-readable surface — never fake a gauge.
            mde_egui::muted_note(
                ui,
                "Live CPU, memory, and disk aren't published to this surface — the shell \
                     reads the mesh directory, not node-local telemetry.",
            );
            show_capability_surface(ui, status);
        });
}

/// Render the bounded This Node capability/action surface. Capability rows are
/// projections of the live snapshot; action rows are deliberately disabled
/// because this state path has no provider or mutation writer. That distinction
/// is visible in the UI rather than being hidden behind a no-op button.
fn show_capability_surface(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Capabilities")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.group(|ui| {
        for projection in status.capability_projection() {
            show_capability_row(ui, projection);
        }
    });

    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Typed node actions")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.group(|ui| {
        for projection in status.action_projection() {
            show_action_row(ui, projection);
        }
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Actions stay disabled: this snapshot is read-only and advertises no \
             typed provider or authorization lane for local mutation.",
        );
    });
}

/// Render only the network facts explicitly published by mesh-status. This is
/// deliberately a read-only projection; missing interface, route, lighthouse,
/// or DNS values remain visible as unavailable instead of becoming guesses.
fn show_connectivity(ui: &mut egui::Ui, status: &NodeStatus) {
    // egui's zoom scales painted geometry after layout. Reduce the logical child
    // width by that same factor so a narrow large-text card wraps against the
    // painted viewport rather than laying out one line that later overflows it.
    let zoom = ui.ctx().zoom_factor().max(1.0);
    if zoom > 1.0 {
        ui.set_max_width(ui.available_width() / zoom);
    }
    let availability = status.connectivity_availability();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.add(
        egui::Label::new(
            RichText::new(availability.detail())
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );

    let facts = &status.connectivity;
    connectivity_field(ui, "Interface", facts.interface.as_deref());
    connectivity_field(ui, "CIDR", facts.cidr.as_deref());
    connectivity_field(ui, "Default route", facts.default_route.as_deref());
    let lighthouses = (!facts.lighthouses.is_empty()).then(|| facts.lighthouses.join(", "));
    connectivity_field(ui, "Lighthouses", lighthouses.as_deref());
    let dns_servers = (!facts.dns_servers.is_empty()).then(|| facts.dns_servers.join(", "));
    connectivity_field(ui, "DNS", dns_servers.as_deref());

    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Provider state")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.group(|ui| {
        for projection in facts.provider_projection() {
            show_connectivity_provider_row(ui, projection);
        }
    });

    ui.add(
        egui::Label::new(
            RichText::new("Read-only: no connectivity mutation provider is connected.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );
}

fn show_connectivity_provider_row(ui: &mut egui::Ui, projection: ConnectivityProviderProjection) {
    let availability = projection.availability;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(projection.provider.label())
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
    });
    ui.add(
        egui::Label::new(
            RichText::new(availability.detail())
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );
    if let Some(interface) = projection.interface.as_deref() {
        connectivity_field(ui, "Interface", Some(interface));
    }
    if let Some(cidr) = projection.cidr.as_deref() {
        connectivity_field(ui, "CIDR", Some(cidr));
    }
}

fn connectivity_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    let (value, tone) = value.map_or(("not published", Style::TEXT_DIM), |value| {
        (value, Style::TEXT)
    });
    // Connectivity values are provider output, not fixed copy: DNS and
    // lighthouse lists can be long, while the unavailable state is deliberately
    // verbose. Keep the label/value relationship accessible, but let the value
    // wrap inside the card instead of allowing the shared single-line `field`
    // primitive to paint past a narrow or large-text pane.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.add(egui::Label::new(RichText::new(value).color(tone).size(Style::SMALL)).wrap());
    });
}

fn show_capability_row(ui: &mut egui::Ui, projection: CapabilityProjection) {
    let availability = projection.availability;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(projection.capability.label())
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(projection.capability.description()).size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(availability.detail())
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
}

fn show_action_row(ui: &mut egui::Ui, projection: ActionProjection) {
    let availability = projection.availability;
    ui.horizontal_wrapped(|ui| {
        // This is intentionally disabled unconditionally. No current action has
        // a writer seam, and a future read-model change must not accidentally
        // turn `Available` into an unaudited mutation from this `&self` path.
        let response = ui.add_enabled(
            false,
            egui::Button::new(RichText::new(projection.action.label()).size(Style::SMALL)),
        );
        let response = response.on_hover_text(availability.detail());
        install_action_accessibility(
            ui.ctx(),
            response.id,
            response.rect,
            projection.action,
            availability,
        );
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(availability.detail()).size(Style::SMALL),
        );
    });
}

/// Keep the typed action boundary visible to assistive technology as well as to
/// sighted operators. These controls remain disabled because the snapshot is a
/// read-only observation; the value carries the exact provider/authorization
/// reason instead of making a screen reader guess from a dim button.
fn install_action_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    action: ThisNodeAction,
    availability: CapabilityAvailability,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(action.label());
        node.set_value(format!(
            "{}: {}",
            availability.word(),
            availability.detail()
        ));
        node.set_bounds(accesskit_rect(rect));
        node.set_disabled();
        node.clear_actions();
    });
}

fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The identity card: hostname + role + a leader marker, then overlay IP, cipher,
/// presence + heartbeat freshness, and the installed version + update hint.
fn show_identity(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&status.hostname)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        if let Some(role) = &status.role {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::ACCENT, RichText::new(role).size(Style::SMALL));
        }
        if status.is_leader() {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.colored_label(Style::OK, RichText::new("mesh leader").size(Style::SMALL));
        }
    });
    ui.add_space(Style::SP_XS);

    mde_egui::field(
        ui,
        "Overlay IP",
        status.overlay_ip.as_deref().unwrap_or("—"),
        if status.overlay_ip.is_some() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    if let Some(cipher) = &status.cipher {
        mde_egui::field(ui, "Tunnel cipher", cipher, Style::TEXT);
    }

    // Presence + heartbeat freshness.
    match &status.presence {
        Some(p) => {
            let tone = presence_tone(p);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Presence")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(tone, RichText::new(p).size(Style::SMALL));
                if let Some(age) = status.heartbeat_label() {
                    ui.add_space(Style::SP_S);
                    mde_egui::muted_note(ui, format!("\u{00B7} heartbeat {age}"));
                }
            });
        }
        None => mde_egui::field(
            ui,
            "Presence",
            "not yet in the peer directory",
            Style::TEXT_DIM,
        ),
    }

    // Installed version + update hint.
    match &status.version {
        Some(ver) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Version")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::TEXT, RichText::new(ver).size(Style::SMALL));
                if status.update_available {
                    ui.add_space(Style::SP_S);
                    let hint = status.latest_version.as_deref().map_or_else(
                        || "update available".to_string(),
                        |latest| format!("update available \u{2192} {latest}"),
                    );
                    ui.colored_label(Style::WARN, RichText::new(hint).size(Style::SMALL));
                }
            });
        }
        None => mde_egui::field(ui, "Version", "unknown", Style::TEXT_DIM),
    }
}

/// The node-services card: one health row per catalog daemon present in the
/// snapshot, or an honest "not yet reported" when this node hasn't published a
/// status record.
fn show_services(ui: &mut egui::Ui, status: &NodeStatus) {
    if status.services.is_empty() {
        let msg = if status.in_directory {
            "Service health not yet reported by this node."
        } else {
            "This node hasn't published a status record yet."
        };
        mde_egui::muted_note(ui, msg);
        return;
    }
    for (label, up) in &status.services {
        ui.horizontal(|ui| {
            let (dot, word, tone) = if *up {
                (Style::OK, "up", Style::TEXT_DIM)
            } else {
                (Style::TEXT_DIM, "down", Style::WARN)
            };
            ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(*label).color(Style::TEXT).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
        });
    }
}

/// The mesh-context card: the live peer count (online / total) and the elected
/// leader.
fn show_mesh(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Peers")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let tone = if status.peers_total == 0 {
            Style::TEXT_DIM
        } else if status.peers_online == status.peers_total {
            Style::OK
        } else {
            Style::WARN
        };
        ui.colored_label(
            tone,
            RichText::new(format!(
                "{}/{} live",
                status.peers_online, status.peers_total
            ))
            .size(Style::SMALL),
        );
    });
    match &status.leader {
        Some(leader) => mde_egui::field(ui, "Leader", leader, Style::TEXT),
        None => mde_egui::field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A faithful mesh-status snapshot: `self` + a `nodes` directory (this node plus
    /// two peers), the fleet counts, and the network overview — the exact shape
    /// `mesh-status-snapshot.sh` writes. `leader` names the mesh leader so both the
    /// is-leader and not-leader paths are reachable from one fixture.
    fn snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "latest_version": "11.2.0",
              "online": 2,
              "total": 3,
              "power_profile":{{"active":"balanced","available":["balanced","performance","power-saver"]}},
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "last_seen_ms":990000,"version":"11.1.0",
                  "services":{{"mackesd":true,"nebula":true,"sync":true,"bus":true,"dns":true,
                    "voice":false,"music":false,"kdc":true,"workbench":true}},
                  "role":"workstation","update":true}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "last_seen_ms":995000,"version":"11.2.0","services":{{}},
                  "role":"lighthouse","update":false}},
                {{"hostname":"peer-2","overlay_ip":"10.42.0.9","presence":"offline",
                  "last_seen_ms":100,"version":"11.1.0","services":{{}},
                  "role":"server","update":true}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":[],"default_gw":"",
                "gateway_endpoints":[],"lighthouse_ips":["10.42.0.1"],"cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    fn connectivity_snapshot(network: &str) -> String {
        format!(
            r#"{{"generated_ms":1000000,"self":"this-node",
              "nodes":[{{"hostname":"this-node","presence":"online"}}],
              "network":{network}}}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &NodeStatus) -> bool {
        renders_at(status, 960.0, 1.0)
    }

    fn renders_at(status: &NodeStatus, width: f32, zoom: f32) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.set_zoom_factor(zoom);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_status(ui, status));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn accesskit_nodes(status: &NodeStatus) -> Vec<egui::accesskit::Node> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.enable_accesskit();
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show_status(ui, status));
            },
        );
        out.platform_output
            .accesskit_update
            .expect("This Node accesskit update")
            .nodes
            .into_iter()
            .map(|(_, node)| node)
            .collect()
    }

    fn connectivity_text_bounds(
        status: &NodeStatus,
        width: f32,
        zoom: f32,
    ) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.set_zoom_factor(zoom);
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show_connectivity(ui, status));
            },
        );
        let mut bounds = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut bounds);
        }
        bounds
    }

    #[test]
    fn unseen_before_the_first_snapshot() {
        let s = NodeStatus::default();
        assert!(!s.seen, "the pre-read status is unseen (connecting)");
        // Even the connecting state is a full paint path, never a blank panel.
        assert!(
            renders(&s),
            "the connecting state produced no draw primitives"
        );
    }

    #[test]
    fn garbage_or_fragment_snapshot_stays_unseen() {
        for bad in ["", "not json", "{}", "[]", r#"{"network":{}}"#] {
            let s = NodeStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn project_folds_this_nodes_own_row_with_real_fields() {
        // The mesh leader is a peer (lh-01), so this node is NOT the leader.
        let s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen && s.in_directory, "this node's own row was found");

        // Identity — every field is the node's real directory reality (§7).
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.cipher.as_deref(), Some("AES-256-GCM"));

        // Presence + heartbeat: generated 1_000_000, last_seen 990_000 → 10s ago.
        assert_eq!(s.presence.as_deref(), Some("online"));
        assert_eq!(s.heartbeat_label().as_deref(), Some("10s ago"));

        // Version + the fleet-wide update hint (this node runs 11.1.0 < 11.2.0).
        assert_eq!(s.version.as_deref(), Some("11.1.0"));
        assert!(s.update_available);
        assert_eq!(s.latest_version.as_deref(), Some("11.2.0"));

        // Node services parse in catalog order; the map's real up/down is kept.
        assert_eq!(
            s.services.len(),
            SERVICE_CATALOG.len(),
            "all 9 daemons present"
        );
        assert_eq!(s.services[0], ("Mesh daemon", true));
        assert!(s.services.iter().any(|(l, up)| *l == "Voice HUD" && !*up));

        // Mesh context — the live peer count + the elected leader.
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert!(!s.is_leader(), "the leader is a peer, not this node");

        // And the whole live panel tessellates.
        assert!(
            renders(&s),
            "the live ThisNode panel produced no draw primitives"
        );
    }

    #[test]
    fn connectivity_fixture_projects_and_renders_published_facts() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16",
                   "default_gw":"192.168.1.1","lighthouse_ips":["10.42.0.1"],
                   "dns_servers":["10.42.0.1","1.1.1.1"]}"#,
            ),
            "fallback",
        );

        assert_eq!(s.connectivity.interface.as_deref(), Some("nebula1"));
        assert_eq!(s.connectivity.cidr.as_deref(), Some("10.42.0.7/16"));
        assert_eq!(s.connectivity.default_route.as_deref(), Some("192.168.1.1"));
        assert_eq!(s.connectivity.lighthouses, vec!["10.42.0.1"]);
        assert_eq!(s.connectivity.dns_servers, vec!["10.42.0.1", "1.1.1.1"]);
        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Available(_)
        ));
        let providers = s.connectivity.provider_projection();
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Mesh)
                .expect("mesh provider")
                .availability,
            ConnectivityAvailability::Available(_)
        ));
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::DnsLighthouse)
                .expect("DNS/lighthouse provider")
                .availability,
            ConnectivityAvailability::Available(_)
        ));
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Wifi)
                .expect("Wi-Fi provider")
                .availability,
            ConnectivityAvailability::Unavailable(_)
        ));
        assert!(matches!(
            s.capability_projection()
                .iter()
                .find(|projection| {
                    projection.capability == NodeCapability::ConnectivityProviders
                })
                .expect("connectivity provider capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(renders(&s), "published connectivity facts must render");
    }

    #[test]
    fn explicit_underlay_provider_states_are_typed_and_credentials_are_not_projected() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{
                    "interfaces":[
                      {"kind":"wifi","name":"wlan0","state":"connected",
                       "cidr":"192.0.2.20/24","ssid":"private-network","psk":"do-not-store"},
                      {"type":"ethernet","ifname":"enp1s0","state":"down"},
                      {"provider":"cellular","interface":"wwan0","status":"connecting",
                       "apn":"private-apn","password":"do-not-store"}
                    ]
                }"#,
            ),
            "fallback",
        );
        let providers = s.connectivity.provider_projection();

        let wifi = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Wifi)
            .expect("Wi-Fi provider");
        assert!(matches!(
            wifi.availability,
            ConnectivityAvailability::Available(_)
        ));
        assert_eq!(wifi.interface.as_deref(), Some("wlan0"));
        assert_eq!(wifi.cidr.as_deref(), Some("192.0.2.20/24"));

        let ethernet = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Ethernet)
            .expect("Ethernet provider");
        assert!(matches!(
            ethernet.availability,
            ConnectivityAvailability::Unavailable(_)
        ));
        assert_eq!(ethernet.interface.as_deref(), Some("enp1s0"));

        let cellular = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Cellular)
            .expect("cellular provider");
        assert!(matches!(
            cellular.availability,
            ConnectivityAvailability::Degraded(_)
        ));
        assert_eq!(cellular.interface.as_deref(), Some("wwan0"));

        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        let change = s
            .action_projection()
            .into_iter()
            .find(|projection| projection.action == ThisNodeAction::ChangeConnectivity)
            .expect("connectivity action");
        assert!(change.availability.detail().contains("provider state"));

        let debug = format!("{s:?}");
        assert!(!debug.contains("private-network"));
        assert!(!debug.contains("do-not-store"));
        assert!(!debug.contains("private-apn"));
        assert!(renders(&s), "typed underlay provider rows must render");
    }

    #[test]
    fn connectivity_absence_and_partial_facts_render_honest_states() {
        let absent = NodeStatus::project(&connectivity_snapshot(r#"{}"#), "fallback");
        assert!(matches!(
            absent.connectivity_availability(),
            ConnectivityAvailability::Unavailable(_)
        ));
        assert!(renders(&absent), "absent connectivity state must render");

        let partial = NodeStatus::project(
            &connectivity_snapshot(r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16"}"#),
            "fallback",
        );
        assert!(matches!(
            partial.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        assert!(renders(&partial), "partial connectivity state must render");
    }

    #[test]
    fn connectivity_fields_reflow_inside_a_narrow_large_text_card() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16",
                   "default_gw":"192.168.1.1",
                   "lighthouse_ips":["10.42.0.1","10.42.0.2","10.42.0.3","10.42.0.4"],
                   "dns_servers":["10.42.0.1","1.1.1.1","9.9.9.9"]}"#,
            ),
            "fallback",
        );
        let bounds = connectivity_text_bounds(&s, 240.0, 1.5);
        assert!(!bounds.is_empty(), "the connectivity card must paint text");
        for (text, rect) in bounds {
            assert!(
                rect.left() >= -0.5 && rect.right() <= 240.0 * 1.5 + 0.5,
                "{text:?} escaped the narrow card: {rect:?}"
            );
        }
    }

    #[test]
    fn status_card_keeps_unavailable_provider_states_visible_at_small_sizes() {
        let unseen = NodeStatus::default();
        for (width, zoom) in [(240.0, 1.0), (320.0, 1.5)] {
            assert!(
                renders_at(&unseen, width, zoom),
                "the unavailable status must remain painted at {width}x{zoom}"
            );
        }
        assert!(
            matches!(
                unseen.connectivity_availability(),
                ConnectivityAvailability::Unavailable(_)
            ),
            "missing hardware/provider facts remain explicitly unavailable"
        );
    }

    #[test]
    fn capability_projection_is_fixed_and_snapshot_driven() {
        let unseen = NodeStatus::default();
        let unseen_caps = unseen.capability_projection();
        assert_eq!(unseen_caps.len(), CAPABILITY_CATALOG.len());
        assert!(matches!(
            unseen_caps[0].availability,
            CapabilityAvailability::Unavailable(_)
        ));
        assert!(matches!(
            unseen_caps[5].availability,
            CapabilityAvailability::Unavailable(_)
        ));

        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let caps = live.capability_projection();
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::MeshSnapshot)
                .expect("mesh snapshot capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::ServiceHealth)
                .expect("service health capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::LocalTelemetry)
                .expect("local telemetry capability")
                .availability,
            CapabilityAvailability::Unavailable(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::MutationProviders)
                .expect("mutation provider capability")
                .availability,
            CapabilityAvailability::Unavailable(_)
        ));
    }

    #[test]
    fn typed_mutation_actions_remain_fail_closed_with_live_facts() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let actions = live.action_projection();
        assert_eq!(actions.len(), ACTION_CATALOG.len());
        assert!(actions.iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Unavailable(_)
        )));

        let update = actions
            .iter()
            .find(|projection| projection.action == ThisNodeAction::ApplyUpdate)
            .expect("update action");
        assert!(update.availability.detail().contains("update target"));
        assert!(actions
            .iter()
            .any(|projection| projection.action == ThisNodeAction::ChangeConnectivity));
    }

    #[test]
    fn typed_actions_export_disabled_accessibility_reasons() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let nodes = accesskit_nodes(&live);
        let restart = nodes
            .iter()
            .find(|node| node.label() == Some("Restart a service"))
            .expect("This Node action should be discoverable to assistive technology");

        assert_eq!(restart.role(), egui::accesskit::Role::Button);
        assert!(restart.is_disabled());
        assert!(!restart.supports_action(egui::accesskit::Action::Click));
        assert!(restart
            .value()
            .expect("disabled action reason")
            .contains("no typed service-control provider"));
    }

    #[test]
    fn leader_row_identifies_this_node_when_it_holds_the_lease() {
        let s = NodeStatus::project(&snapshot("this-node", "this-node"), "fallback");
        assert!(s.is_leader(), "this node holds the leader lease");
        assert!(renders(&s));
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // identifies this node by the locally-resolved hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "last_seen_ms":1,"role":"workstation","services":{"mackesd":true}}],
            "network":{"leader":"","cipher":""}}"#;
        let s = NodeStatus::project(snap, "this-node");
        assert!(s.seen && s.in_directory);
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
    }

    #[test]
    fn seen_but_not_in_directory_shows_identity_without_fabricating_a_row() {
        // The snapshot is readable, but this node's heartbeat record isn't in the
        // directory yet: identity + mesh context still render off `self`/`network`,
        // and the per-node fields honestly say so (never a fake value, §7).
        let s = NodeStatus::project(&snapshot("ghost-node", "lh-01"), "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(!s.in_directory, "no matching directory row for this node");
        assert_eq!(s.hostname, "ghost-node");
        // Network-sourced identity is still available.
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        // Per-node fields are honestly empty, not fabricated.
        assert!(s.role.is_none());
        assert!(s.presence.is_none());
        assert!(s.services.is_empty());
        assert!(s.heartbeat_label().is_none());
        // The honest-partial panel still fully paints.
        assert!(renders(&s));
    }

    #[test]
    fn heartbeat_label_is_none_without_a_recorded_beat() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        s.last_seen_ms = 0;
        assert!(
            s.heartbeat_label().is_none(),
            "no heartbeat recorded → no freshness claimed"
        );
    }

    #[test]
    fn thisnode_state_defaults_to_the_snapshot_path_unseen() {
        let st = ThisNodeState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
    }

    #[test]
    fn bounded_snapshot_reader_rejects_hostile_files_before_projection() {
        let dir = tempfile::tempdir().expect("snapshot tempdir");
        let valid = dir.path().join("valid.json");
        std::fs::write(&valid, snapshot("this-node", "lh-01")).expect("write valid snapshot");
        assert!(read_bounded_snapshot(&valid).is_some());

        let invalid_utf8 = dir.path().join("invalid.json");
        std::fs::write(&invalid_utf8, [0xff, 0xfe]).expect("write invalid snapshot");
        assert!(read_bounded_snapshot(&invalid_utf8).is_none());

        let oversized = dir.path().join("oversized.json");
        std::fs::write(&oversized, vec![b'{'; MAX_SNAPSHOT_BYTES + 1])
            .expect("write oversized snapshot");
        assert!(read_bounded_snapshot(&oversized).is_none());

        let special = dir.path().join("special.json");
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;
            let _socket = UnixListener::bind(&special).expect("create socket");
            assert!(read_bounded_snapshot(&special).is_none());
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir(&special).expect("create special fixture");
            assert!(read_bounded_snapshot(&special).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_snapshot_reader_rejects_final_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("snapshot tempdir");
        let target = dir.path().join("outside.json");
        let link = dir.path().join("mesh-status.json");
        std::fs::write(&target, snapshot("outside", "lh-01")).expect("write target snapshot");
        symlink(&target, &link).expect("create final symlink");
        assert!(read_bounded_snapshot(&link).is_none());
    }
}
