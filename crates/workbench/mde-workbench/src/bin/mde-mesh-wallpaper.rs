//! PD-10 (Q25 / L21–L23) — the live mesh map as the Cosmic desktop
//! background.
//!
//! A layer-shell **Background** surface (now on the libcosmic fork's
//! vendored iced + native wlr-layer-shell, replacing the retired
//! iced_layershell 0.18) rendering the PD-7 map scene full-screen:
//! presence-styled nodes, RTT-shaped force layout, Gray 100 ground.
//! **Pure render** (L21): keyboard interactivity None and the
//! Background layer keep it ambient — interaction lives in the
//! Workbench. **Adaptive budget** (L22): no animation loop — the scene
//! redraws only when the data tick (30 s, or 5 min on battery)
//! actually changes the roster/RTT, so a quiet mesh costs idle CPU.
//!
//! Data: `mackesd peers --json` (the PD-1 join) + the mesh-latency
//! cache (the PD-6 probe) — the same sources the panel reads.

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use std::collections::HashMap;

use cosmic::iced::widget::canvas;
use cosmic::iced::{window, Element, Length, Subscription, Task, Theme};
use mde_workbench::cosmic_compat::prelude::*;
use mde_workbench::panels::peers::sample_flows;
use mde_workbench::panels::peers_map::{
    any_geo_known, geo_layout, read_latency_cache, read_latency_paths, MapNode, MapProgram,
};

fn main() -> Result<(), cosmic::iced::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // CUT-2: the fork's `daemon(boot, update, view)` takes the boot fn first
    // (returns the initial state + task); title/subscription/theme are builder
    // methods and the runner is `.run()`.
    cosmic::iced::daemon(|| (Wallpaper::default(), boot_task()), update, view)
        .title(namespace)
        .subscription(subscription)
        .theme(theme)
        .run()
}

/// The daemon title / namespace (the layer-surface namespace is set on
/// the surface settings below).
fn namespace(_state: &Wallpaper, _id: window::Id) -> String {
    "mde-mesh-wallpaper".to_string()
}

/// Carbon Gray 100 ground — the brand navy stays the logo's.
fn theme(_state: &Wallpaper, _id: window::Id) -> Theme {
    let p = mde_theme::Palette::dark();
    Theme::custom(
        "MDE Wallpaper".to_string(),
        cosmic::iced::theme::Palette {
            background: p.background.into_cosmic_color(),
            text: p.text.into_cosmic_color(),
            primary: p.accent.into_cosmic_color(),
            success: p.success.into_cosmic_color(),
            warning: p.warning.into_cosmic_color(),
            danger: p.danger.into_cosmic_color(),
        },
    )
}

#[derive(Default)]
struct Wallpaper {
    nodes: Vec<MapNode>,
    positions: HashMap<String, (f32, f32)>,
    /// MESHMAP-1 (W4) — whether the current node set is geo-placed (drives the
    /// faint map backdrop). False ⇒ plain force layout.
    geo: bool,
    /// MESHMAP-3 (W3) — last sampled per-node overlay throughput (0.0..=1.0),
    /// keyed by hostname incl. self (loopback). Drives the per-direction
    /// particle density; an empty/all-idle map runs no animation (W8).
    flows: HashMap<String, f64>,
    /// MESHMAP-6 — the REAL per-link tx/rx from the `link-traffic` collector
    /// cache (refreshed each roster tick). Where a peer is present here the
    /// proxy fold (`apply_flows_to`) leaves its tx alone (the collector wins),
    /// and these rates also feed the rx side of the `has_flow` idle gate so an
    /// idle mesh still arms no animation loop.
    real_flows: HashMap<String, mde_workbench::panels::peers_map::LinkFlow>,
    /// MESHMAP-3 (W3) — self's own normalized throughput (the self→peer stream's
    /// sender density). Cached from `flows[self_hostname]` on each sample.
    self_flow: f64,
    /// MESHMAP-5 (W8) — the particle animation phase (0.0..=1.0), advanced by
    /// the anim tick ONLY while traffic flows (zero-CPU idle).
    flow_phase: f32,
    /// This machine's hostname (for self detection + the self-flow key).
    self_hostname: String,
    /// MESHMAP-3 — hostname→overlay-IP for the flow sampler (Netdata target).
    /// `MapNode` carries only the hostname, so the IP lives here alongside it.
    ips: HashMap<String, String>,
}

