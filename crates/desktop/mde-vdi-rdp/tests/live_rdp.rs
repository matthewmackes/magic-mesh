//! E12-4 live remainder — attach the REAL crate path to a REAL RDP server.
//!
//! The unit suite proves the decode→egui and egui→input surfaces on synthetic
//! bytes, and `connect.rs`'s own tests prove the config/input mapping; this
//! test proves the assembled stack against a live RDP endpoint (the E12-4
//! acceptance "an RDP connection to a test guest renders live"), mirroring the
//! E12-6 VNC proof (`mde-vdi-vnc/tests/live_console.rs`).
//!
//! Everything goes through the crate's public API — [`RdpConnection::connect`]
//! runs the real ironrdp connection sequence *built from the session's codec
//! tier* ([`RdpSession::connect_settings`], E12-10), the pump decodes real
//! framebuffer updates into the session via the same `apply_rect` path the
//! unit tests drive, [`RdpSession::frame`] yields the egui [`ColorImage`] the
//! shell would upload, and [`RdpSession::send_input`] +
//! [`RdpConnection::flush_input`] put a real keystroke on the wire. The tier
//! contract is then exercised end-to-end: pin a lighter tier, observe
//! `needs_reconnect`, reconnect, and render again at the new depth.
//!
//! Env-gated + `#[ignore]` — a live server cannot exist in CI. Run:
//!
//! ```text
//! MDE_RDP_LIVE_TARGET=127.0.0.1:13389,mde,mde-live-proof \
//!   cargo test -p mde-vdi-rdp --features live-connect --test live_rdp \
//!   -- --ignored --nocapture
//! ```
//!
//! (target format `host:port[,user,pass]`; user/pass default to the
//! `mde`/`mde-live-proof` fixture account of the xrdp proof container).

#![cfg(feature = "live-connect")]
#![allow(
    clippy::panic,
    reason = "test-only transport: a live-probe failure must abort with typed \
              wire-level evidence, and panicking IS the test failure mechanism"
)]

use std::time::{Duration, Instant};

use mde_vdi_rdp::egui::{ColorImage, Event, Key, Modifiers};
use mde_vdi_rdp::link::{QualityMode, QualityTier};
use mde_vdi_rdp::{PumpOutcome, RdpConfig, RdpConnection, RdpSession};

/// FNV-1a 64 over the frame's RGBA bytes — the pixel checksum recorded as
/// evidence (stable across runs for an unchanged screen).
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

/// Distinct RGBA values in the frame — a rendered desktop shows more than a
/// blank surface would; recorded (not hard-asserted).
fn distinct_colors(image: &ColorImage) -> usize {
    let mut seen: std::collections::HashSet<[u8; 4]> = std::collections::HashSet::with_capacity(64);
    for px in &image.pixels {
        seen.insert(px.to_array());
    }
    seen.len()
}

/// Parse `host:port[,user,pass]`, defaulting the credentials to the xrdp
/// proof-container fixture account.
fn parse_target(raw: &str) -> (String, u16, String, String) {
    let mut parts = raw.split(',');
    let hostport = parts.next().expect("split always yields one part");
    let (host, port_str) = hostport
        .rsplit_once(':')
        .expect("MDE_RDP_LIVE_TARGET must start with host:port");
    let port: u16 = port_str.parse().expect("MDE_RDP_LIVE_TARGET port parses");
    let user = parts.next().unwrap_or("mde").to_owned();
    let pass = parts.next().unwrap_or("mde-live-proof").to_owned();
    (host.to_owned(), port, user, pass)
}

