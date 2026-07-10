//! The live VNC/RFB connect layer (`live-connect` feature) — E12-6.
//!
//! [`VncConnection::connect`] runs the real RFB opening sequence over TCP:
//! `ProtocolVersion`, security negotiation, `ClientInit`, and `ServerInit`.
//! [`VncConnection::pump_once`] then requests one framebuffer update, decodes the
//! server's rectangles through [`VncSession`], and lets the shell upload the
//! resulting `ColorImage`. Input is flushed from the same session queue the
//! unit-tested egui input mapper fills.
//!
//! The transport supports the encodings this crate decodes (`Raw`, `CopyRect`,
//! `RRE`, `Hextile`) and keeps authentication honest: RFB security type `None` works
//! today, which is the XCP-ng console path gated by mesh/dom0 reachability. VNC
//! password auth is reported as an unsupported security mode until a DES
//! challenge implementation lands.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::config::{ConfigError, VncConfig};
use crate::encoding::{parse_pixel_format, parse_rectangle_header, DecodeError, Encoding, Reader};
use crate::session::VncSession;
use crate::wire::{RfbClientMessage, RfbControlMessage};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_PUMP_TIMEOUT: Duration = Duration::from_millis(50);

/// Why the live VNC connection failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The [`VncConfig`] is invalid.
    Config(ConfigError),
    /// DNS/address resolution produced no socket address.
    Resolve {
        /// Hostname or IP address that produced no socket addresses.
        host: String,
        /// TCP port requested for the RFB endpoint.
        port: u16,
    },
    /// TCP connect/read/write failed.
    Io {
        /// The connection phase that failed.
        phase: &'static str,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// The peer did not speak a supported RFB version.
    Protocol(String),
    /// The server requires a security type this transport does not implement.
    UnsupportedSecurity {
        /// RFB security type IDs offered by the server.
        offered: Vec<u8>,
        /// Whether the caller supplied a password that would require VNC auth.
        password_supplied: bool,
    },
    /// Security negotiation failed with a server-supplied reason.
    Security(String),
    /// A framebuffer rectangle could not be decoded.
    Decode(DecodeError),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "VNC config rejected: {e}"),
            Self::Resolve { host, port } => write!(f, "no address resolved for {host}:{port}"),
            Self::Io { phase, source } => write!(f, "VNC {phase} failed: {source}"),
            Self::Protocol(reason) => write!(f, "RFB protocol error: {reason}"),
            Self::UnsupportedSecurity {
                offered,
                password_supplied,
            } => {
                let hint = if *password_supplied {
                    "VNC password auth is not implemented yet"
                } else {
                    "server did not offer security type None"
                };
                write!(f, "unsupported RFB security types {offered:?}: {hint}")
            }
            Self::Security(reason) => write!(f, "RFB security failed: {reason}"),
            Self::Decode(e) => write!(f, "RFB framebuffer decode failed: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io { source, .. } => Some(source),
            Self::Decode(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ConfigError> for ConnectError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<DecodeError> for ConnectError {
    fn from(value: DecodeError) -> Self {
        Self::Decode(value)
    }
}

/// What one [`VncConnection::pump_once`] call observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PumpOutcome {
    /// A framebuffer update landed and was decoded.
    Processed {
        /// Number of rectangles in the update.
        rects: u16,
        /// Bytes read for rectangle payloads.
        payload_bytes: usize,
    },
    /// No server message arrived before the requested timeout.
    TimedOut,
    /// The server closed the connection.
    Terminated {
        /// Human-readable reason the pump stopped.
        reason: String,
    },
}

/// The live RFB properties negotiated during connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// RFB major version.
    pub major: u32,
    /// RFB minor version.
    pub minor: u32,
    /// Server framebuffer width.
    pub width: u16,
    /// Server framebuffer height.
    pub height: u16,
    /// Server desktop name.
    pub name: String,
}

/// A live VNC/RFB connection that feeds a [`VncSession`].
pub struct VncConnection {
    stream: TcpStream,
    negotiated: Negotiated,
    first_update: bool,
    connected_at: Instant,
    last_request: Option<Instant>,
}

