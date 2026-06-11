//! PD-10 (Q25 / L21–L23) — the live mesh map as the Cosmic desktop
//! background.
//!
//! A layer-shell **Background** surface (the same iced 0.14 +
//! iced_layershell 0.18 pair the voice HUD ships) rendering the
//! PD-7 map scene full-screen: presence-styled nodes, RTT-shaped
//! force layout, Gray 100 ground. **Pure render** (L21): keyboard
//! interactivity None and the Background layer keep it ambient —
//! interaction lives in the Workbench. **Adaptive budget** (L22):
//! no animation loop — the scene redraws only when the data tick
//! (30 s, or 5 min on battery) actually changes the roster/RTT,
//! so a quiet mesh costs idle CPU.
//!
//! Data: `mackesd peers --json` (the PD-1 join) + the mesh-latency
//! cache (the PD-6 probe) — the same sources the panel reads.

use iced::widget::canvas;
use iced::{Element, Length, Subscription, Task, Theme};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::to_layer_message;
use mde_workbench::panels::peers_map::{layout, read_latency_cache, MapNode, MapProgram};

fn main() -> Result<(), iced_layershell::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    iced_layershell::application(
        || (Wallpaper::default(), refresh_task()),
        namespace,
        update,
        view,
    )
    .subscription(subscription)
    .theme(|_s: &Wallpaper| {
        // Carbon Gray 100 ground — the brand navy stays the logo's.
        let p = mde_theme::Palette::dark();
        Theme::custom(
            "MDE Wallpaper".to_string(),
            iced::theme::Palette {
                background: p.background.into_iced_color(),
                text: p.text.into_iced_color(),
                primary: p.accent.into_iced_color(),
                success: p.success.into_iced_color(),
                warning: p.warning.into_iced_color(),
                danger: p.danger.into_iced_color(),
            },
        )
    })
    .settings(Settings {
        id: Some("mde-mesh-wallpaper".to_string()),
        layer_settings: LayerShellSettings {
            // Fill the output; Background = under every window, over
            // cosmic-bg's static image when launched after it.
            size: None,
            exclusive_zone: -1,
            margin: (0, 0, 0, 0),
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            layer: Layer::Background,
            // L21 — pure render: never takes the keyboard.
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}

fn namespace() -> String {
    "mde-mesh-wallpaper".to_string()
}

#[derive(Default)]
struct Wallpaper {
    nodes: Vec<MapNode>,
    positions: std::collections::HashMap<String, (f32, f32)>,
}

// `to_layer_message` injects the layer-shell control variants the
// runtime requires on the message type.
#[to_layer_message]
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
    iced::time::every(std::time::Duration::from_secs(period)).map(|_| Message::Refresh)
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
                let hostname = std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                parse_nodes(&raw, &rtt, &hostname)
            })
            .await
            .unwrap_or_default()
        },
        Message::Loaded,
    )
}

/// Build MapNodes from the `mackesd peers --json` reply (pure).
fn parse_nodes(
    raw: &str,
    rtt: &std::collections::HashMap<String, Option<f64>>,
    self_hostname: &str,
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
                    Some(MapNode {
                        hostname,
                        presence,
                        rtt_ms,
                        is_self,
                        // The wallpaper is a pure ambient render (no Netdata
                        // sampling loop); flow particles stay off here.
                        flow: 0.0,
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
        // The to_layer_message control variants — a pure-render
        // wallpaper never drives them.
        _ => Task::none(),
    }
}

fn view(state: &Wallpaper) -> Element<'_, Message> {
    // The MapProgram publishes workbench messages on click; the
    // wallpaper never receives them (KeyboardInteractivity::None +
    // Background layer), and we drop any that arrive by mapping the
    // canvas into a unit element wrapped in `Ignored`.
    let prog = MapProgram {
        nodes: state.nodes.clone(),
        positions: state.positions.clone(),
        palette: mde_theme::Palette::dark(),
        flow_phase: 0.0,
    };
    let map: Element<'_, mde_workbench::Message> =
        canvas(prog).width(Length::Fill).height(Length::Fill).into();
    map.map(|_| Message::Refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_parse_from_the_peers_json() {
        let raw = r#"{"ok":true,"head":null,"peers":[
            {"hostname":"pine","presence":"online"},
            {"hostname":"oak","presence":"offline"}]}"#;
        let mut rtt = std::collections::HashMap::new();
        rtt.insert("oak".to_string(), Some(20.0));
        let nodes = parse_nodes(raw, &rtt, "pine");
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_self);
        assert_eq!(nodes[1].rtt_ms, Some(20.0));
        assert!(parse_nodes("junk", &rtt, "x").is_empty());
    }
}