/// Pump until at least one region has been painted AND the session yields a
/// frame, or the deadline passes. Returns the frame + how many regions were
/// painted getting there.
fn pump_until_frame(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    deadline: Duration,
    what: &str,
) -> (ColorImage, usize) {
    let start = Instant::now();
    let mut painted_total = 0_usize;
    while start.elapsed() < deadline {
        match conn.pump_once(session, Duration::from_secs(5)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects > 0 {
                    if let Some(frame) = session.frame() {
                        return (frame, painted_total);
                    }
                }
            }
            Ok(PumpOutcome::TimedOut) => {} // keep waiting inside the deadline
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while waiting for {what}: {reason}")
            }
            Err(e) => panic!("live: pump failed while waiting for {what}: {e}"),
        }
    }
    panic!(
        "live: no framebuffer update decoded for {what} within {}s ({painted_total} rects seen)",
        deadline.as_secs()
    );
}

/// Drain whatever the server still wants to send for up to `window`, then
/// return the latest frame if anything repainted.
fn settle(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    window: Duration,
) -> Option<ColorImage> {
    let start = Instant::now();
    let mut latest = None;
    while start.elapsed() < window {
        match conn.pump_once(session, Duration::from_millis(700)) {
            Ok(PumpOutcome::Processed { .. }) => {
                if let Some(frame) = session.frame() {
                    latest = Some(frame);
                }
            }
            Ok(PumpOutcome::TimedOut) => break, // server went quiet — settled
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated during settle: {reason}")
            }
            Err(e) => panic!("live: pump failed during settle: {e}"),
        }
    }
    latest
}

