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
use std::time::SystemTime;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{DenseList, Style};

use crate::this_node_catalog::{Section, SectionGroup};

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar reads
/// (the desktop user can't read the root-only replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a heartbeat, a service flip, or a role change surfaces within
/// this window. Matches the chrome bar + the Fleet datacenter poll; the read is a
/// cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// The snapshot writer normally runs every ~30 seconds. Treat a readable but
/// older snapshot as degraded too: a provider that stopped updating must not
/// look current merely because the last file remains parseable.
const MAX_SNAPSHOT_AGE_MS: u64 = 90_000;

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

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn snapshot_age_ms(generated_ms: u64, now_ms: u64) -> Option<u64> {
    (generated_ms > 0).then(|| now_ms.saturating_sub(generated_ms))
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
    /// The directory's explicit `(online, total)` peer counts.
    ///
    /// This stays absent when either field is missing or when the writer emits
    /// an impossible pair. A missing pair must not become a fabricated `0/0`
    /// live count in the hardware center.
    peer_counts: Option<(u64, u64)>,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// Whether the last valid projection is retained after a provider read
    /// failure. Retained values remain diagnostic-only until refreshed.
    stale: bool,
    /// Bounded explanation for the retained stale projection.
    stale_reason: Option<String>,
    /// The Nebula tunnel cipher label, when nebula is up.
    cipher: Option<String>,
    /// Read-only interface, route, lighthouse, and resolver facts published by
    /// the network section of mesh-status.
    connectivity: ConnectivityFacts,
    /// Credential-free power-profile observation from the root snapshot writer.
    power_profile: PowerProfileFacts,
    /// Credential-free PipeWire/Pulse/WirePlumber observation from the node
    /// status writer. Missing fields remain unknown rather than becoming a
    /// fabricated healthy audio stack.
    audio: AudioFacts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PowerProfileFacts {
    active: Option<String>,
    available: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AudioFacts {
    pulse_available: Option<bool>,
    pipewire_graph: Option<bool>,
    wireplumber_policy: Option<bool>,
    alsa_devices: Option<u64>,
    playback: Option<bool>,
    capture: Option<bool>,
    recovery: Option<String>,
}

fn audio_facts(value: Option<&Value>) -> AudioFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return AudioFacts::default();
    };
    let component_available = |flat_key: &str, typed_key: &str| {
        object
            .get(flat_key)
            .and_then(Value::as_bool)
            .or_else(|| {
                object
                    .get(typed_key)
                    .and_then(Value::as_object)
                    .and_then(|component| component.get("availability"))
                    .and_then(Value::as_str)
                    .map(|state| state == "available")
            })
    };
    let typed_pulse = object
        .get("pulse_audio_compatibility")
        .and_then(Value::as_object);
    let recovery = object
        .get("recovery")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .map(str::to_owned);
    let recovery = recovery.or_else(|| {
        object
            .get("recovery")
            .and_then(Value::as_object)
            .and_then(|component| component.get("availability"))
            .and_then(Value::as_str)
            .filter(|state| *state != "available")
            .map(|_| "Audio recovery provider is unavailable; refresh the snapshot.".to_owned())
    });
    AudioFacts {
        pulse_available: object
            .get("pulse_available")
            .and_then(Value::as_bool)
            .or_else(|| {
                typed_pulse
                    .and_then(|pulse| pulse.get("compatibility"))
                    .and_then(Value::as_str)
                    .map(|compatibility| compatibility == "compatible")
            }),
        pipewire_graph: component_available("pipewire_graph", "pipewire_graph"),
        wireplumber_policy: component_available("wireplumber_policy", "wireplumber_policy"),
        alsa_devices: object
            .get("alsa_devices")
            .and_then(Value::as_u64)
            .or_else(|| {
                object
                    .get("alsa_ucm_discovery")
                    .and_then(Value::as_object)
                    .and_then(|component| component.get("observed_items"))
                    .and_then(Value::as_u64)
            })
            .filter(|value| *value <= 256),
        playback: component_available("playback", "playback"),
        capture: component_available("capture", "capture"),
        recovery,
    }
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

fn power_profile_facts(value: Option<&Value>) -> PowerProfileFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return PowerProfileFacts::default();
    };
    let active = object
        .get("active")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
        .map(str::to_owned);
    let mut available = object
        .get("available")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    available.sort();
    available.dedup();
    PowerProfileFacts { active, available }
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
                recovery: match self.mesh_provider_availability() {
                    ConnectivityAvailability::Available(_) => ProviderRecovery::None,
                    ConnectivityAvailability::Degraded(_) => ProviderRecovery::RefreshSnapshot,
                    ConnectivityAvailability::Unavailable(_) => ProviderRecovery::AwaitProvider,
                },
                interface: self.interface.clone(),
                cidr: self.cidr.clone(),
            },
            ConnectivityProvider::DnsLighthouse => ConnectivityProviderProjection {
                provider,
                availability: self.dns_lighthouse_availability(),
                recovery: match self.dns_lighthouse_availability() {
                    ConnectivityAvailability::Available(_) => ProviderRecovery::None,
                    ConnectivityAvailability::Degraded(_) => ProviderRecovery::RefreshSnapshot,
                    ConnectivityAvailability::Unavailable(_) => ProviderRecovery::AwaitProvider,
                },
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
    recovery: ProviderRecovery,
    interface: Option<String>,
    cidr: Option<String>,
}