impl VncConnection {
    /// Open a live RFB connection and size/configure `session` from `ServerInit`.
    ///
    /// # Errors
    /// [`ConnectError`] if the config is invalid, the endpoint cannot be reached,
    /// the peer is not RFB, or security negotiation cannot complete.
    pub fn connect(session: &mut VncSession) -> Result<Self, ConnectError> {
        let config = session.config().clone();
        config.validate()?;
        let mut stream = connect_tcp(&config)?;
        stream
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .map_err(|source| ConnectError::Io {
                phase: "set read timeout",
                source,
            })?;
        stream
            .set_write_timeout(Some(CONNECT_TIMEOUT))
            .map_err(|source| ConnectError::Io {
                phase: "set write timeout",
                source,
            })?;

        let (major, minor) = handshake(&mut stream, config.password.is_some())?;
        let init = client_server_init(&mut stream, config.shared)?;
        session.resize(init.width, init.height);
        if init.format.is_supported() {
            session.set_format(init.format);
        }

        let negotiated = Negotiated {
            major,
            minor,
            width: init.width,
            height: init.height,
            name: init.name,
        };
        let mut conn = Self {
            stream,
            negotiated,
            first_update: true,
            connected_at: Instant::now(),
            last_request: None,
        };
        conn.flush_control(session)?;
        Ok(conn)
    }

    /// The negotiated server properties.
    #[must_use]
    pub const fn negotiated(&self) -> &Negotiated {
        &self.negotiated
    }

    /// Pump one framebuffer update.
    ///
    /// The first call asks for a full update; later calls ask for incremental
    /// updates and respect the session's adaptive update pacing.
    ///
    /// # Errors
    /// [`ConnectError`] for transport or decode failures.
    pub fn pump_once(
        &mut self,
        session: &mut VncSession,
        timeout: Duration,
    ) -> Result<PumpOutcome, ConnectError> {
        self.flush_control(session)?;
        self.request_update(session)?;
        self.stream
            .set_read_timeout(Some(timeout.max(DEFAULT_PUMP_TIMEOUT)))
            .map_err(|source| ConnectError::Io {
                phase: "set pump timeout",
                source,
            })?;

        loop {
            let mut kind = [0u8; 1];
            match self.stream.read_exact(&mut kind) {
                Ok(()) => {}
                Err(e) if is_timeout(&e) => {
                    session.record_stall(self.elapsed_ms());
                    return Ok(PumpOutcome::TimedOut);
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(PumpOutcome::Terminated {
                        reason: "server closed the RFB socket".to_string(),
                    });
                }
                Err(source) => {
                    return Err(ConnectError::Io {
                        phase: "read server message",
                        source,
                    });
                }
            }

            match kind[0] {
                0 => {
                    let (rects, payload_bytes) = self.read_framebuffer_update(session)?;
                    self.first_update = false;
                    session.record_frame(self.elapsed_ms(), payload_bytes);
                    let _ = session.autotune(self.elapsed_ms());
                    return Ok(PumpOutcome::Processed {
                        rects,
                        payload_bytes,
                    });
                }
                1 => self.skip_colour_map()?,
                2 => {}
                3 => self.skip_cut_text()?,
                other => {
                    return Err(ConnectError::Protocol(format!(
                        "unexpected server message type {other}"
                    )));
                }
            }
        }
    }

    /// Flush egui input events queued in `session` to the server.
    ///
    /// # Errors
    /// [`ConnectError`] if the socket write fails.
    pub fn flush_input(&mut self, session: &mut VncSession) -> Result<usize, ConnectError> {
        let queued = session.take_input();
        let count = queued.len();
        write_client_messages(&mut self.stream, &queued)?;
        Ok(count)
    }

    /// Close the socket. RFB has no required graceful shutdown message.
    pub fn shutdown(self) {}

    fn flush_control(&mut self, session: &mut VncSession) -> Result<usize, ConnectError> {
        let controls = session.take_control();
        let count = controls.len();
        write_control_messages(&mut self.stream, &controls)?;
        Ok(count)
    }