/// The live acceptance: real connection sequence (tier-driven), ≥1 real
/// framebuffer update decoded through the crate's public session path into an
/// egui [`ColorImage`], a keystroke forwarded (echo recorded honestly), and
/// the E12-10 tier contract exercised with a real reconnect at a lighter tier.
#[test]
#[ignore = "live RDP server required — set MDE_RDP_LIVE_TARGET=host:port[,user,pass] (see module docs)"]
#[allow(
    clippy::too_many_lines,
    reason = "one linear protocol script — connect → frame → input → tier \
              reconnect reads best unbroken, mirroring the E12-6 VNC proof"
)]
fn live_rdp_renders_accepts_input_and_applies_tier_on_reconnect() {
    let Ok(target) = std::env::var("MDE_RDP_LIVE_TARGET") else {
        eprintln!("live: SKIP — MDE_RDP_LIVE_TARGET not set (host:port[,user,pass])");
        return;
    };
    let (host, port, user, pass) = parse_target(&target);

    let config = RdpConfig::new(host, user, pass)
        .with_port(port)
        .with_resolution(1024, 768);
    let mut session = RdpSession::new(config).expect("live target config is valid");
    // Consume the initial all-black frame so the first frame we record below
    // is genuinely the decoded remote desktop, not the constructor's canvas.
    let _initial = session.frame();

    // ── Connect at the default tier (Full: 32-bpp, RemoteFX advertised) ─────
    assert_eq!(session.quality_tier(), QualityTier::Full);
    assert_eq!(session.connect_settings().color_depth, 32);
    let mut conn = RdpConnection::connect(&mut session)
        .unwrap_or_else(|e| panic!("live: connect failed: {e}"));
    assert!(
        !session.needs_reconnect(),
        "connect must mark the negotiated tier applied"
    );
    assert_eq!(session.applied_tier(), QualityTier::Full);
    let negotiated = conn.negotiated().clone();
    println!(
        "live: CONNECTED tier={:?} desktop={}x{} compression={:?} io_channel={} user_channel={}",
        negotiated.tier,
        negotiated.desktop_size.0,
        negotiated.desktop_size.1,
        negotiated.compression,
        negotiated.io_channel_id,
        negotiated.user_channel_id,
    );
    assert_eq!(
        negotiated.desktop_size,
        session.desktop_size(),
        "server must grant the requested desktop geometry"
    );

    // ── ≥1 real framebuffer update through the crate into an egui image ─────
    let (image, rects) = pump_until_frame(
        &mut conn,
        &mut session,
        Duration::from_secs(60),
        "the first desktop paint",
    );
    assert_eq!(
        image.size,
        [
            usize::from(negotiated.desktop_size.0),
            usize::from(negotiated.desktop_size.1)
        ],
        "frame geometry must match the negotiated desktop"
    );
    assert!(!image.pixels.is_empty(), "live frame decoded no pixels");
    let checksum = fnv1a64(&image);
    let colors = distinct_colors(&image);
    println!(
        "live: FRAME OK {}x{} rects={rects} fnv1a64={checksum:#018x} distinct_colors={colors}",
        image.size[0], image.size[1]
    );
    // Let the session finish painting (xrdp sends the desktop in waves) so the
    // input round-trip below diffs against a settled screen.
    let settled = settle(&mut conn, &mut session, Duration::from_secs(10));
    let baseline = settled.as_ref().map_or(checksum, fnv1a64);
    println!("live: settled baseline fnv1a64={baseline:#018x}");

    // ── Input round-trip (best effort, recorded honestly) ───────────────────
    // Type "m" + Enter through the same session API the shell drives; the
    // fixture xterm echoes, so pixels should move.
    session.send_input(&Event::Text("m".to_owned()));
    for pressed in [true, false] {
        session.send_input(&Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        });
    }
    let sent = conn
        .flush_input(&mut session)
        .unwrap_or_else(|e| panic!("live: input flush failed: {e}"));
    assert!(sent >= 3, "text + key-down + key-up must reach the wire");
    println!("live: sent {sent} fast-path input events via RdpConnection::flush_input");

    std::thread::sleep(Duration::from_millis(700));
    match settle(&mut conn, &mut session, Duration::from_secs(10)) {
        Some(after) => {
            let checksum_after = fnv1a64(&after);
            if checksum_after == baseline {
                println!(
                    "live: INPUT sent OK; framebuffer UNCHANGED after keystroke \
                     (fnv1a64={checksum_after:#018x}) — desktop may not echo"
                );
            } else {
                println!(
                    "live: INPUT ECHOED — framebuffer changed after keystroke \
                     (before={baseline:#018x} after={checksum_after:#018x})"
                );
            }
        }
        None => println!("live: INPUT sent OK; server repainted nothing afterwards"),
    }

    // ── E12-10 tier contract, exercised live ────────────────────────────────
    // Pin a lighter tier: the target moves, the session honestly demands a
    // reconnect, and reconnecting through the same public entry point applies
    // it — the next connection is negotiated at 16-bpp with bulk compression.
    let change = session
        .set_quality_mode(QualityMode::Pinned(QualityTier::Compressed), 0)
        .expect("pinning a lighter tier is a target change");
    assert_eq!(change.to, QualityTier::Compressed);
    assert!(
        session.needs_reconnect(),
        "RDP tiers are reconnect-gated (RdpTierSettings::APPLICATION)"
    );
    assert_eq!(session.connect_settings().color_depth, 16);
    conn.shutdown(&mut session)
        .unwrap_or_else(|e| panic!("live: graceful shutdown failed: {e}"));

    let mut conn2 = RdpConnection::connect(&mut session)
        .unwrap_or_else(|e| panic!("live: tier reconnect failed: {e}"));
    assert!(
        !session.needs_reconnect(),
        "reconnect applied the pinned tier"
    );
    assert_eq!(session.applied_tier(), QualityTier::Compressed);
    let renegotiated = conn2.negotiated().clone();
    println!(
        "live: RECONNECTED tier={:?} desktop={}x{} compression={:?}",
        renegotiated.tier,
        renegotiated.desktop_size.0,
        renegotiated.desktop_size.1,
        renegotiated.compression,
    );
    assert_eq!(renegotiated.tier, QualityTier::Compressed);

    let (image2, rects2) = pump_until_frame(
        &mut conn2,
        &mut session,
        Duration::from_secs(60),
        "the post-reconnect paint",
    );
    let checksum2 = fnv1a64(&image2);
    println!(
        "live: TIER FRAME OK {}x{} rects={rects2} fnv1a64={checksum2:#018x} distinct_colors={}",
        image2.size[0],
        image2.size[1],
        distinct_colors(&image2)
    );
    conn2
        .shutdown(&mut session)
        .unwrap_or_else(|e| panic!("live: final shutdown failed: {e}"));
}
