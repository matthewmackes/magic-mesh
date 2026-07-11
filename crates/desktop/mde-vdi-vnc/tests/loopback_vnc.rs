//! Headless, CI-runnable proof of the RFB protocol surface over a **loopback**
//! socket — no live VNC server required.
//!
//! `tests/live_console.rs` proves the same public [`VncSession`] API against a
//! real XCP-ng guest console, but it is env-gated (`MDE_VNC_LIVE_TARGET`) and
//! `#[ignore]`d, so on the airgapped farm the crate would otherwise get zero
//! *integration*-level protocol coverage. This file closes that gap the way
//! `mde-vdi-spice/tests/loopback_spice.rs` does for SPICE: an in-process server
//! thread speaks the protocol over `127.0.0.1:0`, and the client half drives the
//! crate's real public pieces end-to-end across the wire. Every test here runs
//! by default — no env var, no `--features`, no external target.
//!
//! Unlike SPICE (whose wire decode lives in the `spice-client` dependency, so its
//! loopback proof can only drive the *connect* seam against a closed port), VNC's
//! RFB decoder is pure-Rust and in-crate, so the loopback server can play a real
//! RFB script and the client can prove the whole chain on canned bytes:
//!
//! * `vnc_auth_challenge_response_is_accepted_by_the_server` — the type-2
//!   "VNC Authentication" DES challenge/response ([`vnc_auth_response`]) computed
//!   by the client is byte-for-byte what a server keyed with the same password
//!   accepts. This is the wire step the feature-gated transport performs; here it
//!   crosses a real socket and a server verifies it.
//! * `handshake_auth_and_first_framebuffer_render` — the full opening: RFB
//!   `ProtocolVersion` → security type 2 (DES auth) → `ClientInit`/`ServerInit`
//!   (pixel-format parse via [`parse_pixel_format`]) → a Raw `FramebufferUpdate`
//!   decoded through [`VncSession::apply_rect`] into an egui [`ColorImage`], with
//!   the decoded pixels asserted.
//! * `session_input_reaches_the_server_as_rfb_wire_bytes` — egui input driven
//!   through [`VncSession::send_input`] / [`VncSession::take_input`] and encoded
//!   ([`RfbClientMessage::encode`]) arrives at the server as the exact RFB
//!   `PointerEvent` / `KeyEvent` bytes.
//!
//! **Documented gap:** the RFB handshake *state machine* and the TCP read/write
//! pump themselves live in the feature-gated `connect.rs` (`live-connect`); the
//! server-side script here is written in the test (exactly as `live_console.rs`
//! writes the client-side script), so this proves the crate's public decode /
//! auth / input surface composes over a socket, not the private transport loop.
//! That loop still needs the env-gated live proof.

#![allow(
    clippy::panic,
    clippy::too_many_lines,
    reason = "test-only loopback server + client: a wiring failure must abort with \
              typed wire-level evidence, and panicking IS the test failure mechanism; \
              each RFB script reads best as one linear sequence"
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use mde_vdi_vnc::egui::{Color32, Event, Key, Modifiers, Pos2};
use mde_vdi_vnc::{
    parse_pixel_format, parse_rectangle_header, vnc_auth_response, Encoding, PixelFormat, Reader,
    RfbClientMessage, VncConfig, VncSession,
};

/// A generous-but-finite socket timeout: a real deadlock in the loopback script
/// surfaces as a typed error here rather than hanging CI (the SPICE loopback's
/// "never block" ethos), while a slow farm node still has ample slack.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The canonical 32-bpp little-endian true-colour `PIXEL_FORMAT` wire bytes —
/// the layout a `ServerInit` carries; parses to [`PixelFormat::rgba8888`].
const RGBA8888_PIXEL_FORMAT: [u8; 16] = [
    32, 24, 0, 1, // bpp, depth, big-endian=0, true-colour=1
    0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, // r/g/b max = 255 (BE u16)
    16, 8, 0, // r/g/b shift
    0, 0, 0, // padding
];

/// RFB "VNC Authentication" (type 2) challenge used by the loopback server.
const AUTH_CHALLENGE: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

