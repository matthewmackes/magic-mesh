//! E12-6 live remainder — attach the REAL crate path to a REAL guest console.
//!
//! The unit suite proves the RFB decode/input surface on synthetic bytes; this
//! test proves the same public API against a live XCP-ng guest console (the
//! "guest with no RDP renders + is interactive via VNC" acceptance). The console
//! *location* is resolved out-of-band via the existing DATACENTER `vm-console`
//! verb (`mackesd` `ipc::datacenter::vm_console_command` →
//! `xe console-list vm-uuid=<uuid> params=location --minimal` on the dom0); the
//! raw RFB socket behind that location (XCP-ng HVM consoles are qemu VNC on a
//! dom0-local socket, e.g. `unix:/var/run/xen/vnc-<domid>`) is reached over an
//! SSH tunnel so no XAPI credentials enter the test.
//!
//! Everything protocol-decode/input-encode goes through the crate's public
//! session API — [`VncSession::apply_rect`] fills the framebuffer,
//! [`VncSession::frame`] yields the egui [`ColorImage`] the shell would upload,
//! [`VncSession::send_input`] / [`VncSession::take_input`] +
//! [`RfbClientMessage::to_bytes`] produce the key bytes put on the wire. Only
//! the transport (TCP + the RFB handshake, deliberately the integration-gated
//! layer per the crate docs) lives here.
//!
//! Env-gated + `#[ignore]` — a live console cannot exist in CI. Run:
//!
//! ```text
//! MDE_VNC_LIVE_TARGET=127.0.0.1:15900 \
//!   cargo test -p mde-vdi-vnc --test live_console -- --ignored --nocapture
//! ```

#![allow(
    clippy::panic,
    reason = "test-only transport: a live-probe failure must abort with typed \
              wire-level evidence, and panicking IS the test failure mechanism"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use mde_vdi_vnc::egui::{ColorImage, Event, Key, Modifiers};
use mde_vdi_vnc::{
    parse_pixel_format, parse_rectangle_header, Encoding, PixelFormat, Reader, VncConfig,
    VncSession,
};

/// Read exactly `n` bytes, with context on failure (timeout / EOF are the
/// honest live-failure evidence, so the message says what was being read).
fn read_n(stream: &mut TcpStream, n: usize, what: &str) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    stream
        .read_exact(&mut buf)
        .unwrap_or_else(|e| panic!("live: short read of {what} ({n} bytes): {e}"));
    buf
}

/// Write all bytes, with context on failure.
fn write_all(stream: &mut TcpStream, bytes: &[u8], what: &str) {
    stream
        .write_all(bytes)
        .unwrap_or_else(|e| panic!("live: write of {what} failed: {e}"));
}

/// Big-endian `u16` from the first two bytes of `b`.
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Big-endian `u32` from the first four bytes of `b`.
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

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

/// Distinct RGBA values in the frame — a text console shows at least fg+bg, so
/// this is recorded (not hard-asserted: a blanked console is legitimately 1).
fn distinct_colors(image: &ColorImage) -> usize {
    let mut seen: std::collections::HashSet<[u8; 4]> = std::collections::HashSet::with_capacity(64);
    for px in &image.pixels {
        seen.insert(px.to_array());
    }
    seen.len()
}

/// Read the server's reason string (`u32` length + bytes) after a refusal.
fn read_reason(stream: &mut TcpStream) -> String {
    let len = be32(&read_n(stream, 4, "failure-reason length")) as usize;
    String::from_utf8_lossy(&read_n(stream, len.min(4096), "failure-reason text")).into_owned()
}

/// RFB `ProtocolVersion` + security handshake (RFC 6143 §7.1), security type
/// `None` only — XCP-ng dom0-local qemu consoles carry no RFB auth (access is
/// gated by reaching the dom0 socket). Returns the negotiated `(major, minor)`.
fn handshake(stream: &mut TcpStream) -> (u32, u32) {
    let banner = read_n(stream, 12, "ProtocolVersion banner");
    let text = String::from_utf8_lossy(&banner).into_owned();
    assert!(
        text.starts_with("RFB "),
        "live: not an RFB server (banner {text:?})"
    );
    let major: u32 = text[4..7].parse().expect("RFB major version digits");
    let minor: u32 = text[8..11].parse().expect("RFB minor version digits");
    println!("live: server banner {}", text.trim_end());

    // Reply with the highest version we speak that the server offers.
    let (major, minor) = match (major, minor) {
        (3, m) if m >= 8 => (3, 8),
        (3, 7) => (3, 7),
        _ => (3, 3),
    };
    write_all(
        stream,
        format!("RFB {major:03}.{minor:03}\n").as_bytes(),
        "ProtocolVersion reply",
    );

    if minor >= 7 {
        // 3.7/3.8: server lists security types, client picks one.
        let count = read_n(stream, 1, "security-type count")[0];
        assert!(
            count != 0,
            "live: server refused connection: {}",
            read_reason(stream)
        );
        let types = read_n(stream, usize::from(count), "security-type list");
        println!("live: security types offered: {types:?}");
        assert!(
            types.contains(&1),
            "live: security 'None' not offered (types {types:?}) — probe speaks None only"
        );
        write_all(stream, &[1], "security-type choice (None)");
        if minor >= 8 {
            let result = be32(&read_n(stream, 4, "SecurityResult"));
            assert!(
                result == 0,
                "live: security handshake failed: {}",
                read_reason(stream)
            );
        }
    } else {
        // 3.3: the server dictates the security type.
        let sec = be32(&read_n(stream, 4, "3.3 security type"));
        assert!(
            sec != 0,
            "live: server refused connection: {}",
            read_reason(stream)
        );
        assert!(
            sec == 1,
            "live: server demands security type {sec} — probe speaks None only"
        );
    }
    (major, minor)
}