impl Wallpaper {
    /// MESHMAP-5 (W8) — is any edge carrying enough flow to animate? The anim
    /// tick is subscribed only when this holds, so an idle mesh runs no loop.
    /// Reduce-motion always reports false (static edges, no animation — W8/WCAG).
    fn has_flow(&self) -> bool {
        if mde_workbench::live_theme::reduce_motion() {
            return false;
        }
        // The MESHMAP-3 proxy tx (self→peer busy-ness)...
        self.flows.values().any(|f| *f > 0.02)
            // ...OR MESHMAP-6's real per-link tx/rx (the collector covers the
            // reverse peer→self stream the proxy never could, so the idle gate
            // must see rx too — else a pure-rx edge would draw with no anim).
            || self
                .real_flows
                .values()
                .any(|f| f.tx > 0.02 || f.rx > 0.02)
    }
}

#[derive(Debug, Clone)]
enum Message {
    /// The periodic roster/geo data tick (roster + RTT + paths — L22 cadence).
    Refresh,
    Loaded(Box<LoadedData>),
    /// MESHMAP-3 — re-sample per-node overlay throughput (the ~3 s flow tick).
    FlowTick,
    FlowsSampled(HashMap<String, f64>),
    /// MESHMAP-5 — advance the particle phase (the ~90 ms anim tick; gated on
    /// `has_flow` so it only fires while traffic is moving).
    FlowAnim,
}

/// The roster-refresh payload: the rebuilt node set + whether it's geo-placed +
/// the hostname→overlay-IP map the flow sampler targets.
#[derive(Debug, Clone)]
struct LoadedData {
    nodes: Vec<MapNode>,
    geo: bool,
    self_hostname: String,
    ips: HashMap<String, String>,
    /// MESHMAP-6 — the REAL per-link tx/rx the collector covered this refresh.
    real_flows: HashMap<String, mde_workbench::panels::peers_map::LinkFlow>,
}

/// On battery? (sysfs probe — L22 pauses the cadence down to 5 min.)
fn on_battery() -> bool {
    std::fs::read_to_string("/sys/class/power_supply/AC/online")
        .or_else(|_| std::fs::read_to_string("/sys/class/power_supply/ADP1/online"))
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn subscription(s: &Wallpaper) -> Subscription<Message> {
    // L22 — the roster/geo refresh stays adaptive (30 s AC / 5 min battery).
    let period = if on_battery() { 300 } else { 30 };
    let roster =
        cosmic::iced::time::every(std::time::Duration::from_secs(period)).map(|_| Message::Refresh);
    // MESHMAP-3 — re-sample per-node throughput every ~3 s while there are
    // peers to sample (cheap blocking GETs run off-thread). Mirrors the Peers
    // panel's flow-data cadence.
    let mut subs = vec![roster];
    if !s.nodes.is_empty() && !mde_workbench::live_theme::reduce_motion() {
        subs.push(
            cosmic::iced::time::every(std::time::Duration::from_secs(3)).map(|_| Message::FlowTick),
        );
    }
    // MESHMAP-5 (W8) — the ~90 ms particle-animation tick is subscribed ONLY
    // while real traffic is flowing (and never under reduce-motion), so an idle
    // wallpaper runs no animation loop (zero idle CPU).
    if s.has_flow() {
        subs.push(
            cosmic::iced::time::every(std::time::Duration::from_millis(90))
                .map(|_| Message::FlowAnim),
        );
    }
    Subscription::batch(subs)
}

/// Boot: spawn the Background layer surface (native wlr-layer-shell on
/// the libcosmic fork) and kick off the first data fetch.
fn boot_task() -> Task<Message> {
    let id = window::Id::unique();
    Task::batch([
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            namespace: "mde-mesh-wallpaper".to_string(),
            // Fill the output; Background = under every window, over
            // cosmic-bg's static image when launched after it.
            size: None,
            exclusive_zone: -1,
            margin: Default::default(),
            anchor: Anchor::TOP
                .union(Anchor::BOTTOM)
                .union(Anchor::LEFT)
                .union(Anchor::RIGHT),
            layer: Layer::Background,
            // L21 — pure render: never takes the keyboard; clicks pass
            // through (the PD-10 contract).
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        }),
        refresh_task(),
    ])
}