/// The response a server keyed with password `"pass"` expects for
/// [`AUTH_CHALLENGE`] — cross-checked against OpenSSL `des-ecb` with the RFB
/// bit-reversed key (see `src/des.rs` unit vectors). The client must reproduce
/// this from the crate's public [`vnc_auth_response`] alone.
const AUTH_RESPONSE_FOR_PASS: [u8; 16] = [
    0x5f, 0xb0, 0x2f, 0x4e, 0x6e, 0xc9, 0xfd, 0xa0, 0x6c, 0x41, 0xdf, 0x1f, 0x35, 0x01, 0x51, 0x38,
];

/// rgba8888 little-endian pixel bytes `[B, G, R, pad]` for a primary colour — the
/// on-wire Raw layout the crate's decoder expects.
const fn wire_px(r: u8, g: u8, b: u8) -> [u8; 4] {
    [b, g, r, 0]
}

/// Read exactly `n` bytes or panic with context (timeout / EOF is the honest
/// failure evidence).
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

// ── Server-side RFB script fragments (test scaffolding, mirroring the way
//    live_console.rs writes the *client*-side script) ─────────────────────────

/// Server: RFB 3.8 `ProtocolVersion` exchange. Sends the banner, reads the
/// client's chosen version.
fn server_protocol_version(stream: &mut TcpStream) {
    write_all(stream, b"RFB 003.008\n", "ProtocolVersion banner");
    let reply = read_n(stream, 12, "client ProtocolVersion reply");
    assert!(
        reply.starts_with(b"RFB "),
        "loopback: client version reply not RFB: {reply:?}"
    );
}

/// Server: offer security type 2 (VNC auth), run the DES challenge/response, and
/// verify the client's response is what password `"pass"` produces.
fn server_vnc_auth(stream: &mut TcpStream) {
    write_all(stream, &[1, 2], "security-type list [VncAuth]");
    let choice = read_n(stream, 1, "client security choice");
    assert_eq!(
        choice,
        [2],
        "loopback: client must select VNC auth (type 2)"
    );
    write_all(stream, &AUTH_CHALLENGE, "auth challenge");
    let response = read_n(stream, 16, "auth response");
    assert_eq!(
        response, AUTH_RESPONSE_FOR_PASS,
        "loopback: the crate's vnc_auth_response is not the server-acceptable value"
    );
    write_all(stream, &[0, 0, 0, 0], "SecurityResult OK");
}

/// Server: read `ClientInit`, then send a `ServerInit` describing a 16×16
/// rgba8888 desktop named "loopback".
fn server_client_and_server_init(stream: &mut TcpStream) {
    let _shared = read_n(stream, 1, "ClientInit shared flag");
    let name = b"loopback";
    let mut init = Vec::new();
    init.extend_from_slice(&16u16.to_be_bytes()); // width
    init.extend_from_slice(&16u16.to_be_bytes()); // height
    init.extend_from_slice(&RGBA8888_PIXEL_FORMAT); // PIXEL_FORMAT
    init.extend_from_slice(
        &u32::try_from(name.len())
            .expect("name len fits u32")
            .to_be_bytes(),
    );
    init.extend_from_slice(name);
    write_all(stream, &init, "ServerInit");
}

/// Server: send one Raw `FramebufferUpdate` painting a 2×1 red|blue span at the
/// origin (message type 0 + padding + 1 rect + the two Raw pixels).
fn server_raw_framebuffer_update(stream: &mut TcpStream) {
    let mut msg = vec![0u8]; // message-type: FramebufferUpdate
    msg.push(0); // padding
    msg.extend_from_slice(&1u16.to_be_bytes()); // one rectangle
    msg.extend_from_slice(&0u16.to_be_bytes()); // x
    msg.extend_from_slice(&0u16.to_be_bytes()); // y
    msg.extend_from_slice(&2u16.to_be_bytes()); // w
    msg.extend_from_slice(&1u16.to_be_bytes()); // h
    msg.extend_from_slice(&Encoding::Raw.code().to_be_bytes()); // encoding
    msg.extend_from_slice(&wire_px(0xFF, 0, 0)); // red
    msg.extend_from_slice(&wire_px(0, 0, 0xFF)); // blue
    write_all(stream, &msg, "Raw FramebufferUpdate");
}