    fn request_update(&mut self, session: &VncSession) -> Result<(), ConnectError> {
        let now = Instant::now();
        if let Some(last) = self.last_request {
            let min = Duration::from_millis(session.update_interval_ms());
            let elapsed = now.saturating_duration_since(last);
            if let Some(delay) = min.checked_sub(elapsed) {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
        }
        let (width, height) = session.desktop_size();
        let mut msg = vec![3u8, u8::from(!self.first_update), 0, 0, 0, 0];
        msg.extend_from_slice(&width.to_be_bytes());
        msg.extend_from_slice(&height.to_be_bytes());
        self.stream
            .write_all(&msg)
            .map_err(|source| ConnectError::Io {
                phase: "write FramebufferUpdateRequest",
                source,
            })?;
        self.last_request = Some(Instant::now());
        Ok(())
    }

    fn read_framebuffer_update(
        &mut self,
        session: &mut VncSession,
    ) -> Result<(u16, usize), ConnectError> {
        let head = read_n(&mut self.stream, 3, "FramebufferUpdate header")?;
        let nrects = be16(&head[1..3]);
        let mut payload_bytes = 0usize;
        for _ in 0..nrects {
            let hdr = read_n(&mut self.stream, 12, "rectangle header")?;
            let mut reader = Reader::new(&hdr);
            let rect = parse_rectangle_header(&mut reader)?;
            let payload =
                read_rect_payload(&mut self.stream, &rect, session.format().bytes_per_pixel())?;
            payload_bytes = payload_bytes.saturating_add(payload.len());
            session.apply_rect(&rect, &payload)?;
        }
        Ok((nrects, payload_bytes))
    }

    fn skip_colour_map(&mut self) -> Result<(), ConnectError> {
        let head = read_n(&mut self.stream, 5, "SetColourMapEntries header")?;
        let entries = usize::from(be16(&head[3..5]));
        let _ = read_n(
            &mut self.stream,
            entries.saturating_mul(6),
            "colour-map entries",
        )?;
        Ok(())
    }

    fn skip_cut_text(&mut self) -> Result<(), ConnectError> {
        let head = read_n(&mut self.stream, 7, "ServerCutText header")?;
        let len = be32(&head[3..7]) as usize;
        let _ = read_n(&mut self.stream, len, "cut-text payload")?;
        Ok(())
    }

    fn elapsed_ms(&self) -> u64 {
        let ms = self.connected_at.elapsed().as_millis();
        u64::try_from(ms).unwrap_or(u64::MAX)
    }
}

struct ServerInit {
    width: u16,
    height: u16,
    format: crate::pixel::PixelFormat,
    name: String,
}

fn connect_tcp(config: &VncConfig) -> Result<TcpStream, ConnectError> {
    let addrs = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|source| ConnectError::Io {
            phase: "resolve endpoint",
            source,
        })?;
    let mut saw_addr = false;
    let mut last_error = None;
    for addr in addrs {
        saw_addr = true;
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(source) => last_error = Some(source),
        }
    }
    if !saw_addr {
        return Err(ConnectError::Resolve {
            host: config.host.clone(),
            port: config.port,
        });
    }
    let source = last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no resolved address was attempted")
    });
    Err(ConnectError::Io {
        phase: "TCP connect",
        source,
    })
}