/// Fetch the directory + latency + paths off-thread and build the node set.
fn refresh_task() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(|| {
                let raw = std::process::Command::new("mackesd")
                    .args(["peers", "--json"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                let rtt = read_latency_cache();
                // MESHMAP-4 (W7) — per-peer underlay path (direct vs relayed).
                let paths = read_latency_paths();
                // MESHMAP-6 — read the REAL per-link byte rates (the mackesd
                // `link-traffic` collector cache). A cheap local file read,
                // exactly like the latency cache above. Where present these are
                // the real per-link tx/rx; the MESHMAP-3 per-node `sample_flows`
                // proxy (below) only fills the self→peer (tx) direction for
                // peers the collector doesn't cover. Absent cache → empty → the
                // reverse (rx) stream stays off (honest: the proxy can't split
                // direction, so it never fakes a reverse flow).
                let flows = mde_workbench::panels::peers_map::read_link_traffic();
                let hostname = std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let lh_ips = mde_workbench::panels::peers_map::lighthouse_overlay_ips();
                let nodes = parse_nodes(&raw, &rtt, &paths, &flows, &hostname, &lh_ips);
                let geo = any_geo_known(&nodes);
                let ips = parse_overlay_ips(&raw);
                Box::new(LoadedData {
                    nodes,
                    geo,
                    self_hostname: hostname,
                    ips,
                    // MESHMAP-6 — the set of peers the real collector covers,
                    // so the proxy fold below treats them as already-real (tx)
                    // and never overwrites them, and the rx idle gate can see them.
                    real_flows: flows,
                })
            })
            .await
            .unwrap_or_else(|_| {
                Box::new(LoadedData {
                    nodes: Vec::new(),
                    geo: false,
                    self_hostname: String::new(),
                    ips: HashMap::new(),
                    real_flows: HashMap::new(),
                })
            })
        },
        Message::Loaded,
    )
}

/// MESHMAP-3 — sample every online peer's overlay throughput (+ self's own via
/// loopback) off-thread, reusing the Peers panel's `sample_flows` (no
/// reimplementation). `ips` is hostname→overlay-IP; flow is keyed by hostname so
/// the view reads peer + self flow.
fn flow_task(
    nodes: Vec<MapNode>,
    self_hostname: String,
    ips: HashMap<String, String>,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let mut targets: Vec<(String, String)> = nodes
                    .iter()
                    .filter(|n| !n.is_self && n.presence != "offline" && n.rtt_ms.is_some())
                    .filter_map(|n| {
                        ips.get(&n.hostname)
                            .filter(|ip| !ip.is_empty())
                            .map(|ip| (n.hostname.clone(), ip.clone()))
                    })
                    .collect();
                // Self's own throughput (its local Netdata on loopback).
                if !self_hostname.is_empty() {
                    targets.push((self_hostname, "127.0.0.1".to_string()));
                }
                // MESHMAP-6 — `sample_flows` now returns per-link `LinkFlow`; the
                // proxy only carries `tx` busy-ness (rx is always 0), so collapse
                // to the per-node f64 the wallpaper's proxy map expects.
                sample_flows(&targets)
                    .into_iter()
                    .map(|(host, f)| (host, f.tx))
                    .collect::<HashMap<String, f64>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::FlowsSampled,
    )
}