/// The only recovery actions this read-only provider boundary may advertise.
///
/// These are recovery guidance, not mutation verbs: the snapshot can request a
/// bounded re-read, but it cannot authorize reconnecting a link, changing a
/// profile, or supplying credentials. Keeping the distinction typed prevents a
/// future provider row from turning an unavailable observation into an implicit
/// network write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRecovery {
    None,
    RefreshSnapshot,
    AwaitProvider,
}

impl ProviderRecovery {
    const fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::RefreshSnapshot => Some("Recovery: refresh provider snapshot"),
            Self::AwaitProvider => Some("Recovery: await provider publication"),
        }
    }
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
    let (availability, interface, cidr, recovery) = match facts {
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
            ProviderRecovery::AwaitProvider,
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
            match facts.state {
                ProviderLinkState::Connected => ProviderRecovery::None,
                ProviderLinkState::Degraded
                | ProviderLinkState::Disconnected => ProviderRecovery::RefreshSnapshot,
            },
        ),
    };
    ConnectivityProviderProjection {
        provider,
        availability,
        recovery,
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
        let peer_counts = match (
            v.get("online").and_then(Value::as_u64),
            v.get("total").and_then(Value::as_u64),
        ) {
            (Some(online), Some(total)) if online <= total => Some((online, total)),
            _ => None,
        };
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
            peer_counts,
            leader: network.and_then(|n| nonempty(n, "leader")),
            stale: false,
            stale_reason: None,
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            connectivity: ConnectivityFacts::from_network(network),
            power_profile: power_profile_facts(v.get("power_profile")),
            audio: audio_facts(v.get("audio")),
            hostname,
        }
    }

    fn mark_stale(&mut self, reason: impl Into<String>) {
        self.stale = true;
        self.stale_reason = Some(reason.into());
    }

    fn connectivity_availability(&self) -> ConnectivityAvailability {
        if !self.seen {
            return ConnectivityAvailability::Unavailable(
                "Connectivity facts are unavailable until the mesh-status snapshot is read.",
            );
        }
        if self.stale {
            return ConnectivityAvailability::Degraded(
                "Connectivity facts are retained from a stale snapshot; refresh before relying on them.",
            );
        }
        if self.connectivity.is_empty() {
            return ConnectivityAvailability::Unavailable(
                "No interface, route, provider, lighthouse, or DNS facts are published by mesh-status.",
            );
        }

        let providers = self.provider_projection();
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

    /// Apply snapshot freshness to each provider row. A retained projection is
    /// never allowed to look freshly actionable merely because its last known
    /// link state was connected.
    fn provider_projection(&self) -> [ConnectivityProviderProjection; 5] {
        let mut projections = self.connectivity.provider_projection();
        if self.stale {
            for projection in &mut projections {
                projection.availability = stale_provider_availability(projection.availability);
                projection.recovery = ProviderRecovery::RefreshSnapshot;
            }
        }
        projections
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

    /// Fold the existing read model into the fixed eight-section dashboard.
    /// There is intentionally no local score calculation here: the top rail's
    /// mesh-status authority remains the source of health facts, while this
    /// method only assigns a presentational state to each governed destination.
    fn health_dashboard(&self) -> HealthDashboard {
        let states = Section::ALL.map(|section| (section, self.section_health(section)));
        let overall = if self.stale {
            SectionHealth::Stale
        } else if states
            .iter()
            .any(|(_, health)| *health == SectionHealth::Unhealthy)
        {
            SectionHealth::Unhealthy
        } else if states
            .iter()
            .any(|(_, health)| *health == SectionHealth::Attention)
        {
            SectionHealth::Attention
        } else if states
            .iter()
            .all(|(_, health)| *health == SectionHealth::Unavailable)
        {
            SectionHealth::Unavailable
        } else if states
            .iter()
            .any(|(_, health)| *health == SectionHealth::Unavailable)
        {
            SectionHealth::Attention
        } else {
            SectionHealth::Healthy
        };
        HealthDashboard { states, overall }
    }

    fn section_health(&self, section: Section) -> SectionHealth {
        if self.stale {
            return SectionHealth::Stale;
        }
        if !self.seen {
            return SectionHealth::Unavailable;
        }

        match section {
            Section::Overview => {
                if self.presence.as_deref() == Some("offline")
                    || self.services.iter().any(|(_, up)| !up)
                {
                    SectionHealth::Unhealthy
                } else if self.presence.as_deref() == Some("idle")
                    || self.update_available
                    || !self.in_directory
                {
                    SectionHealth::Attention
                } else if self.services.is_empty() {
                    SectionHealth::Unavailable
                } else {
                    SectionHealth::Healthy
                }
            }
            Section::Connectivity => match self.connectivity_availability() {
                ConnectivityAvailability::Available(_) => SectionHealth::Healthy,
                ConnectivityAvailability::Degraded(_) => SectionHealth::Attention,
                ConnectivityAvailability::Unavailable(_) => SectionHealth::Unavailable,
            },
            Section::DisplaySound => {
                if self.audio == AudioFacts::default() {
                    SectionHealth::Unavailable
                } else if [
                    self.audio.pulse_available,
                    self.audio.pipewire_graph,
                    self.audio.wireplumber_policy,
                    self.audio.playback,
                    self.audio.capture,
                ]
                .into_iter()
                .any(|state| state == Some(false))
                {
                    SectionHealth::Attention
                } else {
                    SectionHealth::Healthy
                }
            }
            Section::Input | Section::Personalization => {
                if section.unavailable_reason().is_some() {
                    SectionHealth::Unavailable
                } else {
                    SectionHealth::Healthy
                }
            }
            Section::PowerPerformance => {
                if self.power_profile.active.is_some() && !self.power_profile.available.is_empty() {
                    SectionHealth::Healthy
                } else if !self.power_profile.available.is_empty() {
                    SectionHealth::Attention
                } else {
                    SectionHealth::Unavailable
                }
            }
            Section::Hardware => {
                // Hardware telemetry and mutation providers are not published
                // by this read boundary. Do not turn the absence into a fake
                // healthy device tree or an invented device count.
                SectionHealth::Unavailable
            }
            Section::MeshSystem => {
                let mesh = self
                    .capability_projection()
                    .into_iter()
                    .find(|projection| projection.capability == NodeCapability::MeshContext)
                    .map(|projection| projection.availability);
                if self.services.iter().any(|(_, up)| !up) {
                    SectionHealth::Unhealthy
                } else if matches!(mesh, Some(CapabilityAvailability::Degraded(_)))
                    || matches!(mesh, Some(CapabilityAvailability::Available(_)))
                        && self.services.is_empty()
                {
                    SectionHealth::Attention
                } else if matches!(mesh, Some(CapabilityAvailability::Available(_)))
                    && !self.services.is_empty()
                {
                    SectionHealth::Healthy
                } else {
                    SectionHealth::Unavailable
                }
            }
        }
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
        if self.stale {
            return CapabilityAvailability::Degraded(
                "The last valid provider projection is stale; refresh before relying on this state.",
            );
        }
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
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Mesh context is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.peer_counts.is_some() && self.leader.is_some() {
                    CapabilityAvailability::Available(
                        "Peer counts and leader state are read from the live snapshot.",
                    )
                } else if self.peer_counts.is_some() || self.leader.is_some() {
                    CapabilityAvailability::Degraded(
                        "The snapshot exposes only part of mesh context; missing facts remain unavailable.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "The snapshot has no peer counts or elected leader to report.",
                    )
                }
            }
            NodeCapability::ConnectivityProviders => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Connectivity providers are unavailable until the mesh-status snapshot is read.",
                    )
                } else {
                    let providers = self.provider_projection();
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
        if self.stale {
            return CapabilityAvailability::Degraded(
                "The provider projection is stale; refresh before requesting an action.",
            );
        }
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
            ThisNodeAction::ChangePowerProfile => {
                CapabilityAvailability::Unavailable(if self.power_profile.available.is_empty() {
                    "No power-profile provider observation is published by mesh-status."
                } else {
                    "Power-profile state is observed read-only; typed local mutation still requires the System provider authorization path."
                })
            }
            ThisNodeAction::ConfigureHardware => CapabilityAvailability::Unavailable(
                "Hardware/OEM mutation is not connected to a typed, bounded provider.",
            ),
        }
    }
}

