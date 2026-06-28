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
use cosmic::iced::widget::canvas;
use cosmic::iced::{window, Element, Length, Subscription, Task, Theme};
use mde_workbench::cosmic_compat::prelude::*;
use mde_workbench::panels::peers_map::{layout, read_latency_cache, MapNode, MapProgram};

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
    positions: std::collections::HashMap<String, (f32, f32)>,
}

#[derive(Debug, Clone)]
enum Message {
    /// The periodic data tick (the only repaint source — L22).
    Refresh,
    Loaded(Vec<MapNode>),
}

/// On battery? (sysfs probe — L22 pauses the cadence down to 5 min.)
fn on_battery() -> bool {
    std::fs::read_to_string("/sys/class/power_supply/AC/online")
        .or_else(|_| std::fs::read_to_string("/sys/class/power_supply/ADP1/online"))
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn subscription(_s: &Wallpaper) -> Subscription<Message> {
    let period = if on_battery() { 300 } else { 30 };
    cosmic::iced::time::every(std::time::Duration::from_secs(period)).map(|_| Message::Refresh)
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

/// Fetch the directory + latency off-thread and build the node set.
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
                // MESHMAP-6 — read the REAL per-link byte rates (the mackesd
                // `link-traffic` collector cache). A cheap local file read,
                // exactly like the latency cache above — NOT the per-node
                // Netdata sampling loop the in-app panel runs (the wallpaper
                // stays a pure ambient render). Absent cache → empty → the
                // flow particles stay off (honest: the wallpaper never ran the
                // proxy, so it shows real traffic or nothing).
                let flows = mde_workbench::panels::peers_map::read_link_traffic();
                let hostname = std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let lh_ips = mde_workbench::panels::peers_map::lighthouse_overlay_ips();
                parse_nodes(&raw, &rtt, &flows, &hostname, &lh_ips)
            })
            .await
            .unwrap_or_default()
        },
        Message::Loaded,
    )
}

/// Build MapNodes from the `mackesd peers --json` reply (pure). `lh_ips` is the
/// lighthouse overlay-IP set (LIGHTHOUSE-9, via `peers_map::lighthouse_overlay_ips`);
/// a node is flagged a lighthouse via the shared `peers_map::is_lighthouse`
/// predicate (overlay-IP membership OR `role == lighthouse`) so the wallpaper
/// hero agrees with the in-app Peers Map + Fleet rollup.
fn parse_nodes(
    raw: &str,
    rtt: &std::collections::HashMap<String, Option<f64>>,
    flows: &std::collections::HashMap<String, mde_workbench::panels::peers_map::LinkFlow>,
    self_hostname: &str,
    lh_ips: &[String],
) -> Vec<MapNode> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    v.get("peers")
        .and_then(|p| p.as_array())
        .map(|arr| {
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
                    let flow = flows.get(&hostname).copied().unwrap_or_default();
                    Some(MapNode {
                        hostname,
                        presence,
                        rtt_ms,
                        is_self,
                        lighthouse,
                        // MESHMAP-6 — the REAL per-link tx/rx from the
                        // `link-traffic` collector cache (read once per refresh,
                        // not a sampling loop). Absent cache → 0 → no particles
                        // (the wallpaper never ran the per-node proxy).
                        flow: flow.tx,
                        flow_rx: flow.rx,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn update(state: &mut Wallpaper, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => refresh_task(),
        Message::Loaded(nodes) => {
            // Repaint only on actual change (L22 — a quiet mesh costs
            // nothing; iced only redraws when state mutates).
            if nodes != state.nodes {
                state.positions = layout(&nodes);
                state.nodes = nodes;
            }
            Task::none()
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
        flow_phase: 0.0,
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
        let nodes = parse_nodes(raw, &rtt, &no_flows(), "pine", &[]);
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_self);
        assert_eq!(nodes[1].rtt_ms, Some(20.0));
        assert!(!nodes[0].lighthouse, "no lighthouse signal → plain peer");
        assert!(parse_nodes("junk", &rtt, &no_flows(), "x", &[]).is_empty());
    }

    #[test]
    fn meshmap6_real_per_link_flow_feeds_the_wallpaper() {
        // MESHMAP-6 — the collector cache's per-link tx/rx lands on the node.
        let raw = r#"{"peers":[
            {"hostname":"anvil","presence":"online","overlay_ip":"10.42.0.5"}]}"#;
        let rtt = std::collections::HashMap::new();
        let mut flows = std::collections::HashMap::new();
        flows.insert(
            "anvil".to_string(),
            mde_workbench::panels::peers_map::LinkFlow { tx: 0.42, rx: 0.11 },
        );
        let nodes = parse_nodes(raw, &rtt, &flows, "self", &[]);
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
        let bare = parse_nodes(raw, &rtt, &no_flows(), "self", &[]);
        assert_eq!(bare[0].flow, 0.0);
        assert_eq!(bare[0].flow_rx, 0.0);
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
        let lh_ips = vec!["10.42.0.1".to_string()];
        let nodes = parse_nodes(raw, &rtt, &no_flows(), "self", &lh_ips);
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
