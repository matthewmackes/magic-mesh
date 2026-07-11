//! Headless, CI-runnable proof of the RDP egui-facing surface over a
//! **loopback** socket — no live RDP server required.
//!
//! `tests/live_rdp.rs` proves the assembled stack against a real xrdp endpoint,
//! but it is `#![cfg(feature = "live-connect")]` + env-gated
//! (`MDE_RDP_LIVE_TARGET`) + `#[ignore]`d, so on the airgapped farm the crate
//! would otherwise get zero *integration*-level coverage. This file closes that
//! gap the way `mde-vdi-spice/tests/loopback_spice.rs` does for SPICE: an
//! in-process server thread streams over `127.0.0.1:0` and the client half drives
//! the crate's real public [`RdpSession`] surface end-to-end across the wire.
//! Every test here runs by default — no env var, no `--features`, no external
//! target.
//!
//! **What is and isn't the RDP wire here.** Unlike VNC (a pure-Rust in-crate
//! decoder), RDP delegates the *wire* to `ironrdp`, and the whole connection
//! sequence + PDU codec lives behind the `live-connect` feature. So the default
//! build exposes only the two `ironrdp`-free protocol halves this file exercises:
//!
//! * the **decode surface** — [`RdpSession::apply_rect`] / `apply_full_frame`,
//!   which take the raw BGRA/BGRX pixel bytes `ironrdp`'s `DecodedImage` yields
//!   and blit them into the egui [`ColorImage`] the shell uploads. The loopback
//!   server streams exactly those decoded bytes over a socket (the direct analog
//!   of SPICE's "a decoded surface arrives → a frame is available" loopback
//!   proof), so the decode composes end-to-end across a real connection with
//!   partial reads, not just against an in-memory slice.
//! * the **input mapping** — [`RdpSession::send_input`] resolving [`egui::Event`]s
//!   into protocol-neutral [`RdpInputEvent`] intents. The test round-trips those
//!   resolved intents over the socket with a small, explicitly test-local framing.
//!
//! **Documented gap:** the actual RDP PDU byte encoding (the `ironrdp` input PDU
//! and the bitmap/codec *decode*) is the feature-gated live layer — this file
//! proves the crate's public decode/input surface composes over a socket, not the
//! `ironrdp` wire itself, which still needs the env-gated `live_rdp.rs` proof.

#![allow(
    clippy::panic,
    reason = "test-only loopback server + client: a wiring failure must abort with \
              typed evidence, and panicking IS the test failure mechanism"
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use mde_vdi_rdp::egui::{Color32, Event, Key, Modifiers, Pos2};
use mde_vdi_rdp::{PixelFormat, RdpConfig, RdpInputEvent, RdpSession, Scancode};

/// A generous-but-finite socket timeout so a wiring deadlock surfaces as a typed
/// error instead of hanging CI (the SPICE loopback's "never block" ethos).
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The smallest RDP-legal desktop (`validate()` enforces a 200px minimum and an
/// even width); tests paint a tiny span at the origin and assert on it.
const DESKTOP: u16 = 200;

/// Read exactly `n` bytes or panic with context.
fn read_n(stream: &mut TcpStream, n: usize, what: &str) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    stream
        .read_exact(&mut buf)
        .unwrap_or_else(|e| panic!("loopback: short read of {what} ({n} bytes): {e}"));
    buf
}

/// Write all bytes or panic with context.
fn write_all(stream: &mut TcpStream, bytes: &[u8], what: &str) {
    stream
        .write_all(bytes)
        .unwrap_or_else(|e| panic!("loopback: write of {what} failed: {e}"));
}

/// Apply the read/write timeouts both halves use.
fn arm_timeouts(stream: &TcpStream) {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .expect("set write timeout");
}

/// Bind a loopback listener and hand its address to the caller alongside it.
fn bound_listener() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("listener addr");
    (listener, addr)
}

/// A fresh session at the minimum RDP desktop size.
fn session() -> RdpSession {
    RdpSession::new(RdpConfig::new("127.0.0.1", "u", "p").with_resolution(DESKTOP, DESKTOP))
        .expect("valid config")
}

/// A test-local wire framing for a decoded rectangle the loopback server streams:
/// `[x u16, y u16, w u16, h u16]` followed by `w*h*4` BGRA bytes. (This framing is
/// test scaffolding standing in for the `ironrdp` bitmap PDU, which is the
/// feature-gated live layer; the *pixel bytes* are the real decoded surface.)
fn rect_message(x: u16, y: u16, w: u16, h: u16, bgra: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(&x.to_be_bytes());
    msg.extend_from_slice(&y.to_be_bytes());
    msg.extend_from_slice(&w.to_be_bytes());
    msg.extend_from_slice(&h.to_be_bytes());
    msg.extend_from_slice(bgra);
    msg
}

/// A minimal, explicitly test-local encoding of an [`RdpInputEvent`] so the
/// resolved intents can be proven to survive a socket round-trip. NOT the RDP
/// input PDU (that is `ironrdp`, feature-gated) — just enough to move the intent
/// across the wire intact.
fn encode_intent(ev: RdpInputEvent) -> Vec<u8> {
    match ev {
        RdpInputEvent::PointerMove { x, y } => {
            let mut b = vec![1u8];
            b.extend_from_slice(&x.to_be_bytes());
            b.extend_from_slice(&y.to_be_bytes());
            b
        }
        RdpInputEvent::Key { scancode, down } => {
            vec![
                4u8,
                scancode.code,
                u8::from(scancode.extended),
                u8::from(down),
            ]
        }
        // The two variants above are the ones these tests drive; the rest are not
        // exercised here (they are covered by the src/ input unit tests).
        other => panic!("loopback: intent {other:?} has no test framing"),
    }
}