/// Parse the hostname→overlay-IP map from the `mackesd peers --json` reply (the
/// flow sampler's Netdata targets). Pure.
fn parse_overlay_ips(raw: &str) -> HashMap<String, String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return HashMap::new();
    };
    v.get("peers")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let host = p.get("hostname")?.as_str()?.to_string();
                    let ip = p.get("overlay_ip")?.as_str()?.to_string();
                    (!ip.is_empty()).then_some((host, ip))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build MapNodes from the `mackesd peers --json` reply (pure). `lh_ips` is the
/// lighthouse overlay-IP set (LIGHTHOUSE-9, via `peers_map::lighthouse_overlay_ips`);
/// a node is flagged a lighthouse via the shared `peers_map::is_lighthouse`
/// predicate (overlay-IP membership OR `role == lighthouse`) so the wallpaper
/// hero agrees with the in-app Peers Map + Fleet rollup. `paths` resolves each
/// peer's relay (W7): a relayed peer's `relay_via` overlay-IP is mapped to the
/// relaying node's hostname so the draw pass bends the edge through it.
/// MESHMAP-6 — `flows` is the REAL per-link tx/rx from the `link-traffic`
/// collector cache: where a peer is present its `flow` (self→peer tx) and
/// `flow_rx` (peer→self rx) come straight off the collector; an uncovered peer
/// gets `flow: 0.0` here and is filled by the MESHMAP-3 `sample_flows` proxy
/// later (`apply_flows_to`), while `flow_rx` stays 0 (the proxy can't split
/// direction, so the reverse stream honestly stays off).
fn parse_nodes(
    raw: &str,
    rtt: &HashMap<String, Option<f64>>,
    paths: &HashMap<String, mde_workbench::panels::peers_map::PathInfo>,
    flows: &HashMap<String, mde_workbench::panels::peers_map::LinkFlow>,
    self_hostname: &str,
    lh_ips: &[String],
) -> Vec<MapNode> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    let Some(arr) = v.get("peers").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    // overlay-IP → hostname, to resolve a relay IP to its node (W7).
    let ip_to_host: HashMap<&str, &str> = arr
        .iter()
        .filter_map(|p| {
            let host = p.get("hostname")?.as_str()?;
            let ip = p.get("overlay_ip")?.as_str()?;
            (!ip.is_empty()).then_some((ip, host))
        })
        .collect();
    arr.iter()
        .filter_map(|p| {
            let hostname = p.get("hostname")?.as_str()?.to_string();
            let presence = p
                .get("presence")
                .and_then(|x| x.as_str())
                .unwrap_or("offline")
                .to_string();
            let is_self = hostname == self_hostname;
            let rtt_ms = rtt.get(&hostname).copied().flatten();
            let overlay_ip = p.get("overlay_ip").and_then(|x| x.as_str()).unwrap_or("");
            let role = p.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let lighthouse =
                mde_workbench::panels::peers_map::is_lighthouse(role, overlay_ip, lh_ips);
            let relay_via = paths.get(&hostname).and_then(|pi| {
                pi.relay_via
                    .as_deref()
                    .and_then(|ip| ip_to_host.get(ip).map(|h| (*h).to_string()))
            });
            // MESHMAP-6 — the REAL per-link tx/rx from the `link-traffic`
            // collector cache (read once per refresh in `refresh_task`). Where
            // the collector covers this peer its rates land directly; otherwise
            // `flow` (tx) stays idle here and the MESHMAP-3 `sample_flows` proxy
            // fills it on the next FlowTick, while `flow_rx` stays 0 (the proxy
            // can't split direction → the reverse stream honestly stays off).
            let real = flows.get(&hostname).copied().unwrap_or_default();
            Some(MapNode {
                hostname,
                presence,
                rtt_ms,
                is_self,
                lighthouse,
                flow: real.tx,
                flow_rx: real.rx,
                relay_via,
            })
        })
        .collect()
}