/// Retained provider facts must not continue to announce current availability
/// after the source snapshot goes stale. Preserve an actually-unavailable state
/// (there is still no observation to degrade), but make every observed state
/// visibly stale in the row itself rather than relying on the banner above it.
fn stale_provider_availability(
    availability: ConnectivityAvailability,
) -> ConnectivityAvailability {
    match availability {
        ConnectivityAvailability::Available(_) | ConnectivityAvailability::Degraded(_) => {
            ConnectivityAvailability::Degraded(
                "The last provider observation is stale; refresh before relying on it.",
            )
        }
        ConnectivityAvailability::Unavailable(detail) => {
            ConnectivityAvailability::Unavailable(detail)
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

/// Presentation severity for a governed This Node section.
///
/// This is deliberately not a second numeric health score. The dashboard folds
/// the same snapshot/provider observations that the top rail already exposes:
/// explicit down/offline facts become unhealthy, partial facts become attention,
/// missing facts remain unavailable, and a retained projection remains stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionHealth {
    Healthy,
    Attention,
    Unhealthy,
    Unavailable,
    Stale,
}

impl SectionHealth {
    const fn label(self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Attention => "Attention",
            Self::Unhealthy => "Unhealthy",
            Self::Unavailable => "Unavailable",
            Self::Stale => "Stale",
        }
    }

    const fn glyph(self) -> &'static str {
        match self {
            Self::Healthy => "●",
            Self::Attention => "▲",
            Self::Unhealthy => "■",
            Self::Unavailable => "—",
            Self::Stale => "◌",
        }
    }

    const fn tone(self) -> Color32 {
        match self {
            Self::Healthy => Style::OK,
            Self::Attention => Style::WARN,
            Self::Unhealthy => Style::DANGER,
            Self::Unavailable | Self::Stale => Style::TEXT_DIM,
        }
    }

    const fn is_alert(self) -> bool {
        matches!(self, Self::Attention | Self::Unhealthy | Self::Stale)
    }
}