#[test]
fn decoded_rectangle_streams_into_an_egui_frame() {
    let (listener, addr) = bound_listener();
    // Server: stream one decoded 2×1 red|blue rectangle at the origin (BGRA).
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        let bgra = [
            0x00, 0x00, 0xFF, 0xFF, // BGRA red
            0xFF, 0x00, 0x00, 0xFF, // BGRA blue
        ];
        write_all(&mut stream, &rect_message(0, 0, 2, 1, &bgra), "rect update");
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);

    let head = read_n(&mut client, 8, "rect header");
    let w = u16::from_be_bytes([head[4], head[5]]);
    let h = u16::from_be_bytes([head[6], head[7]]);
    let x = u16::from_be_bytes([head[0], head[1]]);
    let y = u16::from_be_bytes([head[2], head[3]]);
    let stride = usize::from(w) * PixelFormat::BYTES_PER_PIXEL;
    let payload = read_n(&mut client, usize::from(h) * stride, "rect pixels");

    let mut rdp = session();
    let _ = rdp.frame(); // consume the initial black frame
    rdp.apply_rect(
        usize::from(x),
        usize::from(y),
        usize::from(w),
        usize::from(h),
        PixelFormat::Bgra,
        &payload,
        stride,
    )
    .expect("crate decodes the rectangle streamed over loopback");

    let image = rdp.frame().expect("a frame is available after the update");
    assert_eq!(image.size, [usize::from(DESKTOP); 2], "geometry unchanged");
    assert_eq!(
        image.pixels[0],
        Color32::from_rgb(0xFF, 0, 0),
        "pixel 0 red"
    );
    assert_eq!(
        image.pixels[1],
        Color32::from_rgb(0, 0, 0xFF),
        "pixel 1 blue"
    );
    assert_eq!(
        image.pixels[2],
        Color32::from_rgb(0, 0, 0),
        "untouched pixel stays black"
    );

    server.join().expect("server thread completed");
}

#[test]
fn full_desktop_frame_streams_over_the_socket() {
    let (listener, addr) = bound_listener();
    let pixels = usize::from(DESKTOP) * usize::from(DESKTOP);
    let frame_bytes = pixels * PixelFormat::BYTES_PER_PIXEL;

    // Server: stream a whole-desktop BGRX frame — opaque black except pixel 0
    // (red) and pixel 1 (blue). BGRX exercises the X-format opaque-alpha
    // normalisation on socket-streamed bytes.
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        let mut buf = vec![0u8; frame_bytes]; // all zero → black, padding alpha
        buf[0..4].copy_from_slice(&[0x00, 0x00, 0xFF, 0x00]); // BGRX red
        buf[4..8].copy_from_slice(&[0xFF, 0x00, 0x00, 0x00]); // BGRX blue
        write_all(&mut stream, &buf, "full frame");
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);
    let src = read_n(&mut client, frame_bytes, "full frame");

    let mut s = session();
    let _ = s.frame();
    s.apply_full_frame(PixelFormat::Bgrx, &src)
        .expect("crate decodes the full frame streamed over loopback");

    let image = s.frame().expect("a frame after the full update");
    assert_eq!(
        image.size,
        [usize::from(DESKTOP); 2],
        "geometry matches desktop"
    );
    assert_eq!(
        image.pixels[0],
        Color32::from_rgb(0xFF, 0, 0),
        "pixel 0 red"
    );
    assert_eq!(
        image.pixels[1],
        Color32::from_rgb(0, 0, 0xFF),
        "pixel 1 blue"
    );
    // A BGRX padding byte of 0 must normalise to opaque black, not transparent.
    let mid = image.pixels[pixels / 2];
    assert_eq!(mid, Color32::from_rgb(0, 0, 0), "interior stays black");
    assert_eq!(mid.a(), 0xFF, "BGRX forces opaque alpha across the wire");

    server.join().expect("server thread completed");
}

#[test]
fn session_input_intents_survive_a_socket_round_trip() {
    let (listener, addr) = bound_listener();
    // Server: echo the exact intent bytes back so both ends agree on the framing.
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        // PointerMove (5 bytes) + Key (4 bytes) in the test framing.
        let received = read_n(&mut stream, 5 + 4, "queued intents");
        write_all(&mut stream, &received, "echo intents");
        received
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);

    // Drive egui input through the public session API.
    let mut s = session();
    s.send_input(&Event::PointerMoved(Pos2::new(7.0, 9.0)));
    s.send_input(&Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    let intents = s.take_input();
    assert_eq!(
        intents,
        vec![
            RdpInputEvent::PointerMove { x: 7, y: 9 },
            RdpInputEvent::Key {
                scancode: Scancode {
                    code: 0x1E, // set-1 'A'
                    extended: false,
                },
                down: true,
            },
        ],
        "session resolved the wrong input intents"
    );

    let mut wire = Vec::new();
    for ev in &intents {
        wire.extend_from_slice(&encode_intent(*ev));
    }
    assert_eq!(
        wire,
        vec![1, 0, 7, 0, 9, 4, 0x1E, 0, 1],
        "unexpected test-framed intent bytes"
    );
    write_all(&mut client, &wire, "queued intents");

    let echoed = read_n(&mut client, wire.len(), "echoed intents");
    assert_eq!(
        echoed, wire,
        "intents did not survive the socket round-trip"
    );
    let server_saw = server.join().expect("server thread");
    assert_eq!(server_saw, wire, "server-side capture disagrees");
}