/// Client: read + decode one Raw `FramebufferUpdate` into `session`, returning
/// the rectangle count (mirrors `live_console.rs`'s `pump_one_update`, Raw-only).
fn client_pump_one_update(stream: &mut TcpStream, session: &mut VncSession) -> u16 {
    let msg_type = read_n(stream, 1, "server message type")[0];
    assert_eq!(msg_type, 0, "loopback: expected a FramebufferUpdate");
    let head = read_n(stream, 3, "FramebufferUpdate header"); // padding + count
    let nrects = u16::from_be_bytes([head[1], head[2]]);
    for _ in 0..nrects {
        let hdr = read_n(stream, 12, "rectangle header");
        let mut reader = Reader::new(&hdr);
        let rect = parse_rectangle_header(&mut reader).expect("rectangle header parses");
        assert_eq!(
            Encoding::from_i32(rect.encoding),
            Encoding::Raw,
            "loopback: server sent a non-Raw rectangle"
        );
        let len =
            usize::from(rect.width) * usize::from(rect.height) * session.format().bytes_per_pixel();
        let payload = read_n(stream, len, "Raw rectangle payload");
        session
            .apply_rect(&rect, &payload)
            .expect("crate decodes the Raw rectangle streamed over loopback");
    }
    nrects
}

#[test]
fn vnc_auth_challenge_response_is_accepted_by_the_server() {
    let (listener, addr) = bound_listener();
    // Server: send the challenge, read the response, accept iff it matches the
    // value a server keyed with "pass" would compute.
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        write_all(&mut stream, &AUTH_CHALLENGE, "auth challenge");
        let response = read_n(&mut stream, 16, "auth response");
        let accepted = response == AUTH_RESPONSE_FOR_PASS;
        write_all(&mut stream, &[u8::from(accepted)], "auth verdict");
        accepted
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);
    let challenge_bytes = read_n(&mut client, 16, "challenge");
    let mut challenge = [0u8; 16];
    challenge.copy_from_slice(&challenge_bytes);
    // The crate's public auth path is the ONLY thing computing the response.
    let response = vnc_auth_response(b"pass", &challenge);
    write_all(&mut client, &response, "auth response");
    let verdict = read_n(&mut client, 1, "verdict");
    assert_eq!(verdict, [1], "loopback: server rejected the auth response");

    assert!(
        server.join().expect("server thread"),
        "loopback: server did not accept the crate-computed response"
    );
}