/// Parsed RFB `ServerInit` (RFC 6143 §7.3.2).
struct ServerInit {
    width: u16,
    height: u16,
    format: PixelFormat,
    name: String,
}

/// Send `ClientInit` and read the `ServerInit` — the server's real framebuffer
/// geometry, wire pixel format, and desktop name.
fn client_server_init(stream: &mut TcpStream, shared: bool) -> ServerInit {
    write_all(stream, &[u8::from(shared)], "ClientInit");
    let head = read_n(stream, 24, "ServerInit");
    let mut reader = Reader::new(&head[4..20]);
    let format = parse_pixel_format(&mut reader).expect("ServerInit PIXEL_FORMAT parses");
    let name_len = be32(&head[20..24]) as usize;
    let name =
        String::from_utf8_lossy(&read_n(stream, name_len.min(4096), "desktop name")).into_owned();
    ServerInit {
        width: be16(&head[0..2]),
        height: be16(&head[2..4]),
        format,
        name,
    }
}

/// The 16-byte wire `PIXEL_FORMAT` for a `SetPixelFormat` message — the encode
/// mirror of the crate's [`parse_pixel_format`].
fn pixel_format_bytes(f: PixelFormat) -> [u8; 16] {
    let [r0, r1] = f.red_max.to_be_bytes();
    let [g0, g1] = f.green_max.to_be_bytes();
    let [b0, b1] = f.blue_max.to_be_bytes();
    [
        f.bits_per_pixel,
        f.depth,
        u8::from(f.big_endian),
        u8::from(f.true_color),
        r0,
        r1,
        g0,
        g1,
        b0,
        b1,
        f.red_shift,
        f.green_shift,
        f.blue_shift,
        0,
        0,
        0,
    ]
}

/// Ask for a full (non-incremental) framebuffer refresh of the whole desktop.
fn request_full_update(stream: &mut TcpStream, width: u16, height: u16) {
    let mut msg = vec![3u8, 0, 0, 0, 0, 0]; // type, incremental=0, x=0, y=0
    msg.extend_from_slice(&width.to_be_bytes());
    msg.extend_from_slice(&height.to_be_bytes());
    write_all(stream, &msg, "FramebufferUpdateRequest");
}

/// Read server messages until one complete `FramebufferUpdate` has been decoded
/// into the session (skipping Bell / colour-map / cut-text), returning the
/// rectangle count. Only Raw rectangles are expected — the probe advertises Raw
/// alone so every payload length is knowable up front.
fn pump_one_update(stream: &mut TcpStream, session: &mut VncSession) -> u16 {
    loop {
        let msg_type = read_n(stream, 1, "server message type")[0];
        match msg_type {
            0 => {
                let head = read_n(stream, 3, "FramebufferUpdate header");
                let nrects = be16(&head[1..3]);
                for i in 0..nrects {
                    let hdr = read_n(stream, 12, "rectangle header");
                    let mut reader = Reader::new(&hdr);
                    let rect =
                        parse_rectangle_header(&mut reader).expect("rectangle header parses");
                    assert!(
                        Encoding::from_i32(rect.encoding) == Encoding::Raw,
                        "live: rect {i} arrived as encoding {} — probe advertised Raw only",
                        rect.encoding
                    );
                    let len = usize::from(rect.width)
                        * usize::from(rect.height)
                        * session.format().bytes_per_pixel();
                    let payload = read_n(stream, len, "Raw rectangle payload");
                    session
                        .apply_rect(&rect, &payload)
                        .expect("crate decodes the live Raw rectangle");
                }
                return nrects;
            }
            1 => {
                // SetColourMapEntries: 1 pad + first-colour + n, then 6 bytes each.
                let head = read_n(stream, 5, "SetColourMapEntries header");
                let n = usize::from(be16(&head[3..5]));
                let _ = read_n(stream, n * 6, "colour-map entries");
            }
            2 => {} // Bell
            3 => {
                let head = read_n(stream, 7, "ServerCutText header");
                let len = be32(&head[3..7]) as usize;
                let _ = read_n(stream, len, "cut-text payload");
            }
            other => panic!("live: unexpected server message type {other}"),
        }
    }
}