fn handshake(stream: &mut TcpStream, password_supplied: bool) -> Result<(u32, u32), ConnectError> {
    let banner = read_n(stream, 12, "ProtocolVersion banner")?;
    let text = String::from_utf8_lossy(&banner);
    if !text.starts_with("RFB ") {
        return Err(ConnectError::Protocol(format!(
            "not an RFB server (banner {text:?})"
        )));
    }
    let server_major = text[4..7]
        .parse::<u32>()
        .map_err(|_| ConnectError::Protocol(format!("bad RFB major in banner {text:?}")))?;
    let server_minor = text[8..11]
        .parse::<u32>()
        .map_err(|_| ConnectError::Protocol(format!("bad RFB minor in banner {text:?}")))?;
    let (major, minor) = match (server_major, server_minor) {
        (3, m) if m >= 8 => (3, 8),
        (3, 7) => (3, 7),
        (3, _) => (3, 3),
        _ => {
            return Err(ConnectError::Protocol(format!(
                "unsupported RFB version {server_major}.{server_minor}"
            )));
        }
    };
    stream
        .write_all(format!("RFB {major:03}.{minor:03}\n").as_bytes())
        .map_err(|source| ConnectError::Io {
            phase: "write ProtocolVersion",
            source,
        })?;

    if minor >= 7 {
        let count = read_n(stream, 1, "security-type count")?[0];
        if count == 0 {
            return Err(ConnectError::Security(read_reason(stream)?));
        }
        let types = read_n(stream, usize::from(count), "security-type list")?;
        if !types.contains(&1) {
            return Err(ConnectError::UnsupportedSecurity {
                offered: types,
                password_supplied,
            });
        }
        stream.write_all(&[1]).map_err(|source| ConnectError::Io {
            phase: "write security-type choice",
            source,
        })?;
        if minor >= 8 {
            let result = be32(&read_n(stream, 4, "SecurityResult")?);
            if result != 0 {
                return Err(ConnectError::Security(read_reason(stream)?));
            }
        }
    } else {
        let sec = be32(&read_n(stream, 4, "3.3 security type")?);
        if sec == 0 {
            return Err(ConnectError::Security(read_reason(stream)?));
        }
        if sec != 1 {
            let offered = vec![u8::try_from(sec).unwrap_or(u8::MAX)];
            return Err(ConnectError::UnsupportedSecurity {
                offered,
                password_supplied,
            });
        }
    }
    Ok((major, minor))
}

fn client_server_init(stream: &mut TcpStream, shared: bool) -> Result<ServerInit, ConnectError> {
    stream
        .write_all(&[u8::from(shared)])
        .map_err(|source| ConnectError::Io {
            phase: "write ClientInit",
            source,
        })?;
    let head = read_n(stream, 24, "ServerInit")?;
    let mut reader = Reader::new(&head[4..20]);
    let format = parse_pixel_format(&mut reader)?;
    let name_len = be32(&head[20..24]) as usize;
    let name = String::from_utf8_lossy(&read_n(stream, name_len.min(64 * 1024), "desktop name")?)
        .into_owned();
    Ok(ServerInit {
        width: be16(&head[0..2]),
        height: be16(&head[2..4]),
        format,
        name,
    })
}

fn read_rect_payload(
    stream: &mut TcpStream,
    rect: &crate::encoding::Rectangle,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>, ConnectError> {
    match Encoding::from_i32(rect.encoding) {
        Encoding::Raw => read_n(
            stream,
            usize::from(rect.width)
                .saturating_mul(usize::from(rect.height))
                .saturating_mul(bytes_per_pixel),
            "Raw rectangle payload",
        ),
        Encoding::CopyRect => read_n(stream, 4, "CopyRect payload"),
        Encoding::Rre => read_rre_payload(stream, bytes_per_pixel),
        Encoding::Hextile => read_hextile_payload(stream, rect, bytes_per_pixel),
        Encoding::Other(code) => Err(ConnectError::Protocol(format!(
            "server sent unsupported rectangle encoding {code}"
        ))),
    }
}

fn read_rre_payload(
    stream: &mut TcpStream,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>, ConnectError> {
    let mut payload = read_n(stream, 4 + bytes_per_pixel, "RRE header/background")?;
    let count = be32(&payload[0..4]) as usize;
    let rest = read_n(
        stream,
        count.saturating_mul(bytes_per_pixel.saturating_add(8)),
        "RRE subrects",
    )?;
    payload.extend_from_slice(&rest);
    Ok(payload)
}

fn read_hextile_payload(
    stream: &mut TcpStream,
    rect: &crate::encoding::Rectangle,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>, ConnectError> {
    let mut payload = Vec::new();
    let tiles_x = usize::from(rect.width).div_ceil(16);
    let tiles_y = usize::from(rect.height).div_ceil(16);
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let tile_w = if tx + 1 == tiles_x {
                usize::from(rect.width) - tx * 16
            } else {
                16
            };
            let tile_h = if ty + 1 == tiles_y {
                usize::from(rect.height) - ty * 16
            } else {
                16
            };
            let sub = read_n(stream, 1, "Hextile subencoding")?[0];
            payload.push(sub);
            if sub & 0x01 != 0 {
                let raw = read_n(
                    stream,
                    tile_w
                        .saturating_mul(tile_h)
                        .saturating_mul(bytes_per_pixel),
                    "Hextile raw tile",
                )?;
                payload.extend_from_slice(&raw);
                continue;
            }
            if sub & 0x02 != 0 {
                payload.extend_from_slice(&read_n(stream, bytes_per_pixel, "Hextile background")?);
            }
            if sub & 0x04 != 0 {
                payload.extend_from_slice(&read_n(stream, bytes_per_pixel, "Hextile foreground")?);
            }
            if sub & 0x08 != 0 {
                let count = read_n(stream, 1, "Hextile subrect count")?[0];
                payload.push(count);
                let coloured = sub & 0x10 != 0;
                let one = 2 + if coloured { bytes_per_pixel } else { 0 };
                payload.extend_from_slice(&read_n(
                    stream,
                    usize::from(count).saturating_mul(one),
                    "Hextile subrects",
                )?);
            }
        }
    }
    Ok(payload)
}

fn write_client_messages(
    stream: &mut TcpStream,
    messages: &[RfbClientMessage],
) -> Result<(), ConnectError> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::new();
    for msg in messages {
        msg.encode(&mut buf);
    }
    stream.write_all(&buf).map_err(|source| ConnectError::Io {
        phase: "write input messages",
        source,
    })
}