#[test]
fn handshake_auth_and_first_framebuffer_render() {
    let (listener, addr) = bound_listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        server_protocol_version(&mut stream);
        server_vnc_auth(&mut stream);
        server_client_and_server_init(&mut stream);
        server_raw_framebuffer_update(&mut stream);
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);

    // ── ProtocolVersion ──────────────────────────────────────────────────────
    let banner = read_n(&mut client, 12, "ProtocolVersion banner");
    assert!(banner.starts_with(b"RFB "), "not an RFB banner: {banner:?}");
    write_all(&mut client, b"RFB 003.008\n", "ProtocolVersion reply");

    // ── Security type 2 (VNC auth) via the crate's DES ───────────────────────
    let count = read_n(&mut client, 1, "security-type count")[0];
    let types = read_n(&mut client, usize::from(count), "security-type list");
    assert!(
        types.contains(&2),
        "server did not offer VNC auth: {types:?}"
    );
    write_all(&mut client, &[2], "security choice (VncAuth)");
    let challenge_bytes = read_n(&mut client, 16, "auth challenge");
    let mut challenge = [0u8; 16];
    challenge.copy_from_slice(&challenge_bytes);
    write_all(
        &mut client,
        &vnc_auth_response(b"pass", &challenge),
        "auth response",
    );
    let sec_result = read_n(&mut client, 4, "SecurityResult");
    assert_eq!(sec_result, [0, 0, 0, 0], "auth was refused");

    // ── ClientInit → ServerInit (parse the pixel format via the crate) ───────
    write_all(&mut client, &[1], "ClientInit (shared)");
    let head = read_n(&mut client, 24, "ServerInit head");
    let width = u16::from_be_bytes([head[0], head[1]]);
    let height = u16::from_be_bytes([head[2], head[3]]);
    let mut fmt_reader = Reader::new(&head[4..20]);
    let format = parse_pixel_format(&mut fmt_reader).expect("ServerInit PIXEL_FORMAT parses");
    assert_eq!(format, PixelFormat::rgba8888(), "unexpected server format");
    assert!(format.is_supported(), "server format must be decodable");
    let name_len = u32::from_be_bytes([head[20], head[21], head[22], head[23]]) as usize;
    let _name = read_n(&mut client, name_len, "desktop name");

    // Build the session through the public config path from the ServerInit
    // geometry — exactly what the shell (and live_console.rs) does.
    let config = VncConfig::new("127.0.0.1")
        .with_port(addr.port())
        .with_size(width, height)
        .shared(true);
    let mut session = VncSession::new(config).expect("ServerInit geometry is a valid config");
    session.set_format(format);

    // ── One real framebuffer, decoded across the socket into egui pixels ─────
    let rects = client_pump_one_update(&mut client, &mut session);
    assert_eq!(rects, 1, "expected exactly one rectangle");
    let image = session
        .frame()
        .expect("a frame is available after the update");
    assert_eq!(image.size, [16, 16], "frame geometry matches ServerInit");
    assert_eq!(
        image.pixels[0],
        Color32::from_rgb(0xFF, 0, 0),
        "first decoded pixel is red"
    );
    assert_eq!(
        image.pixels[1],
        Color32::from_rgb(0, 0, 0xFF),
        "second decoded pixel is blue"
    );

    server.join().expect("server thread completed");
}

#[test]
fn session_input_reaches_the_server_as_rfb_wire_bytes() {
    let (listener, addr) = bound_listener();
    // Server: read the exact number of input bytes the client will send and echo
    // them back for the client to assert (so both ends agree on the framing).
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server accept");
        arm_timeouts(&stream);
        // PointerEvent (6 bytes) + KeyEvent (8 bytes).
        let received = read_n(&mut stream, 6 + 8, "queued input messages");
        write_all(&mut stream, &received, "echo input");
        received
    });

    let mut client = TcpStream::connect(addr).expect("client connect");
    arm_timeouts(&client);

    // Drive egui input through the public session API and encode the resolved
    // RFB messages exactly as the transport would.
    let mut session =
        VncSession::new(VncConfig::new("127.0.0.1").with_size(16, 16)).expect("valid config");
    session.send_input(&Event::PointerMoved(Pos2::new(7.0, 9.0)));
    session.send_input(&Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    let queued = session.take_input();
    assert_eq!(
        queued,
        vec![
            RfbClientMessage::PointerEvent {
                button_mask: 0,
                x: 7,
                y: 9,
            },
            RfbClientMessage::KeyEvent {
                down: true,
                keysym: 0x61, // 'a'
            },
        ],
        "session resolved the wrong input messages"
    );
    let mut wire = Vec::new();
    for msg in &queued {
        msg.encode(&mut wire);
    }
    write_all(&mut client, &wire, "queued input messages");

    let echoed = read_n(&mut client, wire.len(), "echoed input");
    assert_eq!(echoed, wire, "server saw different bytes than sent");
    // The exact RFB wire layout the server received.
    assert_eq!(
        wire,
        vec![
            5, 0, 0, 7, 0, 9, // PointerEvent: type 5, mask 0, x=7, y=9
            4, 1, 0, 0, 0x00, 0x00, 0x00, 0x61, // KeyEvent: type 4, down, keysym 'a'
        ],
        "unexpected RFB input wire bytes"
    );

    let server_saw = server.join().expect("server thread");
    assert_eq!(server_saw, wire, "server-side capture disagrees");
}