fn update(state: &mut Wallpaper, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => refresh_task(),
        Message::Loaded(data) => {
            // Repaint only on actual change (L22 — a quiet mesh costs
            // nothing; iced only redraws when state mutates). The first load
            // (empty → populated) also arms the flow sampler.
            let first = state.nodes.is_empty() && !data.nodes.is_empty();
            let mut data = data;
            // MESHMAP-6 — the freshly-read real per-link flows (collector cache)
            // are already baked onto the parsed nodes; cache them for the rx idle
            // gate + the proxy-fallback skip set.
            state.real_flows = data.real_flows.clone();
            // Carry the live proxy flows onto the freshly-parsed nodes so a roster
            // refresh doesn't blink the particles off until the next FlowTick —
            // and so the change check compares apples to apples (roster identity,
            // not transient flow). The proxy only fills peers the collector
            // doesn't cover (MESHMAP-6 collector wins; see `apply_flows_to`).
            let flows = state.flows.clone();
            apply_flows_to(&mut data.nodes, &flows, &data.real_flows);
            if data.nodes != state.nodes || data.geo != state.geo {
                // MESHMAP-1 (W4) — geo placement when any node is region-known.
                state.positions = geo_layout(&data.nodes);
                state.geo = data.geo;
                state.self_hostname = data.self_hostname.clone();
                state.ips = data.ips.clone();
                state.nodes = data.nodes;
            }
            if first {
                return flow_task(
                    state.nodes.clone(),
                    state.self_hostname.clone(),
                    state.ips.clone(),
                );
            }
            Task::none()
        }
        Message::FlowTick => {
            if state.nodes.is_empty() {
                return Task::none();
            }
            flow_task(
                state.nodes.clone(),
                state.self_hostname.clone(),
                state.ips.clone(),
            )
        }
        Message::FlowsSampled(flows) => {
            // Fold the sampled per-node proxy flow onto the nodes (drives the
            // self→peer stream density for peers the collector doesn't cover) +
            // cache self's own (the self→peer stream). MESHMAP-6: peers the
            // real `link-traffic` collector covers keep their collector rate.
            self_apply_flows(state, &flows);
            state.flows = flows;
            Task::none()
        }
        Message::FlowAnim => {
            // Advance the particle phase; wraps at 1.0 (W8: only ticked while
            // `has_flow`, so an idle mesh never reaches here).
            state.flow_phase = (state.flow_phase + 0.06).fract();
            Task::none()
        }
    }
}

/// Fold sampled per-node flow onto the node set + cache self's own throughput.
fn self_apply_flows(state: &mut Wallpaper, flows: &HashMap<String, f64>) {
    let real = state.real_flows.clone();
    apply_flows_to(&mut state.nodes, flows, &real);
    state.self_flow = flows.get(&state.self_hostname).copied().unwrap_or(0.0);
}

/// Fold the per-node sources onto a node set. MESHMAP-6 — the real per-link
/// collector (`real`) is authoritative: a covered peer keeps its collector tx
/// (`flow`) + rx (`flow_rx`); an uncovered peer falls back to the MESHMAP-3
/// `sample_flows` per-node proxy (`flows`) for `flow` (tx busy-ness) and stays
/// `flow_rx: 0.0` (the proxy can't split direction → the reverse stream honestly
/// stays off). Self's node `flow` stays 0.0 (self doesn't ride its own edges —
/// `self_flow` drives the self→peer stream). Pure on `nodes`.
fn apply_flows_to(
    nodes: &mut [MapNode],
    flows: &HashMap<String, f64>,
    real: &HashMap<String, mde_workbench::panels::peers_map::LinkFlow>,
) {
    for n in nodes.iter_mut() {
        if n.is_self {
            n.flow = 0.0;
            n.flow_rx = 0.0;
            continue;
        }
        if let Some(rf) = real.get(&n.hostname) {
            // Collector covers this peer → real tx/rx win over the proxy.
            n.flow = rf.tx;
            n.flow_rx = rf.rx;
        } else {
            // Fallback: per-node proxy tx; no real rx data → reverse stream off.
            n.flow = flows.get(&n.hostname).copied().unwrap_or(0.0);
            n.flow_rx = 0.0;
        }
    }
}