fn write_control_messages(
    stream: &mut TcpStream,
    messages: &[RfbControlMessage],
) -> Result<(), ConnectError> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::new();
    for msg in messages {
        msg.encode(&mut buf);
    }
    stream.write_all(&buf).map_err(|source| ConnectError::Io {
        phase: "write control messages",
        source,
    })
}

fn read_n(stream: &mut TcpStream, n: usize, what: &'static str) -> Result<Vec<u8>, ConnectError> {
    let mut buf = vec![0u8; n];
    stream
        .read_exact(&mut buf)
        .map_err(|source| ConnectError::Io {
            phase: what,
            source,
        })?;
    Ok(buf)
}

fn read_reason(stream: &mut TcpStream) -> Result<String, ConnectError> {
    let len = be32(&read_n(stream, 4, "failure-reason length")?) as usize;
    Ok(
        String::from_utf8_lossy(&read_n(stream, len.min(4096), "failure-reason text")?)
            .into_owned(),
    )
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
mod tests {
    use super::{ConnectError, PumpOutcome, VncConnection};
    use crate::pixel::PixelFormat;
    use crate::{egui, VncConfig, VncSession};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn pump_outcome_names_processed_timeout_and_end_states() {
        let p = PumpOutcome::Processed {
            rects: 2,
            payload_bytes: 64,
        };
        assert_eq!(
            format!("{p:?}"),
            "Processed { rects: 2, payload_bytes: 64 }"
        );
        assert_eq!(PumpOutcome::TimedOut, PumpOutcome::TimedOut);
        assert_eq!(
            PumpOutcome::Terminated {
                reason: "closed".to_string()
            },
            PumpOutcome::Terminated {
                reason: "closed".to_string()
            }
        );
    }

    #[test]
    fn unsupported_security_message_names_password_auth_gap() {
        let err = ConnectError::UnsupportedSecurity {
            offered: vec![2],
            password_supplied: true,
        };
        assert!(err.to_string().contains("password auth is not implemented"));
    }

    #[test]
    fn live_connection_handshakes_pumps_a_raw_frame_and_flushes_input() {
        let (config, server) = spawn_raw_rfb_server();
        let mut session = VncSession::new(config).expect("test config is valid");
        let mut conn = VncConnection::connect(&mut session).expect("RFB handshake succeeds");

        assert_eq!(conn.negotiated().major, 3);
        assert_eq!(conn.negotiated().minor, 8);
        assert_eq!(conn.negotiated().width, 2);
        assert_eq!(conn.negotiated().height, 1);
        assert_eq!(conn.negotiated().name, "mcnf-test");

        let outcome = conn
            .pump_once(&mut session, Duration::from_millis(250))
            .expect("framebuffer update decodes");
        assert_eq!(
            outcome,
            PumpOutcome::Processed {
                rects: 1,
                payload_bytes: 8,
            }
        );
        let frame = session
            .frame()
            .expect("decoded raw rect marked frame dirty");
        assert_eq!(frame.size, [2, 1]);
        assert_eq!(frame.pixels[0], egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(frame.pixels[1], egui::Color32::from_rgb(0, 255, 0));

        session.send_input(&egui::Event::PointerMoved(egui::pos2(1.0, 0.0)));
        assert_eq!(
            conn.flush_input(&mut session)
                .expect("input write succeeds"),
            1
        );
        conn.shutdown();

        let captured = server.join().expect("test RFB server exits cleanly");
        assert!(
            captured
                .windows(10)
                .any(|w| w == [3, 0, 0, 0, 0, 0, 0, 2, 0, 1]),
            "client never sent the full framebuffer update request: {captured:?}"
        );
        assert!(
            captured.windows(6).any(|w| w == [5, 0, 0, 1, 0, 0]),
            "client never flushed the pointer event: {captured:?}"
        );
    }

    fn spawn_raw_rfb_server() -> (VncConfig, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test RFB listener");
        let port = listener.local_addr().expect("read listener address").port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept RFB client");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set server read timeout");
            let mut captured = Vec::new();

            stream
                .write_all(b"RFB 003.008\n")
                .expect("write protocol banner");
            captured.extend(read_exact(&mut stream, 12));
            stream.write_all(&[1, 1]).expect("offer security type None");
            captured.extend(read_exact(&mut stream, 1));
            stream
                .write_all(&0u32.to_be_bytes())
                .expect("write security success");
            captured.extend(read_exact(&mut stream, 1));
            stream.write_all(&server_init()).expect("write ServerInit");

            read_until_framebuffer_request(&mut stream, &mut captured);
            stream
                .write_all(&raw_framebuffer_update())
                .expect("write raw framebuffer update");
            read_one_client_message(&mut stream, &mut captured);
            captured
        });
        (
            VncConfig::new("127.0.0.1").with_port(port).shared(true),
            handle,
        )
    }

    fn read_exact(stream: &mut TcpStream, len: usize) -> Vec<u8> {
        let mut buf = vec![0; len];
        stream.read_exact(&mut buf).expect("read client bytes");
        buf
    }

    fn read_until_framebuffer_request(stream: &mut TcpStream, captured: &mut Vec<u8>) {
        loop {
            if read_one_client_message(stream, captured) == Some(3) {
                return;
            }
        }
    }

    fn read_one_client_message(stream: &mut TcpStream, captured: &mut Vec<u8>) -> Option<u8> {
        let mut kind = [0];
        if stream.read_exact(&mut kind).is_err() {
            return None;
        }
        captured.push(kind[0]);
        match kind[0] {
            0 => captured.extend(read_exact(stream, 19)),
            2 => {
                let head = read_exact(stream, 3);
                let count = u16::from_be_bytes([head[1], head[2]]);
                captured.extend_from_slice(&head);
                captured.extend(read_exact(stream, usize::from(count) * 4));
            }
            3 => captured.extend(read_exact(stream, 9)),
            4 => captured.extend(read_exact(stream, 7)),
            5 => captured.extend(read_exact(stream, 5)),
            _ => {}
        }
        Some(kind[0])
    }

    fn server_init() -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&2u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        encode_pixel_format(PixelFormat::rgba8888(), &mut msg);
        msg.extend_from_slice(&9u32.to_be_bytes());
        msg.extend_from_slice(b"mcnf-test");
        msg
    }

    fn raw_framebuffer_update() -> Vec<u8> {
        let mut msg = vec![0, 0];
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&2u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0i32.to_be_bytes());
        msg.extend_from_slice(&[0, 0, 255, 0]);
        msg.extend_from_slice(&[0, 255, 0, 0]);
        msg
    }

    fn encode_pixel_format(format: PixelFormat, out: &mut Vec<u8>) {
        out.push(format.bits_per_pixel);
        out.push(format.depth);
        out.push(u8::from(format.big_endian));
        out.push(u8::from(format.true_color));
        out.extend_from_slice(&format.red_max.to_be_bytes());
        out.extend_from_slice(&format.green_max.to_be_bytes());
        out.extend_from_slice(&format.blue_max.to_be_bytes());
        out.push(format.red_shift);
        out.push(format.green_shift);
        out.push(format.blue_shift);
        out.extend_from_slice(&[0, 0, 0]);
    }
}
