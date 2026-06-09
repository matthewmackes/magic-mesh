//! MESH-A-4.a (v5.0.0) — surrounding-host taxonomy + classifier.
//!
//! A "surrounding host" is a LAN neighbour that is **not** a mesh peer
//! (R8-Q1..Q15, design doc §7.3). This module owns the two locked
//! enumerations — the 14 [`HostType`]s (R8-Q9) and the 3
//! [`TrustState`]s (R8-Q10) — plus the pure [`classify`] heuristic
//! that turns a discovery pass's [`HostSignals`] into a best-guess
//! type. `TrustState` serialises to the same lowercase strings the
//! `mde_card::probe::HostFacts.trust_state` field already carries (its
//! doc-comment names this module as the taxonomy owner).
//!
//! The discovery collectors that gather [`HostSignals`] from the wire
//! (mDNS / ARP / OUI / reverse-DNS / HTTP-banner / nmap fingerprint)
//! land in MESH-A-4.b; the worker that stores + mesh-syncs the
//! `SurroundingHost` records lands in MESH-A-4.c. This sub-task ships
//! the taxonomy + classifier + the `mackesd classify-host` CLI that
//! exercises it end-to-end.
//!
//! ## Classification heuristics (best-choice — no design lock)
//!
//! The design doc locks the 14 types but not the rules that infer
//! them, so [`classify`] uses a confidence-ordered cascade:
//!
//! 1. **mDNS service type** (strongest) — a printer announces
//!    `_ipp._tcp`, a Chromecast `_googlecast._tcp`, a NAS `_smb._tcp`.
//! 2. **MAC-OUI vendor** — disambiguates network gear, cameras,
//!    printers, NAS, consoles that don't announce mDNS.
//! 3. **Open ports** (weakest) — only the few unambiguous ones
//!    (9100 raw-print → Printer, 554 RTSP → Camera).
//!
//! Anything unmatched is [`HostType::Unknown`] — the classifier never
//! guesses past its confidence. Switch / Ap / Server need richer
//! signals (SNMP sysObjectID, LLDP) deferred to MESH-A-4.b; they are
//! valid taxonomy members reachable for manual assignment today.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

/// One of the 14 surrounding-host types (R8-Q9). Wire form is the
/// kebab-case [`HostType::wire_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostType {
    /// Home/office gateway router.
    Router,
    /// Managed or unmanaged network switch.
    Switch,
    /// Wireless access point.
    Ap,
    /// Network printer or scanner.
    Printer,
    /// Network-attached storage / file server.
    Nas,
    /// IP camera or NVR.
    Camera,
    /// Casting / streaming video target (Chromecast, AirPlay, Roku).
    TvCast,
    /// Smart speaker / audio receiver (Sonos, Echo, AirPlay audio).
    SmartSpeaker,
    /// Generic IoT / home-automation device.
    Iot,
    /// Phone or tablet handheld.
    Phone,
    /// Desktop or laptop computer.
    Computer,
    /// Headless server host.
    Server,
    /// Game console (PlayStation, Nintendo, Xbox).
    GameConsole,
    /// Unclassified — the signals matched no known type.
    Unknown,
}

impl HostType {
    /// Stable kebab-case wire name (matches the serde rename).
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            HostType::Router => "router",
            HostType::Switch => "switch",
            HostType::Ap => "ap",
            HostType::Printer => "printer",
            HostType::Nas => "nas",
            HostType::Camera => "camera",
            HostType::TvCast => "tv-cast",
            HostType::SmartSpeaker => "smart-speaker",
            HostType::Iot => "iot",
            HostType::Phone => "phone",
            HostType::Computer => "computer",
            HostType::Server => "server",
            HostType::GameConsole => "game-console",
            HostType::Unknown => "unknown",
        }
    }
}

/// Trust classification (R8-Q10). Serialises to the lowercase strings
/// `mde_card::probe::HostFacts.trust_state` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustState {
    /// Operator-trusted neighbour.
    Trusted,
    /// Seen but not yet trusted — the default for a freshly-discovered
    /// host.
    Unknown,
    /// Operator-blocked; MESH-A-5 enforces the mesh-wide firewall DROP.
    Blocked,
}

impl Default for TrustState {
    fn default() -> Self {
        // A freshly-seen neighbour is untrusted-but-not-blocked.
        TrustState::Unknown
    }
}

impl TrustState {
    /// Stable lowercase wire name (matches the serde rename + the
    /// `mde_card::probe::HostFacts.trust_state` strings).
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            TrustState::Trusted => "trusted",
            TrustState::Unknown => "unknown",
            TrustState::Blocked => "blocked",
        }
    }
}

/// The signals a discovery pass (MESH-A-4.b) gathers about a host,
/// fed to [`classify`]. All fields optional/empty — a host seen only
/// in the ARP table has just an `oui_vendor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostSignals {
    /// mDNS service types advertised (`_ipp._tcp`, `_airplay._tcp`, …).
    #[serde(default)]
    pub mdns_services: Vec<String>,
    /// Open TCP ports observed.
    #[serde(default)]
    pub open_ports: Vec<u16>,
    /// MAC-OUI vendor string (`Hewlett Packard`, `Ubiquiti Inc`, …).
    #[serde(default)]
    pub oui_vendor: String,
    /// Hostname (mDNS / reverse-DNS), used for the console hostname
    /// hint (MESH-A-4.b.2). Empty when unknown.
    #[serde(default)]
    pub hostname: String,
}

/// A discovered surrounding host (a LAN neighbour that is not a mesh
/// peer). Built by the MESH-A-4.b collectors; the A-4.c worker stores
/// + mesh-syncs these records. (The A-4.a note pencilled this struct
/// in for A-4.c; it lands here in A-4.b.1, where the mDNS sweep first
/// constructs it.)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SurroundingHost {
    /// IPv4/IPv6 address.
    pub ip: String,
    /// MAC address (empty until an ARP/OUI pass fills it — A-4.b.2).
    #[serde(default)]
    pub mac: String,
    /// MAC-OUI vendor (empty until A-4.b.2).
    #[serde(default)]
    pub vendor: String,
    /// Hostname (mDNS / reverse-DNS; may be empty).
    #[serde(default)]
    pub hostname: String,
    /// Advertised service identifiers (mDNS service types today).
    #[serde(default)]
    pub services: Vec<String>,
    /// Classified host type.
    pub host_type: HostType,
    /// Trust state (defaults to Unknown for a freshly-seen host).
    #[serde(default)]
    pub trust: TrustState,
    /// Unix-epoch ms first seen.
    pub first_seen_ms: i64,
    /// Unix-epoch ms last seen.
    pub last_seen_ms: i64,
}

/// One resolved mDNS service record (an `avahi-browse` `=` line).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MdnsService {
    /// Resolved address.
    pub ip: String,
    /// Resolved hostname.
    pub hostname: String,
    /// Service type (`_ipp._tcp`, `_googlecast._tcp`, …).
    pub service_type: String,
}

/// Classify a host from its discovery signals into one of the 14
/// [`HostType`]s. See the module docs for the confidence cascade;
/// returns [`HostType::Unknown`] when nothing matches.
#[must_use]
pub fn classify(sig: &HostSignals) -> HostType {
    // 1. Console hostname hint — highest confidence for the specific
    //    `PS4-`/`Xbox-`/`Nintendo-` patterns, and it must outrank a
    //    media service the console also advertises (a PS4 announces
    //    `_spotify-connect._tcp`, which would otherwise read as a
    //    smart speaker).
    if let Some(t) = host_type_from_hostname(&sig.hostname) {
        return t;
    }
    // 2. mDNS service type — strongest generic signal.
    for svc in &sig.mdns_services {
        if let Some(t) = host_type_from_mdns(svc) {
            return t;
        }
    }
    // 3. MAC-OUI vendor.
    if let Some(t) = host_type_from_vendor(&sig.oui_vendor) {
        return t;
    }
    // 4. Open ports — weakest, only the unambiguous ones.
    for &port in &sig.open_ports {
        if let Some(t) = host_type_from_port(port) {
            return t;
        }
    }
    HostType::Unknown
}

/// Map an mDNS service type to a host type. Substring match so a full
/// `_ipp._tcp.local.` instance name still resolves.
fn host_type_from_mdns(service: &str) -> Option<HostType> {
    let s = service.to_ascii_lowercase();
    if s.contains("_ipp")
        || s.contains("_printer")
        || s.contains("_pdl-datastream")
        || s.contains("_scanner")
        || s.contains("_uscan")
    {
        return Some(HostType::Printer);
    }
    if s.contains("_googlecast")
        || s.contains("_airplay")
        || s.contains("_amzn-wplay")
        || s.contains("_roku")
        || s.contains("_androidtvremote")
    {
        return Some(HostType::TvCast);
    }
    if s.contains("_raop") || s.contains("_spotify-connect") || s.contains("_sonos") {
        return Some(HostType::SmartSpeaker);
    }
    if s.contains("_smb")
        || s.contains("_afpovertcp")
        || s.contains("_nfs")
        || s.contains("_adisk")
        || s.contains("_webdav")
    {
        return Some(HostType::Nas);
    }
    if s.contains("_axis-video") || s.contains("_rtsp") || s.contains("_onvif") {
        return Some(HostType::Camera);
    }
    if s.contains("_hap")
        || s.contains("_homekit")
        || s.contains("_matter")
        || s.contains("_hue")
        || s.contains("_coap")
    {
        return Some(HostType::Iot);
    }
    if s.contains("_apple-mobdev") || s.contains("_companion-link") {
        return Some(HostType::Phone);
    }
    if s.contains("_workstation") {
        return Some(HostType::Computer);
    }
    None
}