/// The fixed health projection used by the landing view and its tree.
///
/// Keeping the section/state pairs together makes it impossible for the
/// dashboard and hierarchy to silently disagree about which governed section
/// is affected. `states` is populated from `Section::ALL`, not provider data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HealthDashboard {
    states: [(Section, SectionHealth); 8],
    overall: SectionHealth,
}

impl HealthDashboard {
    fn health(self, section: Section) -> SectionHealth {
        self.states
            .iter()
            .find_map(|(candidate, health)| (*candidate == section).then_some(*health))
            .unwrap_or(SectionHealth::Unavailable)
    }

    fn count(self, health: SectionHealth) -> usize {
        self.states
            .iter()
            .filter(|(_, candidate)| *candidate == health)
            .count()
    }

    fn alert_count(self) -> usize {
        self.states
            .iter()
            .filter(|(_, health)| health.is_alert())
            .count()
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
    /// missing / unreadable snapshot retains a previously valid projection as
    /// stale; before the first valid snapshot it yields the unseen status.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            match read_bounded_snapshot(&self.snapshot_path) {
                Some(snapshot) => {
                    let projected = NodeStatus::project(&snapshot, &self.local_host);
                    if projected.seen || !self.status.seen {
                        self.status = projected;
                        if let Some(age_ms) =
                            snapshot_age_ms(self.status.generated_ms, unix_epoch_ms())
                        {
                            if age_ms > MAX_SNAPSHOT_AGE_MS {
                                self.status.mark_stale(format!(
                                    "The mesh-status snapshot is {} seconds old; retained values may be outdated.",
                                    age_ms / 1_000
                                ));
                            }
                        }
                    } else {
                        self.status.mark_stale(
                            "The latest mesh-status snapshot was malformed; retained values are stale.",
                        );
                    }
                }
                None if self.status.seen => self.status.mark_stale(
                    "The mesh-status provider is unavailable; retained values are stale.",
                ),
                None => self.status = NodeStatus::default(),
            }
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the plane's live content into `ui`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Provider snapshot")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            if ui.button("Refresh now").clicked() {
                self.last_poll = None;
                self.poll(ui.ctx());
            }
        });
        ui.add_space(Style::SP_XS);
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
        let dashboard = status.health_dashboard();
        show_health_dashboard(ui, status, dashboard);
        ui.add_space(Style::SP_S);
        show_section_hierarchy(ui, status, dashboard);
        show_capability_surface(ui, status);
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if status.stale {
                mde_egui::card().show(ui, |ui| {
                    ui.colored_label(Style::WARN, "This Node status is stale");
                    ui.label(
                        RichText::new(
                            status
                                .stale_reason
                                .as_deref()
                                .unwrap_or("The provider did not return a fresh snapshot."),
                        )
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                    );
                });
                ui.add_space(Style::SP_S);
            }
            let dashboard = status.health_dashboard();
            show_health_dashboard(ui, status, dashboard);
            ui.add_space(Style::SP_S);
            show_section_hierarchy(ui, status, dashboard);
            ui.add_space(Style::SP_S);
            mde_egui::card().show(ui, |ui| show_identity(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Connectivity")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            mde_egui::card().show(ui, |ui| show_connectivity(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Display & sound")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            mde_egui::card().show(ui, |ui| show_audio(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Node services")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            mde_egui::card().show(ui, |ui| show_services(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Mesh")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            mde_egui::card().show(ui, |ui| show_mesh(ui, status));
            ui.add_space(Style::SP_S);

            ui.label(
                RichText::new("Power & performance")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            mde_egui::card().show(ui, |ui| show_power_profile(ui, status));
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

fn show_health_dashboard(ui: &mut egui::Ui, status: &NodeStatus, dashboard: HealthDashboard) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Health dashboard").strong());
            ui.colored_label(
                dashboard.overall.tone(),
                RichText::new(format!(
                    "{} {}",
                    dashboard.overall.glyph(),
                    dashboard.overall.label()
                ))
                .strong(),
            );
        });

        if dashboard.overall == SectionHealth::Healthy {
            ui.label(
                RichText::new("All systems operational")
                    .color(Style::OK)
                    .size(Style::BODY),
            );
            ui.label(
                RichText::new(
                    "The current mesh-status projection reports healthy facts for every governed section.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        } else if dashboard.alert_count() > 0 {
            let heading = if dashboard.count(SectionHealth::Unhealthy) > 0 {
                "Critical alerts"
            } else if dashboard.overall == SectionHealth::Stale {
                "Status requires refresh"
            } else {
                "Attention needed"
            };
            ui.label(RichText::new(heading).color(dashboard.overall.tone()).strong());
            let mut rows = DenseList::new();
            for (section, health) in dashboard.states {
                if !health.is_alert() {
                    continue;
                }
                rows.row(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            health.tone(),
                            RichText::new(format!("{} {}", health.glyph(), health.label()))
                                .size(Style::SMALL),
                        );
                        ui.label(RichText::new(section.label()).strong().size(Style::SMALL));
                        ui.label(
                            RichText::new(section_health_detail(section, health, status))
                                .color(Style::TEXT_DIM)
                                .size(Style::SMALL),
                        );
                    });
                });
            }
        } else {
            ui.colored_label(
                Style::TEXT_DIM,
                format!(
                    "Health data is unavailable for {} governed sections.",
                    dashboard.count(SectionHealth::Unavailable)
                ),
            );
            ui.label(
                RichText::new(
                    "This Node will show provider-backed health when the shared snapshot publishes it.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }

        if dashboard.count(SectionHealth::Unavailable) > 0
            && dashboard.overall != SectionHealth::Unavailable
        {
            ui.label(
                RichText::new(format!(
                    "{} sections have no published provider observation.",
                    dashboard.count(SectionHealth::Unavailable)
                ))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
    });
}

fn show_section_hierarchy(ui: &mut egui::Ui, status: &NodeStatus, dashboard: HealthDashboard) {
    ui.label(RichText::new("This Node hierarchy").strong());
    ui.label(
        RichText::new(
            "Expand a governed section to inspect its current detail availability. Badges use the same live health authority as the dashboard.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);

    for group in SectionGroup::ALL {
        let group_health = group_health(dashboard, group);
        egui::CollapsingHeader::new(
            RichText::new(format!(
                "{}   {} {}",
                group.label(),
                group_health.glyph(),
                group_health.label()
            ))
            .color(group_health.tone()),
        )
        .id_salt(("this-node-hierarchy", group.label()))
        .open(Some(true))
        .show(ui, |ui| {
            ui.label(
                RichText::new(group.description())
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            let mut rows = DenseList::new();
            for section in group.sections() {
                let health = dashboard.health(*section);
                rows.row(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            health.tone(),
                            RichText::new(format!("{} {}", health.glyph(), health.label()))
                                .size(Style::SMALL),
                        );
                        ui.label(RichText::new(section.label()).strong().size(Style::SMALL));
                        ui.label(
                            RichText::new(section.description())
                                .color(Style::TEXT_DIM)
                                .size(Style::SMALL),
                        );
                    });
                    if health != SectionHealth::Healthy {
                        ui.label(
                            RichText::new(section_health_detail(*section, health, status))
                                .color(health.tone())
                                .size(Style::SMALL),
                        );
                    }
                });
            }
        });
    }
}

fn group_health(dashboard: HealthDashboard, group: SectionGroup) -> SectionHealth {
    let mut has_attention = false;
    let mut has_unavailable = false;
    for section in group.sections() {
        match dashboard.health(*section) {
            SectionHealth::Unhealthy => return SectionHealth::Unhealthy,
            SectionHealth::Stale => return SectionHealth::Stale,
            SectionHealth::Attention => has_attention = true,
            SectionHealth::Unavailable => has_unavailable = true,
            SectionHealth::Healthy => {}
        }
    }
    if has_attention {
        SectionHealth::Attention
    } else if has_unavailable {
        SectionHealth::Unavailable
    } else {
        SectionHealth::Healthy
    }
}

fn section_health_detail(section: Section, health: SectionHealth, status: &NodeStatus) -> String {
    match health {
        SectionHealth::Healthy => "Current provider facts are available.".to_string(),
        SectionHealth::Stale => status
            .stale_reason
            .clone()
            .unwrap_or_else(|| "The retained provider projection is stale; refresh first.".into()),
        SectionHealth::Unavailable => section
            .unavailable_reason()
            .map(str::to_owned)
            .unwrap_or_else(|| "No provider observation for this section is published yet.".into()),
        SectionHealth::Attention => match section {
            Section::Connectivity => status.connectivity_availability().detail().to_string(),
            Section::Overview => {
                if !status.in_directory {
                    "This node is named by the snapshot but has no directory row yet.".into()
                } else if status.update_available {
                    "A newer mesh version is visible; update execution remains provider-gated."
                        .into()
                } else {
                    "The node is present but its health facts are only partially reported.".into()
                }
            }
            Section::PowerPerformance => {
                "Power-profile facts are partial; missing telemetry is not inferred.".into()
            }
            Section::MeshSystem => {
                "Mesh context or service health is only partially reported.".into()
            }
            _ => {
                "This section has partial provider facts; missing values remain unavailable.".into()
            }
        },
        SectionHealth::Unhealthy => {
            if status.presence.as_deref() == Some("offline") {
                "The node's published presence is offline.".into()
            } else {
                "One or more published service rows report down.".into()
            }
        }
    }
}

fn show_power_profile(ui: &mut egui::Ui, status: &NodeStatus) {
    if status.power_profile.available.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "Power-profile provider not observed");
        ui.label(
            RichText::new("No profile names crossed the credential-free mesh-status boundary.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Active:");
        ui.colored_label(
            if status.stale { Style::WARN } else { Style::OK },
            status.power_profile.active.as_deref().unwrap_or("unknown"),
        );
    });
    ui.label(
        RichText::new(format!(
            "Advertised profiles: {}",
            status.power_profile.available.join(", ")
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_audio(ui: &mut egui::Ui, status: &NodeStatus) {
    let facts = &status.audio;
    if facts == &AudioFacts::default() {
        ui.colored_label(Style::TEXT_DIM, "Audio provider not observed");
        ui.label(
            RichText::new(
                "PipeWire, PulseAudio compatibility, WirePlumber, and ALSA/UCM facts are not published by mesh-status.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    let rows = [
        ("PulseAudio compatibility", facts.pulse_available),
        ("PipeWire graph", facts.pipewire_graph),
        ("WirePlumber policy", facts.wireplumber_policy),
        ("Playback", facts.playback),
        ("Capture", facts.capture),
    ];
    for (label, value) in rows {
        ui.horizontal(|ui| {
            ui.label(label);
            match value {
                Some(true) => ui.colored_label(Style::OK, "available"),
                Some(false) => ui.colored_label(Style::WARN, "unavailable"),
                None => ui.colored_label(Style::TEXT_DIM, "unknown"),
            };
        });
    }
    if let Some(count) = facts.alsa_devices {
        ui.label(
            RichText::new(format!("ALSA/UCM devices discovered: {count}"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }
    if let Some(recovery) = &facts.recovery {
        ui.label(
            RichText::new(format!("Recovery: {recovery}"))
                .color(Style::WARN)
                .size(Style::SMALL),
        );
    }
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
    mde_egui::card().show(ui, |ui| {
        let mut rows = DenseList::new();
        for projection in status.capability_projection() {
            rows.row(ui, |ui| show_capability_row(ui, projection));
        }
    });

    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Typed node actions")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    mde_egui::card().show(ui, |ui| {
        let mut rows = DenseList::new();
        for projection in status.action_projection() {
            rows.row(ui, |ui| show_action_row(ui, projection));
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
    mde_egui::card().show(ui, |ui| {
        let mut rows = DenseList::new();
        for projection in status.provider_projection() {
            rows.row(ui, |ui| show_connectivity_provider_row(ui, projection));
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
    if let Some(recovery) = projection.recovery.label() {
        connectivity_field(ui, "Next safe step", Some(recovery));
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
        let response = mde_egui::widgets::hover_text(response, availability.detail());
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
    let mut rows = DenseList::new();
    for (label, up) in &status.services {
        rows.row(ui, |ui| {
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
        });
    }
}

/// The mesh-context card: the live peer count (online / total) and the elected
/// leader.
fn show_mesh(ui: &mut egui::Ui, status: &NodeStatus) {
    match status.peer_counts {
        Some((online, total)) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Peers")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                let tone = if total == 0 {
                    Style::TEXT_DIM
                } else if online == total {
                    Style::OK
                } else {
                    Style::WARN
                };
                ui.colored_label(
                    tone,
                    RichText::new(format!("{online}/{total} live")).size(Style::SMALL),
                );
            });
        }
        None => mde_egui::field(ui, "Peers", "unavailable", Style::TEXT_DIM),
    }
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
              "audio":{{"pulse_available":true,"pipewire_graph":true,
                "wireplumber_policy":true,"alsa_devices":2,"playback":true,
                "capture":true,"recovery":""}},
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
        assert_eq!(s.peer_counts, Some((2, 3)));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert!(!s.is_leader(), "the leader is a peer, not this node");
        assert_eq!(s.power_profile.active.as_deref(), Some("balanced"));
        assert_eq!(
            s.power_profile.available,
            vec!["balanced", "performance", "power-saver"]
        );
        assert_eq!(s.audio.pulse_available, Some(true));
        assert_eq!(s.audio.pipewire_graph, Some(true));
        assert_eq!(s.audio.alsa_devices, Some(2));
        assert_eq!(s.audio.playback, Some(true));
        assert_eq!(s.audio.capture, Some(true));

        // And the whole live panel tessellates.
        assert!(
            renders(&s),
            "the live ThisNode panel produced no draw primitives"
        );
    }

    #[test]
    fn audio_projection_is_bounded_and_does_not_invent_provider_health() {
        let value = serde_json::json!({
            "pulse_available": false,
            "pipewire_graph": true,
            "wireplumber_policy": null,
            "alsa_devices": 999,
            "playback": false,
            "capture": true,
            "recovery": "  restart PipeWire and refresh the snapshot  ",
        });
        let facts = audio_facts(Some(&value));
        assert_eq!(facts.pulse_available, Some(false));
        assert_eq!(facts.pipewire_graph, Some(true));
        assert_eq!(facts.wireplumber_policy, None);
        assert_eq!(facts.alsa_devices, None);
        assert_eq!(facts.playback, Some(false));
        assert_eq!(facts.capture, Some(true));
        assert_eq!(facts.recovery.as_deref(), Some("restart PipeWire and refresh the snapshot"));
        assert_eq!(audio_facts(None), AudioFacts::default());

        let typed = serde_json::json!({
            "availability": "available",
            "pulse_audio_compatibility": {
                "availability": "available",
                "compatibility": "compatible"
            },
            "pipewire_graph": {"availability": "available"},
            "wireplumber_policy": {"availability": "unavailable"},
            "alsa_ucm_discovery": {"availability": "available", "observed_items": 3},
            "playback": {"availability": "available"},
            "capture": {"availability": "unavailable"},
            "recovery": {"availability": "unavailable"}
        });
        let typed_facts = audio_facts(Some(&typed));
        assert_eq!(typed_facts.pulse_available, Some(true));
        assert_eq!(typed_facts.pipewire_graph, Some(true));
        assert_eq!(typed_facts.wireplumber_policy, Some(false));
        assert_eq!(typed_facts.alsa_devices, Some(3));
        assert_eq!(typed_facts.playback, Some(true));
        assert_eq!(typed_facts.capture, Some(false));
        assert!(typed_facts.recovery.is_some());
    }

    #[test]
    fn health_dashboard_tracks_the_fixed_tree_and_explicit_attention() {
        let s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let dashboard = s.health_dashboard();

        assert_eq!(dashboard.states.len(), Section::ALL.len());
        assert_eq!(dashboard.overall, SectionHealth::Unhealthy);
        assert_eq!(
            dashboard.health(Section::Overview),
            SectionHealth::Unhealthy
        );
        assert_eq!(
            dashboard.health(Section::Connectivity),
            SectionHealth::Healthy
        );
        assert_eq!(
            dashboard.health(Section::DisplaySound),
            SectionHealth::Healthy
        );
        assert_eq!(
            dashboard.health(Section::PowerPerformance),
            SectionHealth::Healthy
        );
        assert_eq!(
            dashboard.health(Section::Hardware),
            SectionHealth::Unavailable
        );
        assert!(dashboard.alert_count() > 0);

        let flattened: Vec<_> = SectionGroup::ALL
            .into_iter()
            .flat_map(SectionGroup::sections)
            .copied()
            .collect();
        assert_eq!(flattened, Section::ALL);
        assert!(
            renders(&s),
            "the dashboard and hierarchy must paint together"
        );
    }

    #[test]
    fn stale_health_dashboard_marks_every_tree_row_stale() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        s.mark_stale("provider stopped publishing");

        let dashboard = s.health_dashboard();
        assert_eq!(dashboard.overall, SectionHealth::Stale);
        assert!(dashboard
            .states
            .iter()
            .all(|(_, health)| *health == SectionHealth::Stale));
        assert!(
            section_health_detail(Section::Hardware, SectionHealth::Stale, &s)
                .contains("provider stopped publishing")
        );
        assert!(renders(&s), "stale dashboard rows must remain renderable");
        assert!(s
            .provider_projection()
            .iter()
            .all(|projection| projection.recovery == ProviderRecovery::RefreshSnapshot));
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
        assert_eq!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Wifi)
                .expect("Wi-Fi provider")
                .recovery,
            ProviderRecovery::AwaitProvider
        );
        assert_eq!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Mesh)
                .expect("mesh provider")
                .recovery,
            ProviderRecovery::None
        );
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
    fn missing_or_impossible_peer_counts_stay_unavailable() {
        for snapshot in [
            r#"{"self":"this-node","nodes":[],"total":3}"#,
            r#"{"self":"this-node","nodes":[],"online":2,"total":1}"#,
            r#"{"self":"this-node","nodes":[],"online":"2","total":3}"#,
        ] {
            let status = NodeStatus::project(snapshot, "fallback");
            assert!(status.seen, "the snapshot itself is readable");
            assert_eq!(
                status.peer_counts, None,
                "invalid counts must not become 0/0"
            );

            let mesh = status
                .capability_projection()
                .into_iter()
                .find(|projection| projection.capability == NodeCapability::MeshContext)
                .expect("mesh context capability");
            assert!(matches!(
                mesh.availability,
                CapabilityAvailability::Unavailable(_)
            ));
            assert!(mesh.availability.detail().contains("peer counts"));
            assert!(
                renders(&status),
                "the unavailable peer state must still render"
            );
        }
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
        assert_eq!(ethernet.recovery, ProviderRecovery::RefreshSnapshot);

        let cellular = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Cellular)
            .expect("cellular provider");
        assert!(matches!(
            cellular.availability,
            ConnectivityAvailability::Degraded(_)
        ));
        assert_eq!(cellular.interface.as_deref(), Some("wwan0"));
        assert_eq!(cellular.recovery, ProviderRecovery::RefreshSnapshot);

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
        assert_eq!(s.peer_counts, Some((2, 3)));
        // Per-node fields are honestly empty, not fabricated.
        assert!(s.role.is_none());
        assert!(s.presence.is_none());
        assert!(s.services.is_empty());
        assert!(s.heartbeat_label().is_none());
        // The honest-partial panel still fully paints.
        assert!(renders(&s));
    }

    #[test]
    fn provider_loss_retains_last_snapshot_as_explicitly_stale() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen);
        assert!(!s.stale);
        let hostname = s.hostname.clone();
        let overlay_ip = s.overlay_ip.clone();

        s.mark_stale("provider unavailable");

        assert!(s.stale);
        assert_eq!(s.stale_reason.as_deref(), Some("provider unavailable"));
        assert_eq!(s.hostname, hostname);
        assert_eq!(s.overlay_ip, overlay_ip);
        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        assert!(s.capability_projection().iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Degraded(_)
        )));
        assert!(s.provider_projection().iter().all(|projection| {
            !matches!(
                projection.availability,
                ConnectivityAvailability::Available(_)
            )
        }));
        let mesh = s
            .provider_projection()
            .into_iter()
            .find(|projection| projection.provider == ConnectivityProvider::Mesh)
            .expect("mesh provider projection");
        assert!(matches!(
            mesh.availability,
            ConnectivityAvailability::Degraded(
                "The last provider observation is stale; refresh before relying on it."
            )
        ));
        assert_eq!(
            mesh.recovery,
            ProviderRecovery::RefreshSnapshot,
            "stale provider rows must expose refresh as the safe next step"
        );
        assert!(s.action_projection().iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Degraded(_)
        )));
        assert!(renders(&s), "stale retained state must remain renderable");
    }

    #[test]
    fn snapshot_age_is_bounded_and_future_timestamps_do_not_fake_staleness() {
        assert_eq!(snapshot_age_ms(0, 10_000), None);
        assert_eq!(snapshot_age_ms(9_000, 100_000), Some(91_000));
        assert_eq!(snapshot_age_ms(101_000, 100_000), Some(0));
        assert!(snapshot_age_ms(9_000, 100_000).is_some_and(|age| age > MAX_SNAPSHOT_AGE_MS));
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