/// The live acceptance: handshake with a real console, decode ≥1 real
/// framebuffer update through [`VncSession`] into an egui [`ColorImage`], then
/// push a key event through the same session API and note whether the guest's
/// pixels change (a login prompt echoes; no change is recorded, not failed).
#[test]
#[ignore = "live console required — set MDE_VNC_LIVE_TARGET=host:port (see module docs)"]
#[allow(
    clippy::too_many_lines,
    reason = "one linear protocol script — handshake → init → frame → input \
              round-trip reads best unbroken, mirroring the RFB message order"
)]
fn live_console_renders_and_accepts_input() {
    let Ok(target) = std::env::var("MDE_VNC_LIVE_TARGET") else {
        eprintln!("live: SKIP — MDE_VNC_LIVE_TARGET not set (host:port of a raw RFB socket)");
        return;
    };
    let (host, port_str) = target
        .rsplit_once(':')
        .expect("MDE_VNC_LIVE_TARGET must be host:port");
    let port: u16 = port_str.parse().expect("MDE_VNC_LIVE_TARGET port parses");

    let mut stream = TcpStream::connect((host, port))
        .unwrap_or_else(|e| panic!("live: cannot connect to {target}: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout set");
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .expect("write timeout set");

    // ── Handshake + ServerInit ──────────────────────────────────────────────
    let (major, minor) = handshake(&mut stream);
    println!("live: negotiated RFB {major}.{minor}, security None");

    // Build the session through the crate's public config path FIRST so the
    // ClientInit shared-flag comes from the same VncConfig the shell would use.
    let init = {
        let probe_cfg = VncConfig::new(host).with_port(port).shared(true);
        client_server_init(&mut stream, probe_cfg.shared)
    };
    println!(
        "live: ServerInit {}x{} name={:?} format={:?}",
        init.width, init.height, init.name, init.format
    );

    let config = VncConfig::new(host)
        .with_port(port)
        .with_size(init.width, init.height)
        .shared(true);
    let mut session = VncSession::new(config).expect("live ServerInit geometry is a valid config");

    if init.format.is_supported() {
        session.set_format(init.format);
        println!("live: using the server's native pixel format");
    } else {
        // Palette or otherwise undecodable — ask for the crate's canonical
        // 32-bpp true-colour layout instead (SetPixelFormat, RFC 6143 §7.5.1).
        let fallback = PixelFormat::rgba8888();
        let mut msg = vec![0u8, 0, 0, 0];
        msg.extend_from_slice(&pixel_format_bytes(fallback));
        write_all(&mut stream, &msg, "SetPixelFormat");
        session.set_format(fallback);
        println!("live: server format unsupported — switched to rgba8888 via SetPixelFormat");
    }

    // Advertise Raw only (SetEncodings): every rectangle then has a computable
    // length, and Raw exercises the crate's baseline decode on real bytes.
    let mut set_enc = vec![2u8, 0, 0, 1];
    set_enc.extend_from_slice(&Encoding::Raw.code().to_be_bytes());
    write_all(&mut stream, &set_enc, "SetEncodings [Raw]");

    // ── One real framebuffer through the crate into an egui texture image ──
    request_full_update(&mut stream, init.width, init.height);
    let rects = pump_one_update(&mut stream, &mut session);
    assert!(rects >= 1, "live: first FramebufferUpdate carried no rects");

    let image = session
        .frame()
        .expect("session yields a frame after the live update");
    assert_eq!(
        image.size,
        [usize::from(init.width), usize::from(init.height)],
        "frame geometry must match the live ServerInit"
    );
    assert!(!image.pixels.is_empty(), "live frame decoded no pixels");
    let checksum = fnv1a64(&image);
    let colors = distinct_colors(&image);
    println!(
        "live: FRAME OK {}x{} rects={rects} fnv1a64={checksum:#018x} distinct_colors={colors}",
        image.size[0], image.size[1]
    );

    // ── Input round-trip (best effort, recorded honestly) ───────────────────
    // Type "m" + Enter through the same session API the shell drives; a getty
    // echoes the character / reprints its prompt, so pixels should move.
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
    let queued = session.take_input();
    assert!(
        !queued.is_empty(),
        "session queued no wire messages for the key events"
    );
    let mut wire = Vec::new();
    for msg in &queued {
        msg.encode(&mut wire);
    }
    write_all(&mut stream, &wire, "queued KeyEvent messages");
    println!(
        "live: sent {} input messages ({} wire bytes) via VncSession::take_input",
        queued.len(),
        wire.len()
    );

    std::thread::sleep(Duration::from_millis(700));
    request_full_update(&mut stream, init.width, init.height);
    let rects_after = pump_one_update(&mut stream, &mut session);
    if let Some(after) = session.frame() {
        let checksum_after = fnv1a64(&after);
        if checksum_after == checksum {
            println!(
                "live: INPUT sent OK; framebuffer UNCHANGED after keypress \
                 (fnv1a64={checksum_after:#018x}, rects={rects_after}) — console may not echo"
            );
        } else {
            println!(
                "live: INPUT ECHOED — framebuffer changed after keypress \
                 (before={checksum:#018x} after={checksum_after:#018x}, rects={rects_after})"
            );
        }
    } else {
        println!("live: INPUT sent OK; server returned no changed frame (rects={rects_after})");
    }
}