fn view(state: &Wallpaper, _id: window::Id) -> Element<'_, Message, Theme> {
    // The MapProgram publishes workbench messages on click; the
    // wallpaper never receives them (KeyboardInteractivity::None +
    // Background layer), and we drop any that arrive by mapping the
    // canvas into the wallpaper's own Refresh message.
    let prog = MapProgram {
        nodes: state.nodes.clone(),
        positions: state.positions.clone(),
        palette: mde_theme::Palette::dark(),
        flow_phase: state.flow_phase,
        self_flow: state.self_flow,
        geo: state.geo,
        // MESHMAP-5 (W8) — reduce-motion → static colored edges (no particles).
        reduce_motion: mde_workbench::live_theme::reduce_motion(),
    };
    let map: Element<'_, mde_workbench::Message, Theme> =
        canvas(prog).width(Length::Fill).height(Length::Fill).into();
    map.map(|_| Message::Refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared empty per-link flow map for the parse tests that don't
    /// exercise MESHMAP-6 flow wiring.
    fn no_flows() -> std::collections::HashMap<String, mde_workbench::panels::peers_map::LinkFlow> {
        std::collections::HashMap::new()
    }

    #[test]
    fn nodes_parse_from_the_peers_json() {
        let raw = r#"{"ok":true,"head":null,"peers":[
            {"hostname":"pine","presence":"online"},
            {"hostname":"oak","presence":"offline"}]}"#;
        let mut rtt = std::collections::HashMap::new();
        rtt.insert("oak".to_string(), Some(20.0));
        let paths = HashMap::new();
        let nodes = parse_nodes(raw, &rtt, &paths, &no_flows(), "pine", &[]);
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_self);
        assert_eq!(nodes[1].rtt_ms, Some(20.0));
        assert!(!nodes[0].lighthouse, "no lighthouse signal → plain peer");
        assert!(parse_nodes("junk", &rtt, &paths, &no_flows(), "x", &[]).is_empty());
    }

    #[test]
    fn meshmap6_real_per_link_flow_feeds_the_wallpaper() {
        // MESHMAP-6 — the collector cache's per-link tx/rx lands on the node.
        let raw = r#"{"peers":[
            {"hostname":"anvil","presence":"online","overlay_ip":"10.42.0.5"}]}"#;
        let rtt = std::collections::HashMap::new();
        let paths = HashMap::new();
        let mut flows = std::collections::HashMap::new();
        flows.insert(
            "anvil".to_string(),
            mde_workbench::panels::peers_map::LinkFlow { tx: 0.42, rx: 0.11 },
        );
        let nodes = parse_nodes(raw, &rtt, &paths, &flows, "self", &[]);
        assert_eq!(nodes.len(), 1);
        assert!(
            (nodes[0].flow - 0.42).abs() < 1e-9,
            "real tx feeds self→peer"
        );
        assert!(
            (nodes[0].flow_rx - 0.11).abs() < 1e-9,
            "real rx feeds peer→self"
        );
        // Absent cache → no particles (the wallpaper never ran the proxy).
        let bare = parse_nodes(raw, &rtt, &paths, &no_flows(), "self", &[]);
        assert_eq!(bare[0].flow, 0.0);
        assert_eq!(bare[0].flow_rx, 0.0);
    }

    #[test]
    fn relay_via_resolves_to_the_relaying_node_hostname() {
        // MESHMAP-4 (W7) — a relayed peer's path `relay_via` overlay-IP is
        // mapped to the relaying node's hostname so the wallpaper bends the
        // edge through it; a direct path leaves `relay_via` None.
        use mde_workbench::panels::peers_map::PathInfo;
        let raw = r#"{"peers":[
            {"hostname":"self","presence":"online","overlay_ip":"10.42.0.6"},
            {"hostname":"lh","presence":"online","overlay_ip":"10.42.0.1","role":"lighthouse"},
            {"hostname":"forge","presence":"online","overlay_ip":"10.42.0.9"},
            {"hostname":"oak","presence":"online","overlay_ip":"10.42.0.8"}]}"#;
        let rtt: HashMap<String, Option<f64>> = [
            ("forge".to_string(), Some(60.0)),
            ("oak".to_string(), Some(8.0)),
        ]
        .into_iter()
        .collect();
        let paths: HashMap<String, PathInfo> = [
            (
                "forge".to_string(),
                PathInfo {
                    path: "relay".into(),
                    endpoint: None,
                    relay_via: Some("10.42.0.1".into()), // through the lighthouse
                },
            ),
            (
                "oak".to_string(),
                PathInfo {
                    path: "direct".into(),
                    endpoint: Some("203.0.113.5:4242".into()),
                    relay_via: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let nodes = parse_nodes(raw, &rtt, &paths, &no_flows(), "self", &["10.42.0.1".to_string()]);
        let by = |h: &str| nodes.iter().find(|n| n.hostname == h).unwrap();
        assert_eq!(
            by("forge").relay_via.as_deref(),
            Some("lh"),
            "relayed peer bends through the lighthouse node"
        );
        assert_eq!(by("oak").relay_via, None, "direct peer has no relay bend");
    }

    #[test]
    fn flow_folds_onto_peers_and_self_is_cached() {
        // MESHMAP-3 (W3) — sampled flow lands on each peer node (peer→self
        // stream density); self's own throughput is cached as `self_flow`
        // (self→peer stream) and never on self's node `flow`.
        let mut state = Wallpaper {
            self_hostname: "self".to_string(),
            nodes: vec![
                MapNode {
                    hostname: "self".into(),
                    presence: "online".into(),
                    rtt_ms: None,
                    is_self: true,
                    lighthouse: false,
                    flow: 0.0,
                    flow_rx: 0.0,
                    relay_via: None,
                },
                MapNode {
                    hostname: "forge".into(),
                    presence: "online".into(),
                    rtt_ms: Some(10.0),
                    is_self: false,
                    lighthouse: false,
                    flow: 0.0,
                    flow_rx: 0.0,
                    relay_via: None,
                },
            ],
            ..Wallpaper::default()
        };
        let flows: HashMap<String, f64> = [("self".to_string(), 0.7), ("forge".to_string(), 0.4)]
            .into_iter()
            .collect();
        self_apply_flows(&mut state, &flows);
        let forge = state.nodes.iter().find(|n| n.hostname == "forge").unwrap();
        let me = state.nodes.iter().find(|n| n.is_self).unwrap();
        assert!(
            (forge.flow - 0.4).abs() < 1e-9,
            "peer proxy flow (tx) folded onto node"
        );
        assert_eq!(
            forge.flow_rx, 0.0,
            "proxy can't split direction → reverse (rx) stream stays off"
        );
        assert_eq!(
            me.flow, 0.0,
            "self's node flow stays 0 (self_flow drives it)"
        );
        assert!((state.self_flow - 0.7).abs() < 1e-9, "self_flow cached");
    }

    #[test]
    fn meshmap6_real_collector_wins_over_the_proxy() {
        // MESHMAP-6 — where the `link-traffic` collector covers a peer, its real
        // per-link tx/rx win over the MESHMAP-3 per-node proxy (and feed the
        // reverse stream the proxy never could); an uncovered peer falls back to
        // the proxy tx with rx off.
        let mut state = Wallpaper {
            self_hostname: "self".to_string(),
            nodes: vec![
                MapNode {
                    hostname: "anvil".into(),
                    presence: "online".into(),
                    rtt_ms: Some(5.0),
                    is_self: false,
                    lighthouse: false,
                    flow: 0.0,
                    flow_rx: 0.0,
                    relay_via: None,
                },
                MapNode {
                    hostname: "forge".into(),
                    presence: "online".into(),
                    rtt_ms: Some(10.0),
                    is_self: false,
                    lighthouse: false,
                    flow: 0.0,
                    flow_rx: 0.0,
                    relay_via: None,
                },
            ],
            // anvil is collector-covered; forge is not.
            real_flows: [(
                "anvil".to_string(),
                mde_workbench::panels::peers_map::LinkFlow { tx: 0.9, rx: 0.3 },
            )]
            .into_iter()
            .collect(),
            ..Wallpaper::default()
        };
        // The proxy would attribute 0.4 to anvil + 0.5 to forge.
        let flows: HashMap<String, f64> = [("anvil".to_string(), 0.4), ("forge".to_string(), 0.5)]
            .into_iter()
            .collect();
        self_apply_flows(&mut state, &flows);
        let anvil = state.nodes.iter().find(|n| n.hostname == "anvil").unwrap();
        let forge = state.nodes.iter().find(|n| n.hostname == "forge").unwrap();
        assert!(
            (anvil.flow - 0.9).abs() < 1e-9,
            "collector tx wins over the proxy"
        );
        assert!(
            (anvil.flow_rx - 0.3).abs() < 1e-9,
            "collector feeds the reverse rx stream"
        );
        assert!(
            (forge.flow - 0.5).abs() < 1e-9,
            "uncovered peer keeps the proxy tx"
        );
        assert_eq!(forge.flow_rx, 0.0, "uncovered peer has no rx (proxy only)");
    }

    #[test]
    fn overlay_ips_parse_for_the_flow_sampler() {
        // MESHMAP-3 — the hostname→overlay-IP map (the Netdata flow targets).
        let raw = r#"{"peers":[
            {"hostname":"forge","overlay_ip":"10.42.0.9"},
            {"hostname":"oak","overlay_ip":""},
            {"hostname":"pine","overlay_ip":"10.42.0.8"}]}"#;
        let ips = parse_overlay_ips(raw);
        assert_eq!(ips.get("forge").map(String::as_str), Some("10.42.0.9"));
        assert_eq!(ips.get("pine").map(String::as_str), Some("10.42.0.8"));
        assert!(
            !ips.contains_key("oak"),
            "empty IP omitted (no sample target)"
        );
    }

    #[test]
    fn lighthouse_flagged_by_role_or_overlay_ip() {
        // Two anchor signals, one per peer: `lh-role` carries the directory
        // role; `lh-ip` is identified only by its overlay IP being in the
        // snapshot's lighthouse set (the LIGHTHOUSE-9 authoritative source).
        let raw = r#"{"peers":[
            {"hostname":"lh-role","presence":"online","role":"lighthouse","overlay_ip":"10.42.0.9"},
            {"hostname":"lh-ip","presence":"online","role":"server","overlay_ip":"10.42.0.1"},
            {"hostname":"worker","presence":"online","role":"server","overlay_ip":"10.42.0.5"}]}"#;
        let rtt = std::collections::HashMap::new();
        let paths = HashMap::new();
        let lh_ips = vec!["10.42.0.1".to_string()];
        let nodes = parse_nodes(raw, &rtt, &paths, &no_flows(), "self", &lh_ips);
        let by = |h: &str| nodes.iter().find(|n| n.hostname == h).unwrap().lighthouse;
        assert!(by("lh-role"), "role==lighthouse flags the anchor");
        assert!(
            by("lh-ip"),
            "overlay IP in the snapshot set flags the anchor"
        );
        assert!(!by("worker"), "a plain server is not a lighthouse");
    }

    #[test]
    fn lighthouse_ips_parse_from_the_snapshot() {
        // The snapshot parse + lighthouse predicate are the shared
        // `peers_map` helpers; assert the wallpaper relies on them correctly.
        use mde_workbench::panels::peers_map::{is_lighthouse, parse_lighthouse_ips};
        let raw =
            r#"{"network":{"lighthouse_ips":["10.42.0.1","10.42.0.2"],"cipher":"AES-256-GCM"}}"#;
        let ips = parse_lighthouse_ips(raw);
        assert_eq!(ips, vec!["10.42.0.1", "10.42.0.2"]);
        // Missing / malformed snapshot → empty (falls back to the role field).
        assert!(parse_lighthouse_ips("{}").is_empty());
        assert!(parse_lighthouse_ips("junk").is_empty());
        // An empty overlay_ip never matches even if the set has entries.
        assert!(is_lighthouse("server", "10.42.0.1", &ips));
        assert!(!is_lighthouse("server", "", &ips));
    }
}