/// Map a MAC-OUI vendor string to a host type. Case-insensitive
/// substring match against well-known vendor tokens.
fn host_type_from_vendor(vendor: &str) -> Option<HostType> {
    let v = vendor.to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    // Network infrastructure — router/AP/switch are hard to split by
    // vendor alone, so map to Router (the common LAN gateway device).
    for needle in [
        "ubiquiti", "cisco", "netgear", "tp-link", "tplink", "mikrotik", "asustek", "d-link",
        "dlink", "aruba", "ruckus", "juniper", "zyxel", "fortinet",
    ] {
        if v.contains(needle) {
            return Some(HostType::Router);
        }
    }
    for needle in [
        "hewlett", "hp inc", "canon", "epson", "brother", "lexmark", "xerox", "kyocera",
    ] {
        if v.contains(needle) {
            return Some(HostType::Printer);
        }
    }
    for needle in [
        "hikvision",
        "dahua",
        "axis communications",
        "reolink",
        "wyze",
        "amcrest",
    ] {
        if v.contains(needle) {
            return Some(HostType::Camera);
        }
    }
    for needle in ["synology", "qnap", "western digital", "drobo"] {
        if v.contains(needle) {
            return Some(HostType::Nas);
        }
    }
    for needle in ["sonos", "bose", "harman"] {
        if v.contains(needle) {
            return Some(HostType::SmartSpeaker);
        }
    }
    for needle in ["nintendo", "sony interactive"] {
        if v.contains(needle) {
            return Some(HostType::GameConsole);
        }
    }
    if v.contains("raspberry") {
        return Some(HostType::Computer);
    }
    None
}

/// Map an open port to a host type — only the few unambiguous ports.
fn host_type_from_port(port: u16) -> Option<HostType> {
    match port {
        9100 => Some(HostType::Printer), // raw print / JetDirect
        554 => Some(HostType::Camera),   // RTSP
        _ => None,
    }
}

/// Map a hostname to a host type for the few high-confidence patterns
/// (MESH-A-4.b.2). Today only game consoles, whose hostnames
/// (`PS4-…`, `Xbox-…`, `Nintendo-…`) are far more reliable than the
/// media services they also advertise. Case-insensitive substring
/// match; `None` for generic hostnames.
fn host_type_from_hostname(hostname: &str) -> Option<HostType> {
    let h = hostname.to_ascii_lowercase();
    if h.is_empty() {
        return None;
    }
    for needle in ["ps4", "ps5", "playstation", "xbox", "nintendo"] {
        if h.contains(needle) {
            return Some(HostType::GameConsole);
        }
    }
    None
}

/// Parse `avahi-browse -aprt` output into resolved mDNS service
/// records. Only `=` (resolved) lines carry an address; `+` (browse)
/// lines are skipped. Fields are `;`-separated:
/// `=;iface;proto;name;type;domain;hostname;address;port;txt…`.
#[must_use]
pub fn parse_avahi_browse(stdout: &str) -> Vec<MdnsService> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        if !line.starts_with('=') {
            continue;
        }
        let f: Vec<&str> = line.split(';').collect();
        if f.len() < 8 {
            continue;
        }
        let service_type = f[4].trim().to_string();
        let hostname = f[6].trim().to_string();
        let ip = f[7].trim().to_string();
        if ip.is_empty() || service_type.is_empty() {
            continue;
        }
        out.push(MdnsService {
            ip,
            hostname,
            service_type,
        });
    }
    out
}

/// Group resolved mDNS records by IP into [`SurroundingHost`]s,
/// classifying each from its advertised service types. `now_ms`
/// stamps first/last-seen. Pure over the already-collected records.
#[must_use]
pub fn hosts_from_mdns(records: &[MdnsService], now_ms: i64) -> Vec<SurroundingHost> {
    use std::collections::BTreeMap;
    // ip -> (hostname, service-types in first-seen order)
    let mut by_ip: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for r in records {
        let entry = by_ip
            .entry(r.ip.clone())
            .or_insert_with(|| (r.hostname.clone(), Vec::new()));
        if entry.0.is_empty() && !r.hostname.is_empty() {
            entry.0 = r.hostname.clone();
        }
        if !entry.1.contains(&r.service_type) {
            entry.1.push(r.service_type.clone());
        }
    }
    by_ip
        .into_iter()
        .map(|(ip, (hostname, services))| {
            let sig = HostSignals {
                mdns_services: services.clone(),
                hostname: hostname.clone(),
                ..Default::default()
            };
            SurroundingHost {
                ip,
                mac: String::new(),
                vendor: String::new(),
                hostname,
                services,
                host_type: classify(&sig),
                trust: TrustState::default(),
                first_seen_ms: now_ms,
                last_seen_ms: now_ms,
            }
        })
        .collect()
}

