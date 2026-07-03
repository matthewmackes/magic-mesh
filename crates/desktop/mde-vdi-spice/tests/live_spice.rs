//! CHOOSER-5 live remainder — attach the REAL crate path to a REAL KVM SPICE
//! console.
//!
//! The unit suite proves the SPICE decode/input surface on synthetic bytes and
//! `tests/loopback_spice.rs` proves the connect path errors cleanly without a
//! server; this test proves the assembled stack against a live QEMU/KVM SPICE
//! console (the CHOOSER-5 acceptance "connects to a KVM VM's Spice console;
//! connect→a frame arrives"), mirroring the RDP/VNC live proofs
//! (`mde-vdi-rdp/tests/live_rdp.rs`, `mde-vdi-vnc/tests/live_console.rs`).
//!
//! Everything goes through the crate's public API — [`SpiceTransport::connect`]
//! runs the real `spice-client` connection + channel handshake,
//! `SpiceClientShared::start_event_loop` (driven on a background task) fills the
//! display channel, [`pump_frame`] folds the decoded primary surface into the
//! session via the same [`SpiceSession::apply_surface`] the unit tests drive,
//! [`frame`] yields the egui [`ColorImage`] the shell would upload, and
//! [`flush_input`] puts a real keystroke on the SPICE inputs channel.
//!
//! [`pump_frame`]: mde_vdi_spice::SpiceTransport::pump_frame
//! [`flush_input`]: mde_vdi_spice::SpiceTransport::flush_input
//! [`frame`]: mde_vdi_spice::SpiceSession::frame
//! [`SpiceSession::apply_surface`]: mde_vdi_spice::SpiceSession::apply_surface
//!
//! Env-gated + `#[ignore]` — a live SPICE console cannot exist in CI. Run:
//!
//! ```text
//! MDE_SPICE_LIVE_TARGET=127.0.0.1:5900 \
//!   cargo test -p mde-vdi-spice --test live_spice -- --ignored --nocapture
//! ```
//!
//! (target format `host:port[,password]`; the SPICE ticket is optional.)

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "test-only transport: a live-probe failure must abort with typed \
              evidence, and panicking IS the test failure mechanism"
)]

use std::time::{Duration, Instant};

use mde_vdi_spice::egui::{ColorImage, Event, Key, Modifiers};
use mde_vdi_spice::{SpiceConfig, SpiceSession, SpiceTransport};

/// FNV-1a 64 over the frame's RGBA bytes — a pixel checksum recorded as evidence.
fn fnv1a64(image: &ColorImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for px in &image.pixels {
        for byte in px.to_array() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Count of distinct colours — a real desktop is not a single flat fill.
fn distinct_colors(image: &ColorImage) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in &image.pixels {
        seen.insert(px.to_array());
    }
    seen.len()
}

/// Parse `host:port[,password]`.
fn parse_target(raw: &str) -> SpiceConfig {
    let (endpoint, password) = raw
        .split_once(',')
        .map_or((raw, None), |(e, p)| (e, Some(p)));
    let (host, port) = endpoint
        .rsplit_once(':')
        .expect("target must be host:port[,password]");
    let port: u16 = port.parse().expect("port must be a u16");
    let mut cfg = SpiceConfig::new(host).with_port(port);
    if let Some(password) = password {
        cfg = cfg.with_password(password);
    }
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live SPICE console required — set MDE_SPICE_LIVE_TARGET=host:port[,password] (see module docs)"]
async fn live_spice_console_connects_renders_and_accepts_input() {
    let Ok(target) = std::env::var("MDE_SPICE_LIVE_TARGET") else {
        panic!("set MDE_SPICE_LIVE_TARGET=host:port[,password]");
    };
    let cfg = parse_target(&target);
    let mut session = SpiceSession::new(cfg.clone()).expect("valid config");

    let transport = SpiceTransport::connect(&cfg)
        .await
        .expect("connect to the SPICE console");

    // Drive the client's message loop on a background task (it fills the display
    // channel's surfaces); the foreground pumps decoded surfaces into the session.
    // `SpiceClientShared` is Arc-backed + Clone, so the loop shares the client.
    let loop_client = transport.client().clone();
    let event_loop = tokio::spawn(async move { loop_client.start_event_loop().await });

    // Pump until a real primary surface arrives (bounded), then assert it renders.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut rendered = None;
    while Instant::now() < deadline {
        if transport.pump_frame(&mut session).await.expect("pump") {
            if let Some(img) = session.frame() {
                rendered = Some(img);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let img = rendered.expect("no SPICE frame arrived within 20 s");
    eprintln!(
        "live SPICE frame: {}x{} checksum={:016x} distinct_colors={}",
        img.size[0],
        img.size[1],
        fnv1a64(&img),
        distinct_colors(&img),
    );
    assert!(img.size[0] > 0 && img.size[1] > 0, "empty desktop");

    // Put a real keystroke on the wire (Enter) — proves the inputs channel.
    session.send_input(&Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    session.send_input(&Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    transport
        .flush_input(&mut session)
        .await
        .expect("flush input");

    event_loop.abort();
}