/// Browse the LAN for mDNS services via `avahi-browse -aprt` and parse
/// the resolved records. Returns empty when `binary` is absent
/// (headless / air-gapped peer) or exits non-zero. The shell-out is
/// HW-bench-gated like the netassess collectors; [`parse_avahi_browse`]
/// is the unit-tested pure half.
#[must_use]
pub fn collect_mdns(binary: &str) -> Vec<MdnsService> {
    let Ok(out) = Command::new(binary).args(["-a", "-p", "-r", "-t"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_avahi_browse(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `getent hosts <ip>` output into the resolved hostname. The
/// line is `<address>   <canonical-name> [aliases…]`; returns the
/// canonical name, or `None` when there is no name field.
#[must_use]
pub fn parse_getent_hosts(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Reverse-resolve `ip` to a hostname via `getent hosts` (the system
/// resolver — DNS PTR + `/etc/hosts` + mDNS). `None` when unresolved
/// or `getent` is absent. HW-bench-gated shell-out; the pure half is
/// [`parse_getent_hosts`].
#[must_use]
pub fn reverse_dns(ip: &str) -> Option<String> {
    let out = Command::new("getent").args(["hosts", ip]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_getent_hosts(&String::from_utf8_lossy(&out.stdout))
}

/// An OUI (first-3-octets) → vendor table, built from a system OUI file
/// in nmap's `nmap-mac-prefixes` format (`<6hex> <vendor>`).
#[derive(Debug, Clone, Default)]
pub struct OuiTable {
    map: HashMap<String, String>,
}

impl OuiTable {
    /// Vendor for a MAC address (any common separator), keyed on its
    /// 3-octet OUI prefix. `None` when the prefix isn't in the table.
    #[must_use]
    pub fn vendor_for(&self, mac: &str) -> Option<String> {
        self.map.get(&mac_oui_prefix(mac)?).cloned()
    }

    /// Number of OUI entries parsed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the table is empty (no OUI file found / parsed).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Normalise a MAC to its 6-hex-digit OUI prefix (uppercase, no
/// separators). `None` when fewer than 3 octets of hex are present.
#[must_use]
pub fn mac_oui_prefix(mac: &str) -> Option<String> {
    let hex: String = mac
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect::<String>()
        .to_ascii_uppercase();
    if hex.len() < 6 {
        None
    } else {
        Some(hex)
    }
}

/// Parse an nmap-style OUI table (`<6hex> <vendor>` per line; `#`
/// comments + blank / short / garbage lines skipped).
#[must_use]
pub fn parse_oui_db(contents: &str) -> OuiTable {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((prefix, vendor)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let prefix = prefix.trim().to_ascii_uppercase();
        if prefix.len() != 6 || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let vendor = vendor.trim();
        if !vendor.is_empty() {
            map.insert(prefix, vendor.to_string());
        }
    }
    OuiTable { map }
}

/// Load the system OUI table — nmap's prefixes file, present when nmap
/// is installed (already a MESH-PROBE dependency). Empty when absent.
#[must_use]
pub fn load_system_oui() -> OuiTable {
    std::fs::read_to_string("/usr/share/nmap/nmap-mac-prefixes")
        .map(|c| parse_oui_db(&c))
        .unwrap_or_default()
}

/// Parse `ip neigh` output into an ip→mac map (lowercased MAC). The
/// surrounding-host enricher only needs the address→MAC mapping, so
/// this is a lighter, map-shaped parse than netassess's
/// `parse_ip_neigh` (which returns `Vec<ArpEntry>` behind the
/// async-services feature; this module stays feature-free, so it keeps
/// its own small parser rather than depending on a gated worker).
#[must_use]
pub fn parse_neigh_map(stdout: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(ip) = toks.first() else {
            continue;
        };
        if let Some(pos) = toks.iter().position(|t| *t == "lladdr") {
            if let Some(mac) = toks.get(pos + 1) {
                if !ip.is_empty() && !mac.is_empty() {
                    map.insert((*ip).to_string(), mac.to_ascii_lowercase());
                }
            }
        }
    }
    map
}

/// Read the ARP/neighbour table as an ip→mac map via `ip neigh`. Empty
/// when `ip` is absent or errors. HW-bench-gated shell-out; the pure
/// half is [`parse_neigh_map`].
#[must_use]
pub fn arp_neigh_map() -> HashMap<String, String> {
    let Ok(out) = Command::new("ip").args(["neigh"]).output() else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    parse_neigh_map(&String::from_utf8_lossy(&out.stdout))
}

/// Enrich discovered hosts with their MAC (from a pre-built ip→mac map
/// — e.g. [`arp_neigh_map`]) + the OUI vendor, then re-classify with
/// the now-fuller signal set. Pure + testable; the `discover-mdns` CLI
/// + the A-4.c worker supply the map + table. `classify`'s cascade
/// keeps a confident mDNS/hostname type ahead of the vendor, so
/// enrichment only ever *adds* type information (a mDNS-less Cisco box
/// becomes a Router from its OUI).
#[must_use]
pub fn enrich_hosts(
    mut hosts: Vec<SurroundingHost>,
    mac_by_ip: &HashMap<String, String>,
    oui: &OuiTable,
) -> Vec<SurroundingHost> {
    for host in &mut hosts {
        if host.mac.is_empty() {
            if let Some(mac) = mac_by_ip.get(&host.ip) {
                host.mac = mac.clone();
            }
        }
        if host.vendor.is_empty() && !host.mac.is_empty() {
            if let Some(v) = oui.vendor_for(&host.mac) {
                host.vendor = v;
            }
        }
        let sig = HostSignals {
            mdns_services: host.services.clone(),
            hostname: host.hostname.clone(),
            oui_vendor: host.vendor.clone(),
            ..Default::default()
        };
        host.host_type = classify(&sig);
    }
    hosts
}

/// Parse the `Server:` header value from `curl -I` output. Header name
/// match is case-insensitive; `None` when absent or empty.
#[must_use]
pub fn parse_http_server(headers: &str) -> Option<String> {
    for line in headers.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("server") {
                let v = value.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Map an HTTP `Server` banner to a host type for the recognisable
/// embedded-device servers (MESH-A-4.c.3). Generic servers
/// (nginx/Apache) say nothing about device type → `None`.
#[must_use]
pub fn host_type_from_http_server(server: &str) -> Option<HostType> {
    let s = server.to_ascii_lowercase();
    if s.contains("cups") || s.contains("ipp") {
        return Some(HostType::Printer);
    }
    if s.contains("hikvision")
        || s.contains("dahua")
        || s.contains("axis")
        || s.contains("webcam")
        || s.contains("rtsp")
    {
        return Some(HostType::Camera);
    }
    if s.contains("synology")
        || s.contains("diskstation")
        || s.contains("qnap")
        || s.contains("freenas")
        || s.contains("truenas")
    {
        return Some(HostType::Nas);
    }
    if s.contains("routeros")
        || s.contains("mikrotik")
        || s.contains("openwrt")
        || s.contains("dd-wrt")
    {
        return Some(HostType::Router);
    }
    None
}

/// Fetch the `Server` banner from `http://<ip>` via `curl -sI`
/// (3s timeout). `None` when curl is absent or the host doesn't serve
/// HTTP. HW-bench-gated shell-out; the pure half is
/// [`parse_http_server`].
#[must_use]
pub fn http_server_banner(ip: &str) -> Option<String> {
    let url = format!("http://{ip}");
    let out = Command::new("curl")
        .args(["-sI", "--max-time", "3", &url])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_http_server(&String::from_utf8_lossy(&out.stdout))
}

/// Refine still-[`HostType::Unknown`] hosts from their HTTP `Server`
/// banner. Only Unknown hosts are probed — a confident mDNS / hostname
/// / vendor type is left alone, and skipping typed hosts bounds the
/// per-sweep `curl` calls. The shell-out is HW-bench-gated.
pub fn refine_unknown_with_http(hosts: &mut [SurroundingHost]) {
    for host in hosts.iter_mut() {
        if host.host_type != HostType::Unknown {
            continue;
        }
        if let Some(server) = http_server_banner(&host.ip) {
            if let Some(t) = host_type_from_http_server(&server) {
                host.host_type = t;
            }
        }
    }
}

/// Parse nmap's `Device type:` line from `nmap -O` output. nmap prints
/// at most one such line (values `|`-separated when ambiguous, e.g.
/// `general purpose|router`); returns the raw value, `None` when absent
/// or empty. Case-insensitive on the key. Pure half of
/// [`nmap_os_fingerprint`].
#[must_use]
pub fn parse_nmap_device_type(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("device type") {
                let v = value.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Map an nmap `Device type:` value to a host type (MESH-A-4.c.3.b).
/// nmap's device-type taxonomy lines up closely with [`HostType`]; the
/// generic `general purpose` class can't distinguish computer vs server
/// → `None`. The value may carry several `|`-separated guesses — the
/// first specific match wins (the scan stops early), so e.g. `general
/// purpose|router` resolves to [`HostType::Router`].
#[must_use]
pub fn host_type_from_nmap_device_type(device_type: &str) -> Option<HostType> {
    let s = device_type.to_ascii_lowercase();
    if s.contains("printer") {
        return Some(HostType::Printer);
    }
    if s.contains("webcam") || s.contains("camera") {
        return Some(HostType::Camera);
    }
    if s.contains("storage") {
        return Some(HostType::Nas);
    }
    if s.contains("wap") || s.contains("access point") {
        return Some(HostType::Ap);
    }
    if s.contains("switch") || s.contains("bridge") {
        return Some(HostType::Switch);
    }
    if s.contains("broadband router") || s.contains("router") || s.contains("gateway") {
        return Some(HostType::Router);
    }
    if s.contains("game console") {
        return Some(HostType::GameConsole);
    }
    if s.contains("media device") {
        return Some(HostType::TvCast);
    }
    if s.contains("phone") {
        return Some(HostType::Phone);
    }
    // "general purpose" (and anything unrecognised) is too weak to pin a
    // computer-vs-server distinction — leave the host Unknown.
    None
}

/// Active OS / TCP-IP fingerprint of `<ip>` via `nmap -O` (MESH-A-4.c.3.b).
/// Privileged — nmap's OS detection needs root; `-Pn` since the host is
/// already known up from the mDNS/ARP sweep, `--osscan-guess` to coax a
/// class from partial matches. Returns the parsed `Device type:` value;
/// `None` when nmap is absent, unprivileged, or yields no device type.
/// HW-bench-gated shell-out; the pure half is [`parse_nmap_device_type`].
#[must_use]
pub fn nmap_os_fingerprint(ip: &str) -> Option<String> {
    let out = Command::new("nmap")
        .args(["-O", "-Pn", "--osscan-guess", ip])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nmap_device_type(&String::from_utf8_lossy(&out.stdout))
}

/// Refine still-[`HostType::Unknown`] hosts with an active `nmap -O`
/// fingerprint (MESH-A-4.c.3.b). Probes only Unknown hosts — the same
/// bound the HTTP enricher uses — since `nmap -O` is privileged + slow;
/// a confident mDNS / hostname / vendor / HTTP type is left alone. The
/// shell-out is HW-bench-gated; the pure mapping is
/// [`host_type_from_nmap_device_type`].
pub fn refine_unknown_with_nmap_os(hosts: &mut [SurroundingHost]) {
    for host in hosts.iter_mut() {
        if host.host_type != HostType::Unknown {
            continue;
        }
        if let Some(dt) = nmap_os_fingerprint(&host.ip) {
            if let Some(t) = host_type_from_nmap_device_type(&dt) {
                host.host_type = t;
            }
        }
    }
}

/// One coalesced surrounding-host card (R8-Q14) — the union of every
/// peer's sighting of a single host identity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CoalescedHost {
    /// Identity key — the MAC when known (stable across IP changes /
    /// roaming), else the IP.
    pub key: String,
    /// The freshest sighting's full record (max `last_seen_ms`).
    pub host: SurroundingHost,
    /// How many raw sightings coalesced — the multi-sighting badge.
    pub sightings: usize,
    /// Every distinct IP this identity was seen at, in first-seen
    /// order — the roaming history.
    pub ips_seen: Vec<String>,
}

/// Coalesce raw per-peer sightings (the union of all peers' snapshots)
/// into one card per host identity (R8-Q14). Identity is the MAC when
/// present (stable across roaming), else the IP. The freshest sighting
/// (max `last_seen_ms`) supplies the card fields; `sightings` counts
/// the raw records; `ips_seen` is the roaming history. Pure + sorted
/// by key.
#[must_use]
pub fn coalesce_sightings(raw: Vec<SurroundingHost>) -> Vec<CoalescedHost> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, (SurroundingHost, usize, Vec<String>)> = BTreeMap::new();
    for host in raw {
        let key = if host.mac.is_empty() {
            host.ip.clone()
        } else {
            host.mac.clone()
        };
        let entry = groups
            .entry(key)
            .or_insert_with(|| (host.clone(), 0, Vec::new()));
        entry.1 += 1;
        if !entry.2.contains(&host.ip) {
            entry.2.push(host.ip.clone());
        }
        if host.last_seen_ms > entry.0.last_seen_ms {
            entry.0 = host;
        }
    }
    groups
        .into_iter()
        .map(|(key, (host, sightings, ips_seen))| CoalescedHost {
            key,
            host,
            sightings,
            ips_seen,
        })
        .collect()
}

/// Read + union every peer's latest surrounding snapshot under `root`
/// (`<root>/<peer>/<iso>-<hash>.json`, each a `Vec<SurroundingHost>`),
/// then [`coalesce_sightings`] into one card per host (R8-Q14). Uses
/// each peer's freshest snapshot (filenames sort chronologically by
/// their `<iso>` prefix). Per-file fail-open: a malformed/unreadable
/// snapshot is skipped, never aborts.
#[must_use]
pub fn read_all_surrounding(root: &Path) -> Vec<CoalescedHost> {
    let mut raw: Vec<SurroundingHost> = Vec::new();
    let Ok(peers) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for peer in peers.flatten() {
        let dir = peer.path();
        if !dir.is_dir() {
            continue;
        }
        // Freshest snapshot = lexically-max filename (the <iso> prefix
        // sorts by time).
        let latest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .max();
        let Some(path) = latest else {
            continue;
        };
        if let Ok(body) = std::fs::read_to_string(&path) {
            if let Ok(hosts) = serde_json::from_str::<Vec<SurroundingHost>>(&body) {
                raw.extend(hosts);
            }
        }
    }
    let mut cards = coalesce_sightings(raw);
    apply_trust(&mut cards, &load_trust_store(&root.join("trust.json")));
    cards
}

/// Operator trust overrides keyed by host identity (MAC when known,
/// else IP). Persisted at `<surrounding-base>/trust.json` (mesh-synced
/// — every peer honours the operator's Trust/Block decisions per
/// R8-Q10 / R8-Q11). Absence of a key means the default `Unknown`.
pub type TrustStore = BTreeMap<String, TrustState>;

/// Parse the trust-store JSON object (`{ "<key>": "trusted" | … }`).
/// Fail-open: a malformed body yields an empty store.
#[must_use]
pub fn parse_trust_store(json: &str) -> TrustStore {
    serde_json::from_str(json).unwrap_or_default()
}

/// Load the trust store from `path` (empty when absent / malformed).
#[must_use]
pub fn load_trust_store(path: &Path) -> TrustStore {
    std::fs::read_to_string(path)
        .map(|s| parse_trust_store(&s))
        .unwrap_or_default()
}

/// Persist the trust store to `path` (creates the parent dir).
///
/// # Errors
///
/// I/O errors creating the parent dir or writing the file.
pub fn save_trust_store(path: &Path, store: &TrustStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(store).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, body)
}

/// Set a host's trust override + persist (the Trust / Block card
/// actions, R8-Q11). Setting `Unknown` clears the override (back to the
/// default). Returns the updated store.
///
/// # Errors
///
/// Propagates [`save_trust_store`] I/O errors.
pub fn set_host_trust(path: &Path, key: &str, state: TrustState) -> std::io::Result<TrustStore> {
    let mut store = load_trust_store(path);
    if state == TrustState::Unknown {
        store.remove(key);
    } else {
        store.insert(key.to_string(), state);
    }
    save_trust_store(path, &store)?;
    Ok(store)
}

/// Apply trust overrides to coalesced cards — a card whose `key` is in
/// the store takes that trust state, else it keeps the default
/// `Unknown`.
pub fn apply_trust(cards: &mut [CoalescedHost], store: &TrustStore) {
    for card in cards.iter_mut() {
        if let Some(state) = store.get(&card.key) {
            card.host.trust = *state;
        }
    }
}

/// The distinct IPs to firewall-DROP — every IP a `Blocked` host was
/// seen at (the mesh-coordinated DROP, R8-Q44). Roaming-aware: all of a
/// blocked card's `ips_seen` are dropped. Pure over the coalesced +
/// trust-applied cards (MESH-A-5).
#[must_use]
pub fn blocked_ips(cards: &[CoalescedHost]) -> Vec<String> {
    let mut ips: Vec<String> = Vec::new();
    for card in cards {
        if card.host.trust == TrustState::Blocked {
            for ip in &card.ips_seen {
                if !ips.contains(ip) {
                    ips.push(ip.clone());
                }
            }
        }
    }
    ips
}

/// firewalld rich-rule body dropping all traffic from a source IP (the
/// mesh-coordinated DROP, R8-Q44). The family is `ipv6` for a
/// colon-bearing address, else `ipv4`.
#[must_use]
pub fn drop_rich_rule_body(ip: &str) -> String {
    let family = if ip.contains(':') { "ipv6" } else { "ipv4" };
    format!(r#"rule family="{family}" source address="{ip}" drop"#)
}

/// Detect ARP-spoofing suspects in a neighbour map (MESH-A-6.1,
/// R8-Q53): a MAC bound to **2+ distinct IPv4 addresses** — the
/// classic poisoning signature (one attacker MAC answering ARP for the
/// gateway + victim IPs). IPv4-only on purpose: a normal dual-stack
/// host shares its MAC across its v4 + v6 addresses, which is not a
/// spoof. Returns `(mac, sorted-ips)` per suspect, MAC-sorted. Pure.
#[must_use]
pub fn arp_spoof_suspects(neigh: &HashMap<String, String>) -> Vec<(String, Vec<String>)> {
    let mut by_mac: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (ip, mac) in neigh {
        if ip.contains(':') {
            continue; // IPv4 only
        }
        let ips = by_mac.entry(mac.clone()).or_default();
        if !ips.contains(ip) {
            ips.push(ip.clone());
        }
    }
    by_mac
        .into_iter()
        .filter(|(_, ips)| ips.len() >= 2)
        .map(|(mac, mut ips)| {
            ips.sort();
            (mac, ips)
        })
        .collect()
}

/// Parse `nmap --script broadcast-dhcp-discover` output for the
/// distinct DHCP server IPs — each `Server Identifier: <ip>` line
/// (MESH-A-6.2, R8-Q54). 2+ distinct servers answering on one segment
/// is a rogue DHCP server. Pure; tolerant of nmap's `|` / `|_` line
/// prefixes.
#[must_use]
pub fn parse_dhcp_servers(stdout: &str) -> Vec<String> {
    let mut servers = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_start_matches(['|', '_', ' ']).trim();
        if let Some(rest) = line.strip_prefix("Server Identifier:") {
            let ip = rest.trim().to_string();
            if !ip.is_empty() && !servers.contains(&ip) {
                servers.push(ip);
            }
        }
    }
    servers
}

/// Run `nmap --script broadcast-dhcp-discover` + parse the responding
/// DHCP servers. Empty when nmap is absent. HW-bench-gated (broadcast
/// scan); the pure half is [`parse_dhcp_servers`].
#[must_use]
pub fn detect_dhcp_servers() -> Vec<String> {
    let Ok(out) = Command::new("nmap")
        .args(["--script", "broadcast-dhcp-discover"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_dhcp_servers(&String::from_utf8_lossy(&out.stdout))
}

/// Default connectivity-check endpoint — returns `204 No Content` on a
/// clear connection; a captive portal intercepts it (MESH-A-6.4).
pub const CAPTIVE_PROBE_URL: &str = "http://connectivitycheck.gstatic.com/generate_204";

/// Detect a captive portal from `curl -sI <generate_204 endpoint>`
/// output (MESH-A-6.4, R8-Q31). A clear connection returns `204` →
/// `None`; any other status means a portal intercepted the probe →
/// `Some(portal_url)` (the `Location:` redirect target when present,
/// else an empty string for a non-redirecting splash). `None` when no
/// status line parses (the probe failed = offline, not captive). Pure.
#[must_use]
pub fn captive_portal_from_headers(headers: &str) -> Option<String> {
    let mut status: Option<u16> = None;
    let mut location: Option<String> = None;
    for line in headers.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("HTTP/") {
            if let Some(code) = rest.split_whitespace().nth(1) {
                status = code.parse().ok();
            }
        } else if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("location") {
                location = Some(v.trim().to_string());
            }
        }
    }
    match status {
        Some(204) | None => None,
        Some(_) => Some(location.unwrap_or_default()),
    }
}

/// Probe `url` (a generate_204 endpoint) via `curl -sI --max-time 4`
/// and report a captive portal. `None` when clear or curl is absent.
/// HW-bench-gated; the pure half is [`captive_portal_from_headers`].
#[must_use]
pub fn detect_captive_portal(url: &str) -> Option<String> {
    let out = Command::new("curl")
        .args(["-sI", "--max-time", "4", url])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    captive_portal_from_headers(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `nameserver <ip>` lines from `/etc/resolv.conf` content
/// (MESH-A-6.5). Requires `nameserver` as a whole leading token; skips
/// comments + non-nameserver lines; dedups. Pure; kept feature-free
/// here rather than reusing netassess's async-services-gated
/// `parse_resolv_conf`.
#[must_use]
pub fn parse_resolv_nameservers(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let mut toks = line.split_whitespace();
        if toks.next() == Some("nameserver") {
            if let Some(ip) = toks.next() {
                let ip = ip.to_string();
                if !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// Detect a DNS leak (MESH-A-6.5, R8-Q41): the configured resolvers
/// (`current`) that are NOT in the expected mesh resolver set
/// (`expected`). A non-empty result means traffic is resolving through
/// an off-mesh DNS server. Pure.
#[must_use]
pub fn dns_leak(current: &[String], expected: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|ip| !expected.contains(ip))
        .cloned()
        .collect()
}

/// SSID → known-BSSIDs baseline for evil-twin detection (MESH-A-6.3).
/// Persisted at `<surrounding-base>/wifi-baseline.json` + learned over
/// time; a known SSID seen on a BSSID absent from its set is a possible
/// evil twin (R8-Q60).
pub type WifiBaseline = BTreeMap<String, BTreeSet<String>>;

/// Split a terse `nmcli` line into fields on unescaped `:` (nmcli
/// escapes a literal colon as `\:`), unescaping each field.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                cur.push(next);
                chars.next();
                continue;
            }
        }
        if c == ':' {
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    fields
}

/// Parse `nmcli -t -f SSID,BSSID dev wifi` into `(ssid, bssid)` pairs
/// (BSSID upper-cased). Handles nmcli's `\:` colon-escaping; skips
/// lines without both fields or with an empty SSID/BSSID. Pure.
#[must_use]
pub fn parse_wifi_bssids(stdout: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let f = split_terse(line);
        if f.len() < 2 {
            continue;
        }
        let ssid = f[0].trim().to_string();
        let bssid = f[1].trim().to_ascii_uppercase();
        if !ssid.is_empty() && !bssid.is_empty() {
            out.push((ssid, bssid));
        }
    }
    out
}

/// Evil-twin suspects: a scanned `(ssid, bssid)` whose SSID is already
/// in the baseline (a known network) but whose BSSID is NOT one of its
/// known APs — a known SSID impersonated by a rogue AP (R8-Q60). Pure;
/// a never-seen SSID is not flagged (its first sighting is just
/// learned).
#[must_use]
pub fn evil_twin_suspects(
    scan: &[(String, String)],
    baseline: &WifiBaseline,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (ssid, bssid) in scan {
        if let Some(known) = baseline.get(ssid) {
            if !known.contains(bssid) {
                out.push((ssid.clone(), bssid.clone()));
            }
        }
    }
    out
}

/// Learn the scanned APs into the baseline (record each `(ssid,
/// bssid)`). Call AFTER [`evil_twin_suspects`] so a new BSSID is
/// flagged before it is learned.
pub fn learn_wifi(baseline: &mut WifiBaseline, scan: &[(String, String)]) {
    for (ssid, bssid) in scan {
        baseline
            .entry(ssid.clone())
            .or_default()
            .insert(bssid.clone());
    }
}

/// Load the WiFi baseline from `path` (empty when absent / malformed).
#[must_use]
pub fn load_wifi_baseline(path: &Path) -> WifiBaseline {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the WiFi baseline to `path` (creates the parent dir).
///
/// # Errors
///
/// I/O errors creating the parent dir or writing the file.
pub fn save_wifi_baseline(path: &Path, baseline: &WifiBaseline) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(baseline).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, body)
}

/// Scan visible WiFi APs via `nmcli -t -f SSID,BSSID dev wifi`. Empty
/// when nmcli is absent / no WiFi. HW-bench-gated; the pure half is
/// [`parse_wifi_bssids`].
#[must_use]
pub fn scan_wifi_bssids() -> Vec<(String, String)> {
    let Ok(out) = Command::new("nmcli")
        .args(["-t", "-f", "SSID,BSSID", "dev", "wifi"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_wifi_bssids(&String::from_utf8_lossy(&out.stdout))
}

/// 24-hour quiet window for persistent-attack accumulation (R8-Q74):
/// hits within the window coalesce into one alert; an alert quiet for
/// longer auto-acks.
pub const ALERT_QUIET_MS: i64 = 24 * 60 * 60 * 1_000;

/// One accumulating persistent-attack alert (R8-Q74) — repeated hits
/// from a single source coalesced into one record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistentAlert {
    /// Attack source (IP / host identity).
    pub source: String,
    /// Accumulated hit count in the current (un-acked) window.
    pub count: u64,
    /// Unix-epoch ms of the first hit in this window.
    pub first_seen_ms: i64,
    /// Unix-epoch ms of the most recent hit.
    pub last_seen_ms: i64,
}

/// Accumulating alert store keyed by attack source. Persisted at
/// `<surrounding-base>/persistent-alerts.json`.
pub type AlertStore = BTreeMap<String, PersistentAlert>;

/// Record a hit from `source` at `now_ms`, coalescing into the single
/// accumulating alert for that source (R8-Q74): a hit within
/// [`ALERT_QUIET_MS`] of the last bumps `count` + `last_seen`; a hit
/// after a longer quiet starts a fresh alert (count resets to 1).
pub fn accumulate_alert(store: &mut AlertStore, source: &str, now_ms: i64) {
    match store.get_mut(source) {
        Some(a) if now_ms.saturating_sub(a.last_seen_ms) <= ALERT_QUIET_MS => {
            a.count += 1;
            a.last_seen_ms = now_ms;
        }
        _ => {
            store.insert(
                source.to_string(),
                PersistentAlert {
                    source: source.to_string(),
                    count: 1,
                    first_seen_ms: now_ms,
                    last_seen_ms: now_ms,
                },
            );
        }
    }
}

/// Auto-ack (drop) alerts quiet for more than [`ALERT_QUIET_MS`]
/// (R8-Q74). Returns the auto-acked sources.
pub fn auto_ack(store: &mut AlertStore, now_ms: i64) -> Vec<String> {
    let acked: Vec<String> = store
        .iter()
        .filter(|(_, a)| now_ms.saturating_sub(a.last_seen_ms) > ALERT_QUIET_MS)
        .map(|(s, _)| s.clone())
        .collect();
    for s in &acked {
        store.remove(s);
    }
    acked
}

/// Load the alert store from `path` (empty when absent / malformed).
#[must_use]
pub fn load_alert_store(path: &Path) -> AlertStore {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the alert store to `path` (creates the parent dir).
///
/// # Errors
///
/// I/O errors creating the parent dir or writing the file.
pub fn save_alert_store(path: &Path, store: &AlertStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(store).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, body)
}

/// Whether an mDNS service type marks an MDE/MAP2 peer (MESH-A-8 /
/// R8-Q90). Best-choice marker set — `_map2-node._tcp` is the observed
/// node service; `_mde*` / `_mackes*` cover the rebrand variants. No
/// single in-tree definition, so this is the documented heuristic.
fn is_mde_service(service_type: &str) -> bool {
    let s = service_type.to_ascii_lowercase();
    s.contains("_map2-node") || s.contains("_mde") || s.contains("_mackes")
}

/// LAN MDE-peer pairing candidates (R8-Q90 auto-suggest): discovered
/// hosts advertising an MDE mDNS service. Returns `(ip, hostname)` per
/// candidate. Pure over the discovered/coalesced hosts; the onboarding
/// wizard (pair-with-passcode / install-first / QR, R8-Q35) is A-8.2.
#[must_use]
pub fn mde_peer_candidates(hosts: &[SurroundingHost]) -> Vec<(String, String)> {
    hosts
        .iter()
        .filter(|h| h.services.iter().any(|s| is_mde_service(s)))
        .map(|h| (h.ip.clone(), h.hostname.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig_mdns(svc: &str) -> HostSignals {
        HostSignals {
            mdns_services: vec![svc.to_string()],
            ..Default::default()
        }
    }

    fn sig_vendor(vendor: &str) -> HostSignals {
        HostSignals {
            oui_vendor: vendor.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn mdns_printer_cast_nas_speaker_camera() {
        assert_eq!(classify(&sig_mdns("_ipp._tcp.local.")), HostType::Printer);
        assert_eq!(classify(&sig_mdns("_googlecast._tcp")), HostType::TvCast);
        assert_eq!(classify(&sig_mdns("_smb._tcp")), HostType::Nas);
        assert_eq!(classify(&sig_mdns("_raop._tcp")), HostType::SmartSpeaker);
        assert_eq!(classify(&sig_mdns("_rtsp._tcp")), HostType::Camera);
    }

    #[test]
    fn vendor_router_camera_console_nas() {
        assert_eq!(classify(&sig_vendor("Ubiquiti Inc")), HostType::Router);
        assert_eq!(classify(&sig_vendor("Hikvision Digital")), HostType::Camera);
        assert_eq!(
            classify(&sig_vendor("Nintendo Co., Ltd.")),
            HostType::GameConsole
        );
        assert_eq!(
            classify(&sig_vendor("Synology Incorporated")),
            HostType::Nas
        );
        assert_eq!(classify(&sig_vendor("Hewlett Packard")), HostType::Printer);
    }

    #[test]
    fn port_fallback_only_for_unambiguous_ports() {
        let printer = HostSignals {
            open_ports: vec![9100],
            ..Default::default()
        };
        assert_eq!(classify(&printer), HostType::Printer);
        let camera = HostSignals {
            open_ports: vec![554],
            ..Default::default()
        };
        assert_eq!(classify(&camera), HostType::Camera);
        // 443 alone is too generic — stays Unknown.
        let web = HostSignals {
            open_ports: vec![443],
            ..Default::default()
        };
        assert_eq!(classify(&web), HostType::Unknown);
    }

    #[test]
    fn mdns_outranks_vendor_and_port() {
        // A printer behind a Ubiquiti-OUI NIC on port 443 still reads
        // as a printer from its mDNS announce.
        let sig = HostSignals {
            mdns_services: vec!["_ipp._tcp".to_string()],
            open_ports: vec![443],
            oui_vendor: "Ubiquiti Inc".to_string(),
            hostname: String::new(),
        };
        assert_eq!(classify(&sig), HostType::Printer);
    }

    #[test]
    fn empty_signals_are_unknown() {
        assert_eq!(classify(&HostSignals::default()), HostType::Unknown);
        assert_eq!(
            classify(&sig_vendor("Totally Unknown Vendor")),
            HostType::Unknown
        );
    }

    #[test]
    fn all_14_host_types_have_distinct_wire_names() {
        let all = [
            HostType::Router,
            HostType::Switch,
            HostType::Ap,
            HostType::Printer,
            HostType::Nas,
            HostType::Camera,
            HostType::TvCast,
            HostType::SmartSpeaker,
            HostType::Iot,
            HostType::Phone,
            HostType::Computer,
            HostType::Server,
            HostType::GameConsole,
            HostType::Unknown,
        ];
        let names: std::collections::HashSet<&str> = all.iter().map(|t| t.wire_name()).collect();
        assert_eq!(names.len(), 14, "all 14 wire names distinct");
    }

    #[test]
    fn host_type_serde_matches_wire_name() {
        assert_eq!(
            serde_json::to_string(&HostType::TvCast).unwrap(),
            "\"tv-cast\""
        );
        assert_eq!(
            serde_json::to_string(&HostType::GameConsole).unwrap(),
            "\"game-console\""
        );
        assert_eq!(serde_json::to_string(&HostType::Ap).unwrap(), "\"ap\"");
    }

    #[test]
    fn trust_state_serializes_to_hostfacts_lowercase_strings() {
        assert_eq!(
            serde_json::to_string(&TrustState::Trusted).unwrap(),
            "\"trusted\""
        );
        assert_eq!(
            serde_json::to_string(&TrustState::Unknown).unwrap(),
            "\"unknown\""
        );
        assert_eq!(
            serde_json::to_string(&TrustState::Blocked).unwrap(),
            "\"blocked\""
        );
        assert_eq!(TrustState::default(), TrustState::Unknown);
    }

    // ── MESH-A-4.b.1: mDNS collector ──

    #[test]
    fn parse_avahi_browse_keeps_resolved_skips_browse_lines() {
        let raw = "+;eth0;IPv4;HP\\032LaserJet;_ipp._tcp;local\n\
                   =;eth0;IPv4;HP\\032LaserJet;_ipp._tcp;local;printer.local;192.168.1.50;631;\"txtvers=1\"\n\
                   =;eth0;IPv4;Chromecast;_googlecast._tcp;local;cast.local;192.168.1.60;8009;\"\"\n";
        let recs = parse_avahi_browse(raw);
        assert_eq!(recs.len(), 2, "the + browse line is skipped");
        assert_eq!(recs[0].ip, "192.168.1.50");
        assert_eq!(recs[0].service_type, "_ipp._tcp");
        assert_eq!(recs[0].hostname, "printer.local");
        assert_eq!(recs[1].ip, "192.168.1.60");
        assert_eq!(recs[1].service_type, "_googlecast._tcp");
    }

    #[test]
    fn hosts_from_mdns_groups_by_ip_and_classifies() {
        let recs = vec![
            MdnsService {
                ip: "192.168.1.50".into(),
                hostname: "printer.local".into(),
                service_type: "_ipp._tcp".into(),
            },
            MdnsService {
                ip: "192.168.1.50".into(),
                hostname: "printer.local".into(),
                service_type: "_pdl-datastream._tcp".into(),
            },
            MdnsService {
                ip: "192.168.1.60".into(),
                hostname: "cast.local".into(),
                service_type: "_googlecast._tcp".into(),
            },
        ];
        let hosts = hosts_from_mdns(&recs, 1234);
        assert_eq!(hosts.len(), 2, "two distinct IPs → two hosts");
        let printer = hosts.iter().find(|h| h.ip == "192.168.1.50").unwrap();
        assert_eq!(printer.host_type, HostType::Printer);
        assert_eq!(printer.services.len(), 2, "both service types retained");
        assert_eq!(printer.hostname, "printer.local");
        assert_eq!(printer.first_seen_ms, 1234);
        assert_eq!(printer.last_seen_ms, 1234);
        assert_eq!(printer.trust, TrustState::Unknown);
        assert!(printer.mac.is_empty(), "MAC fills in A-4.b.2");
        let cast = hosts.iter().find(|h| h.ip == "192.168.1.60").unwrap();
        assert_eq!(cast.host_type, HostType::TvCast);
    }

    // ── MESH-A-4.b.2: hostname hint + reverse-DNS ──

    #[test]
    fn console_hostname_hint_outranks_media_service() {
        // A PS4 advertises _spotify-connect (→ smart-speaker by service
        // type) but its hostname pins it to a game console.
        let sig = HostSignals {
            mdns_services: vec!["_spotify-connect._tcp".to_string()],
            hostname: "PS4-64F7B2.local".to_string(),
            ..Default::default()
        };
        assert_eq!(classify(&sig), HostType::GameConsole);
    }

    #[test]
    fn host_type_from_hostname_matches_consoles_only() {
        assert_eq!(
            host_type_from_hostname("PS5-1234"),
            Some(HostType::GameConsole)
        );
        assert_eq!(
            host_type_from_hostname("Xbox-Living-Room"),
            Some(HostType::GameConsole)
        );
        assert_eq!(
            host_type_from_hostname("nintendo-switch"),
            Some(HostType::GameConsole)
        );
        assert_eq!(host_type_from_hostname("fileserver.local"), None);
        assert_eq!(host_type_from_hostname(""), None);
    }

    #[test]
    fn empty_hostname_preserves_prior_classification() {
        // No hostname → mDNS still wins (A-4.a behaviour unchanged).
        let sig = HostSignals {
            mdns_services: vec!["_ipp._tcp".to_string()],
            ..Default::default()
        };
        assert_eq!(classify(&sig), HostType::Printer);
    }

    #[test]
    fn parse_getent_hosts_extracts_canonical_name() {
        assert_eq!(
            parse_getent_hosts("192.168.1.50   printer.local").as_deref(),
            Some("printer.local")
        );
        assert_eq!(
            parse_getent_hosts("192.168.1.60 cast.local alias1 alias2").as_deref(),
            Some("cast.local")
        );
        assert_eq!(parse_getent_hosts(""), None);
        assert_eq!(parse_getent_hosts("192.168.1.99"), None); // no name field
    }

    // ── MESH-A-4.b.3: MAC-OUI → vendor ──

    #[test]
    fn mac_oui_prefix_normalises_separators() {
        assert_eq!(
            mac_oui_prefix("00:1a:2b:cc:dd:ee").as_deref(),
            Some("001A2B")
        );
        assert_eq!(
            mac_oui_prefix("00-1A-2B-cc-dd-ee").as_deref(),
            Some("001A2B")
        );
        assert_eq!(mac_oui_prefix("001a2bccddee").as_deref(), Some("001A2B"));
        assert_eq!(mac_oui_prefix("00:1a"), None); // < 3 octets of hex
    }

    #[test]
    fn parse_oui_db_and_vendor_lookup() {
        let db = parse_oui_db(
            "# nmap-mac-prefixes\n\
             001A2B Hewlett Packard\n\
             FFFFFF Some Vendor\n\
             badline_no_whitespace\n\
             00 TooShort\n",
        );
        assert_eq!(db.len(), 2, "comment / no-whitespace / short lines skipped");
        assert_eq!(
            db.vendor_for("00:1a:2b:cc:dd:ee").as_deref(),
            Some("Hewlett Packard")
        );
        assert_eq!(
            db.vendor_for("FF-FF-FF-00-00-00").as_deref(),
            Some("Some Vendor")
        );
        assert_eq!(db.vendor_for("12:34:56:78:90:ab"), None);
        assert!(db.vendor_for("zz").is_none()); // unparseable MAC
    }

    #[test]
    fn oui_vendor_feeds_the_classifier() {
        // An HP-OUI MAC resolves to a printer vendor, which classify
        // maps to Printer via host_type_from_vendor.
        let db = parse_oui_db("001A2B Hewlett Packard\n");
        let vendor = db.vendor_for("00:1a:2b:00:00:01").unwrap();
        let sig = HostSignals {
            oui_vendor: vendor,
            ..Default::default()
        };
        assert_eq!(classify(&sig), HostType::Printer);
    }

    // ── MESH-A-4.c.1: ARP-MAC + OUI enrichment sweep ──

    fn bare_host(ip: &str, services: &[&str], host_type: HostType) -> SurroundingHost {
        SurroundingHost {
            ip: ip.into(),
            mac: String::new(),
            vendor: String::new(),
            hostname: String::new(),
            services: services.iter().map(|s| (*s).to_string()).collect(),
            host_type,
            trust: TrustState::Unknown,
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    #[test]
    fn parse_neigh_map_extracts_ip_to_mac() {
        let raw = "192.168.1.1 dev eth0 lladdr 00:00:0c:aa:bb:cc REACHABLE\n\
                   192.168.1.2 dev eth0 FAILED\n\
                   192.168.1.3 dev eth0 lladdr AA:BB:CC:DD:EE:FF STALE\n";
        let m = parse_neigh_map(raw);
        assert_eq!(m.len(), 2, "the lladdr-less FAILED entry is skipped");
        assert_eq!(
            m.get("192.168.1.1").map(String::as_str),
            Some("00:00:0c:aa:bb:cc")
        );
        assert_eq!(
            m.get("192.168.1.3").map(String::as_str),
            Some("aa:bb:cc:dd:ee:ff")
        ); // lowercased
    }

    #[test]
    fn enrich_fills_mac_vendor_and_types_a_mdns_less_host() {
        let mut macs = HashMap::new();
        macs.insert("192.168.1.1".to_string(), "00:00:0c:aa:bb:cc".to_string());
        let oui = parse_oui_db("00000C Cisco Systems\n");
        let out = enrich_hosts(
            vec![bare_host("192.168.1.1", &[], HostType::Unknown)],
            &macs,
            &oui,
        );
        assert_eq!(out[0].mac, "00:00:0c:aa:bb:cc");
        assert_eq!(out[0].vendor, "Cisco Systems");
        assert_eq!(out[0].host_type, HostType::Router); // vendor typed it
    }

    #[test]
    fn enrich_keeps_a_confident_mdns_type() {
        let mut macs = HashMap::new();
        macs.insert("192.168.1.50".to_string(), "00:11:22:33:44:55".to_string());
        let oui = parse_oui_db("001122 Ubiquiti Inc\n");
        let out = enrich_hosts(
            vec![bare_host("192.168.1.50", &["_ipp._tcp"], HostType::Printer)],
            &macs,
            &oui,
        );
        assert_eq!(out[0].vendor, "Ubiquiti Inc"); // vendor recorded …
        assert_eq!(out[0].host_type, HostType::Printer); // … but mDNS still wins
    }

    #[test]
    fn enrich_without_a_mac_leaves_type_unchanged() {
        let out = enrich_hosts(
            vec![bare_host(
                "10.0.0.9",
                &["_googlecast._tcp"],
                HostType::TvCast,
            )],
            &HashMap::new(),
            &OuiTable::default(),
        );
        assert_eq!(out[0].host_type, HostType::TvCast);
        assert!(out[0].mac.is_empty());
    }

    // ── MESH-A-4.c.3: HTTP banner ──

    #[test]
    fn parse_http_server_extracts_case_insensitive() {
        let headers =
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSERVER: CUPS/2.4 IPP/2.1\r\n\r\n";
        assert_eq!(
            parse_http_server(headers).as_deref(),
            Some("CUPS/2.4 IPP/2.1")
        );
        assert_eq!(parse_http_server("HTTP/1.1 200 OK\r\n\r\n"), None);
        assert_eq!(parse_http_server("Server:   \r\n"), None); // empty value
    }

    #[test]
    fn host_type_from_http_server_maps_embedded_devices() {
        assert_eq!(
            host_type_from_http_server("CUPS/2.4"),
            Some(HostType::Printer)
        );
        assert_eq!(
            host_type_from_http_server("Hikvision-Webs"),
            Some(HostType::Camera)
        );
        assert_eq!(
            host_type_from_http_server("Synology DiskStation"),
            Some(HostType::Nas)
        );
        assert_eq!(
            host_type_from_http_server("RouterOS/7.1 (MikroTik)"),
            Some(HostType::Router)
        );
        // Generic web servers say nothing about device type.
        assert_eq!(host_type_from_http_server("nginx/1.24"), None);
        assert_eq!(host_type_from_http_server("Apache/2.4.57"), None);
    }

    // ── MESH-A-4.c.3.b: active nmap -O fingerprint ──

    #[test]
    fn parse_nmap_device_type_extracts_case_insensitive() {
        let out = "Nmap scan report for 10.0.0.5\nDEVICE TYPE: printer\nRunning: HP embedded\n";
        assert_eq!(parse_nmap_device_type(out).as_deref(), Some("printer"));
        // Ambiguous multi-guess values are returned verbatim (the mapping
        // resolves them below).
        let multi = "Device type: general purpose|router\nRunning: Linux 4.X\n";
        assert_eq!(
            parse_nmap_device_type(multi).as_deref(),
            Some("general purpose|router")
        );
        // No device-type line, and an empty value, both → None.
        assert_eq!(parse_nmap_device_type("Running: Linux 5.X\n"), None);
        assert_eq!(parse_nmap_device_type("Device type:   \n"), None);
    }

    #[test]
    fn host_type_from_nmap_device_type_maps_classes() {
        assert_eq!(
            host_type_from_nmap_device_type("printer"),
            Some(HostType::Printer)
        );
        assert_eq!(
            host_type_from_nmap_device_type("router"),
            Some(HostType::Router)
        );
        assert_eq!(host_type_from_nmap_device_type("WAP"), Some(HostType::Ap));
        assert_eq!(
            host_type_from_nmap_device_type("switch"),
            Some(HostType::Switch)
        );
        assert_eq!(
            host_type_from_nmap_device_type("webcam"),
            Some(HostType::Camera)
        );
        assert_eq!(
            host_type_from_nmap_device_type("storage-misc"),
            Some(HostType::Nas)
        );
        assert_eq!(
            host_type_from_nmap_device_type("media device"),
            Some(HostType::TvCast)
        );
        assert_eq!(
            host_type_from_nmap_device_type("game console"),
            Some(HostType::GameConsole)
        );
        assert_eq!(
            host_type_from_nmap_device_type("phone"),
            Some(HostType::Phone)
        );
        // First specific match wins across `|`-separated guesses.
        assert_eq!(
            host_type_from_nmap_device_type("general purpose|router"),
            Some(HostType::Router)
        );
        // The generic class alone is too weak to pin a type → Unknown.
        assert_eq!(host_type_from_nmap_device_type("general purpose"), None);
    }

    // ── MESH-A-4.c.4: coalescing + union reader ──

    fn seen_host(ip: &str, mac: &str, seen: i64, ty: HostType) -> SurroundingHost {
        SurroundingHost {
            ip: ip.into(),
            mac: mac.into(),
            vendor: String::new(),
            hostname: String::new(),
            services: vec![],
            host_type: ty,
            trust: TrustState::Unknown,
            first_seen_ms: seen,
            last_seen_ms: seen,
        }
    }

    #[test]
    fn coalesce_groups_by_mac_with_roaming_and_sightings() {
        let raw = vec![
            seen_host("10.0.0.5", "aa:bb", 100, HostType::Unknown),
            seen_host("10.0.0.9", "aa:bb", 300, HostType::Router), // same MAC, newer, roamed IP
            seen_host("10.0.0.7", "", 200, HostType::Printer),     // no MAC → keyed by IP
        ];
        let out = coalesce_sightings(raw);
        assert_eq!(out.len(), 2, "aa:bb coalesced + the MAC-less .7");
        let mac_card = out.iter().find(|c| c.key == "aa:bb").unwrap();
        assert_eq!(mac_card.sightings, 2);
        assert_eq!(mac_card.host.last_seen_ms, 300, "freshest sighting wins");
        assert_eq!(mac_card.host.host_type, HostType::Router);
        assert_eq!(
            mac_card.ips_seen,
            vec!["10.0.0.5", "10.0.0.9"],
            "roaming history"
        );
        let ip_card = out.iter().find(|c| c.key == "10.0.0.7").unwrap();
        assert_eq!(ip_card.sightings, 1);
    }

    #[test]
    fn read_all_unions_latest_per_peer_and_coalesces() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // peerA saw the host at .5; peerB saw the same MAC at .9.
        std::fs::create_dir_all(root.join("peerA")).unwrap();
        std::fs::write(
            root.join("peerA").join("20260101T000000-a.json"),
            serde_json::to_string(&vec![seen_host("10.0.0.5", "aa:bb", 1, HostType::Unknown)])
                .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("peerB")).unwrap();
        std::fs::write(
            root.join("peerB").join("20260101T000000-b.json"),
            serde_json::to_string(&vec![seen_host("10.0.0.9", "aa:bb", 2, HostType::Router)])
                .unwrap(),
        )
        .unwrap();
        let out = read_all_surrounding(root);
        assert_eq!(out.len(), 1, "both peers' sightings coalesce to one MAC");
        assert_eq!(out[0].sightings, 2, "seen by 2 peers");
        assert_eq!(out[0].ips_seen.len(), 2, "roaming across .5 and .9");
        assert_eq!(out[0].host.last_seen_ms, 2, "freshest wins");
    }

    #[test]
    fn read_all_empty_when_root_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_all_surrounding(&tmp.path().join("nope")).is_empty());
    }

    // ── MESH-A-4.d: trust persistence ──

    #[test]
    fn trust_store_round_trips_and_clears_on_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("surrounding").join("trust.json");
        set_host_trust(&path, "aa:bb", TrustState::Blocked).unwrap();
        set_host_trust(&path, "10.0.0.5", TrustState::Trusted).unwrap();
        let store = load_trust_store(&path);
        assert_eq!(store.get("aa:bb"), Some(&TrustState::Blocked));
        assert_eq!(store.get("10.0.0.5"), Some(&TrustState::Trusted));
        // Unknown clears the override.
        set_host_trust(&path, "aa:bb", TrustState::Unknown).unwrap();
        assert!(!load_trust_store(&path).contains_key("aa:bb"));
    }

    #[test]
    fn parse_trust_store_fail_open() {
        assert!(parse_trust_store("not json").is_empty());
        let s = parse_trust_store(r#"{"aa:bb":"blocked"}"#);
        assert_eq!(s.get("aa:bb"), Some(&TrustState::Blocked));
    }

    #[test]
    fn apply_trust_overrides_card_trust() {
        let mut cards = vec![CoalescedHost {
            key: "aa:bb".into(),
            host: seen_host("10.0.0.5", "aa:bb", 1, HostType::Router),
            sightings: 1,
            ips_seen: vec!["10.0.0.5".into()],
        }];
        assert_eq!(cards[0].host.trust, TrustState::Unknown);
        let mut store = TrustStore::new();
        store.insert("aa:bb".into(), TrustState::Blocked);
        apply_trust(&mut cards, &store);
        assert_eq!(cards[0].host.trust, TrustState::Blocked);
    }

    #[test]
    fn trust_state_wire_names() {
        assert_eq!(TrustState::Trusted.wire_name(), "trusted");
        assert_eq!(TrustState::Unknown.wire_name(), "unknown");
        assert_eq!(TrustState::Blocked.wire_name(), "blocked");
    }

    // ── MESH-A-5.1: blocked-host DROP planner ──

    #[test]
    fn blocked_ips_collects_roaming_ips_of_blocked_cards_only() {
        let mut blocked = CoalescedHost {
            key: "aa:bb".into(),
            host: seen_host("10.0.0.5", "aa:bb", 2, HostType::Unknown),
            sightings: 2,
            ips_seen: vec!["10.0.0.5".into(), "10.0.0.9".into()],
        };
        blocked.host.trust = TrustState::Blocked;
        // trust defaults Unknown → excluded.
        let other = CoalescedHost {
            key: "cc:dd".into(),
            host: seen_host("10.0.0.7", "cc:dd", 1, HostType::Printer),
            sightings: 1,
            ips_seen: vec!["10.0.0.7".into()],
        };
        let ips = blocked_ips(&[blocked, other]);
        assert_eq!(
            ips,
            vec!["10.0.0.5", "10.0.0.9"],
            "both roaming IPs, non-blocked excluded"
        );
    }

    #[test]
    fn drop_rich_rule_body_picks_family() {
        assert_eq!(
            drop_rich_rule_body("10.0.0.5"),
            r#"rule family="ipv4" source address="10.0.0.5" drop"#
        );
        assert!(drop_rich_rule_body("fe80::1").contains(r#"family="ipv6""#));
    }

    // ── MESH-A-6.1: ARP-spoof detection ──

    #[test]
    fn arp_spoof_flags_mac_with_multiple_ipv4s() {
        let mut neigh = HashMap::new();
        neigh.insert("192.168.1.1".to_string(), "aa:bb:cc:00:00:01".to_string()); // gateway
        neigh.insert("192.168.1.5".to_string(), "aa:bb:cc:00:00:02".to_string()); // a host
                                                                                  // Attacker MAC answers ARP for two IPv4s (gateway impersonation):
        neigh.insert("192.168.1.50".to_string(), "de:ad:be:ef:00:00".to_string());
        neigh.insert("192.168.1.60".to_string(), "de:ad:be:ef:00:00".to_string());
        let suspects = arp_spoof_suspects(&neigh);
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0].0, "de:ad:be:ef:00:00");
        assert_eq!(suspects[0].1, vec!["192.168.1.50", "192.168.1.60"]);
    }

    #[test]
    fn arp_spoof_ignores_dual_stack_single_mac() {
        // A normal dual-stack host: one MAC on its v4 + v6 — not a spoof.
        let mut neigh = HashMap::new();
        neigh.insert("192.168.1.5".to_string(), "aa:bb:cc:00:00:02".to_string());
        neigh.insert("fe80::1".to_string(), "aa:bb:cc:00:00:02".to_string());
        assert!(arp_spoof_suspects(&neigh).is_empty());
    }

    // ── MESH-A-6.2: rogue-DHCP detection ──

    #[test]
    fn parse_dhcp_servers_extracts_distinct_server_ids() {
        let out = "Pre-scan script results:\n\
                   | broadcast-dhcp-discover: \n\
                   |   Response 1 of 2: \n\
                   |     IP Offered: 192.168.1.50\n\
                   |     Server Identifier: 192.168.1.1\n\
                   |   Response 2 of 2: \n\
                   |     IP Offered: 192.168.1.51\n\
                   |_    Server Identifier: 192.168.1.250\n";
        let servers = parse_dhcp_servers(out);
        assert_eq!(
            servers,
            vec!["192.168.1.1", "192.168.1.250"],
            "2 servers → rogue"
        );
    }

    #[test]
    fn parse_dhcp_servers_single_and_none_and_dedup() {
        assert_eq!(
            parse_dhcp_servers("|     Server Identifier: 10.0.0.1\n"),
            vec!["10.0.0.1"]
        );
        assert!(parse_dhcp_servers("no dhcp output here").is_empty());
        // Same server quoted twice → deduped.
        assert_eq!(
            parse_dhcp_servers("| Server Identifier: 10.0.0.1\n| Server Identifier: 10.0.0.1\n"),
            vec!["10.0.0.1"]
        );
    }

    // ── MESH-A-6.4: captive-portal detection ──

    #[test]
    fn captive_portal_detection() {
        // Clear — generate_204 returns 204.
        assert_eq!(
            captive_portal_from_headers("HTTP/1.1 204 No Content\r\n\r\n"),
            None
        );
        // Captive: 302 redirect to the portal.
        assert_eq!(
            captive_portal_from_headers(
                "HTTP/1.1 302 Found\r\nLocation: http://portal.lan/login\r\n\r\n"
            )
            .as_deref(),
            Some("http://portal.lan/login")
        );
        // Captive: 200 splash, no redirect → empty portal URL.
        assert_eq!(
            captive_portal_from_headers("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n")
                .as_deref(),
            Some("")
        );
        // No parseable status → treated as clear (offline, not captive).
        assert_eq!(captive_portal_from_headers(""), None);
    }

    // ── MESH-A-6.5: DNS-leak detection ──

    #[test]
    fn parse_resolv_nameservers_extracts_and_dedups() {
        let c = "# comment\nnameserver 10.42.0.1\nsearch lan\nnameserver 8.8.8.8\nnameserver 10.42.0.1\n";
        assert_eq!(parse_resolv_nameservers(c), vec!["10.42.0.1", "8.8.8.8"]);
        assert!(parse_resolv_nameservers("search lan\noptions ndots:1\n").is_empty());
    }

    #[test]
    fn dns_leak_flags_off_mesh_resolvers() {
        let expected = vec!["10.42.0.1".to_string()];
        let current = vec!["10.42.0.1".to_string(), "8.8.8.8".to_string()];
        assert_eq!(dns_leak(&current, &expected), vec!["8.8.8.8"]);
        assert!(dns_leak(&["10.42.0.1".to_string()], &expected).is_empty());
    }

    // ── MESH-A-6.3: evil-twin AP detection ──

    #[test]
    fn split_terse_unescapes_colons() {
        assert_eq!(
            split_terse(r"Coffee\:Shop:AA\:BB\:CC\:DD\:EE\:FF"),
            vec!["Coffee:Shop", "AA:BB:CC:DD:EE:FF"]
        );
    }

    #[test]
    fn parse_wifi_bssids_pairs_and_skips_hidden() {
        let out = "HomeNet:AA\\:BB\\:CC\\:DD\\:EE\\:FF\n:11\\:22\\:33\\:44\\:55\\:66\n";
        let aps = parse_wifi_bssids(out);
        assert_eq!(aps.len(), 1, "empty-SSID hidden network skipped");
        assert_eq!(
            aps[0],
            ("HomeNet".to_string(), "AA:BB:CC:DD:EE:FF".to_string())
        );
    }

    #[test]
    fn evil_twin_flags_known_ssid_on_new_bssid() {
        let mut baseline = WifiBaseline::new();
        learn_wifi(
            &mut baseline,
            &[("HomeNet".into(), "AA:BB:CC:DD:EE:FF".into())],
        );
        // Same SSID, attacker BSSID → evil twin.
        let attack = vec![("HomeNet".to_string(), "DE:AD:BE:EF:00:00".to_string())];
        assert_eq!(evil_twin_suspects(&attack, &baseline), attack);
        // Same SSID + known BSSID → not flagged.
        let ok = vec![("HomeNet".to_string(), "AA:BB:CC:DD:EE:FF".to_string())];
        assert!(evil_twin_suspects(&ok, &baseline).is_empty());
        // Never-seen SSID → not flagged (first sighting only learned).
        let fresh = vec![("Cafe".to_string(), "11:22:33:44:55:66".to_string())];
        assert!(evil_twin_suspects(&fresh, &baseline).is_empty());
    }

    // ── MESH-A-6.8: persistent-attack accumulation ──

    #[test]
    fn accumulate_alert_coalesces_within_window_resets_after() {
        let mut store = AlertStore::new();
        accumulate_alert(&mut store, "10.0.0.66", 1_000);
        assert_eq!(store.get("10.0.0.66").unwrap().count, 1);
        // A hit a minute later → coalesced (count 2, same first_seen).
        accumulate_alert(&mut store, "10.0.0.66", 61_000);
        let a = store.get("10.0.0.66").unwrap();
        assert_eq!(a.count, 2);
        assert_eq!(a.first_seen_ms, 1_000);
        assert_eq!(a.last_seen_ms, 61_000);
        // A hit > 24h after the last → fresh alert (count resets).
        accumulate_alert(&mut store, "10.0.0.66", 61_000 + ALERT_QUIET_MS + 1);
        assert_eq!(store.get("10.0.0.66").unwrap().count, 1);
    }

    #[test]
    fn auto_ack_drops_only_stale_alerts() {
        let mut store = AlertStore::new();
        accumulate_alert(&mut store, "recent", 1_000_000);
        accumulate_alert(&mut store, "stale", 1_000);
        // ~24h+1ms after `stale`'s hit, but < 24h after `recent`'s.
        let now = 1_000 + ALERT_QUIET_MS + 1;
        let acked = auto_ack(&mut store, now);
        assert_eq!(acked, vec!["stale"]);
        assert!(store.contains_key("recent"));
        assert!(!store.contains_key("stale"));
    }

    // ── MESH-A-8.1: LAN MDE-peer detection ──

    #[test]
    fn is_mde_service_matches_markers() {
        assert!(is_mde_service("_map2-node._tcp"));
        assert!(is_mde_service("_mde-sync._tcp"));
        assert!(is_mde_service("_mackes._tcp"));
        assert!(!is_mde_service("_smb._tcp"));
        assert!(!is_mde_service("_googlecast._tcp"));
    }

    #[test]
    fn mde_peer_candidates_flags_mde_advertisers_only() {
        let hosts = vec![
            bare_host(
                "10.0.0.5",
                &["_map2-node._tcp", "_device-info._tcp"],
                HostType::Computer,
            ),
            bare_host("10.0.0.6", &["_smb._tcp"], HostType::Nas), // not MDE
        ];
        let cands = mde_peer_candidates(&hosts);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].0, "10.0.0.5");
    }
}
