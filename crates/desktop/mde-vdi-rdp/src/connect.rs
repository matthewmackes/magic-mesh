//! The live RDP connect layer (`live-connect` feature) — E12-4.
//!
//! Everything below rides the `ironrdp` async stack over a real TCP link:
//! [`RdpConnection::connect`] runs the full connection sequence (X.224
//! negotiation → TLS upgrade → optional CredSSP → capability exchange →
//! finalization) and the pump methods move real wire bytes both ways. The
//! egui-facing state machine stays in [`RdpSession`] — this layer only
//! *feeds* it, through the very same public methods the unit tests drive
//! ([`RdpSession::apply_rect`] on decode, [`RdpSession::take_input`] on
//! input), so the live path and the tested path do not diverge.
//!
//! **Codec tier contract (E12-10):** the connection is *built from*
//! [`RdpSession::connect_settings`] — the session's target [`QualityTier`]
//! decides the negotiated colour depth / codecs / performance flags / bulk
//! compression — and a successful connect calls
//! [`RdpSession::mark_tier_applied`], clearing
//! [`RdpSession::needs_reconnect`]. Reconnecting through this same entry
//! point is exactly how a tier change takes effect (see [`crate::tier`]).
//!
//! **Runtime shape:** the shell and the tests are synchronous, so the
//! connection owns one small current-thread tokio runtime behind a blocking
//! facade. TLS is rustls (via `ironrdp-tls`); the workspace bans OpenSSL
//! linkage (§3 substrate lock). Certificate validation is intentionally
//! disabled by `ironrdp-tls` — RDP hosts overwhelmingly present self-signed
//! certificates and the mesh link itself is already Nebula-authenticated.
//!
//! Honest bounds: CredSSP is in-band NTLM only (a Kerberos KDC round trip
//! surfaces as a typed error, not a silent retry), and a server-initiated
//! Deactivation-Reactivation ends the pump with
//! [`ConnectError::Reactivation`] — callers reconnect, which is the same
//! recovery path a tier change already exercises.

use std::time::{Duration, Instant};

use ironrdp_connector::credssp::KerberosConfig;
use ironrdp_connector::sspi::generator::NetworkRequest;
use ironrdp_connector::{
    ClientConnector, ConnectorError, ConnectorErrorExt as _, Credentials, DesktopSize, ServerName,
};
use ironrdp_graphics::image_processing::PixelFormat as IronPixelFormat;
use ironrdp_pdu::gcc::KeyboardType;
use ironrdp_pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::PointerFlags;
use ironrdp_pdu::input::mouse_x::PointerXFlags;
use ironrdp_pdu::input::{MousePdu, MouseXPdu};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp_pdu::rdp::client_info::TimezoneInfo;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageOutput, SessionError};
use ironrdp_tokio::{
    connect_begin, connect_finalize, mark_as_upgraded, FramedWrite as _, NetworkClient, TokioFramed,
};
use tokio::net::TcpStream;

use crate::config::RdpConfig;
use crate::input::{MouseButton, RdpInputEvent};
use crate::link::QualityTier;
use crate::pixel::{FramebufferError, PixelFormat};
use crate::session::RdpSession;
use crate::tier::RdpTierSettings;

/// Hard ceiling on the whole connection sequence (TCP + TLS + RDP handshake).
/// A peer that cannot finish negotiating in this window is not going to.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The client name advertised in the GCC core data (max 15 chars on the wire).
const CLIENT_NAME: &str = "mde-vdi";

/// Why the live connect layer failed — every wire-facing failure is typed so
/// the shell can distinguish "wrong password" from "link died" (governance:
/// no stringly-typed errors, no panics on the wire path).
#[derive(Debug)]
pub enum ConnectError {
    /// The tokio runtime could not be built (resource exhaustion).
    Runtime(std::io::Error),
    /// Transport-level I/O failed; `context` says which step.
    Io {
        /// The step that was executing (connect / read / write).
        context: &'static str,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// The TLS upgrade failed.
    Tls(std::io::Error),
    /// The server's TLS certificate carried no extractable public key — the
    /// RDP security exchange cannot bind to the channel without it.
    NoServerPublicKey,
    /// The `ironrdp` connection sequence failed (negotiation, CredSSP,
    /// capability exchange, licensing...).
    Connector(ConnectorError),
    /// The `ironrdp` active-stage processing failed mid-session.
    Session(SessionError),
    /// A decoded update did not fit the session framebuffer — the negotiated
    /// desktop differs from the [`RdpConfig`] geometry the session was built
    /// with (rebuild the session at [`Negotiated::desktop_size`]).
    Blit(FramebufferError),
    /// A connection-sequence phase exceeded [`CONNECT_TIMEOUT`].
    Timeout {
        /// The phase that timed out.
        phase: &'static str,
    },
    /// The server initiated a Deactivation-Reactivation sequence (e.g. a
    /// server-side resize). This thin pump does not replay the activation
    /// state machine — reconnect, exactly as for a tier change.
    Reactivation,
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Runtime(e) => write!(f, "tokio runtime build failed: {e}"),
            Self::Io { context, source } => write!(f, "rdp transport {context}: {source}"),
            Self::Tls(e) => write!(f, "rdp TLS upgrade failed: {e}"),
            Self::NoServerPublicKey => {
                write!(f, "server TLS certificate has no extractable public key")
            }
            Self::Connector(e) => write!(f, "rdp connection sequence failed: {e}"),
            Self::Session(e) => write!(f, "rdp active stage failed: {e}"),
            Self::Blit(e) => write!(f, "decoded update does not fit the session desktop: {e}"),
            Self::Timeout { phase } => write!(
                f,
                "rdp {phase} timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ),
            Self::Reactivation => write!(
                f,
                "server initiated deactivation-reactivation; reconnect to continue"
            ),
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(e) | Self::Tls(e) | Self::Io { source: e, .. } => Some(e),
            Self::Connector(e) => Some(e),
            Self::Session(e) => Some(e),
            Self::Blit(e) => Some(e),
            Self::NoServerPublicKey | Self::Timeout { .. } | Self::Reactivation => None,
        }
    }
}

/// What one [`RdpConnection::pump_once`] call observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpOutcome {
    /// One inbound PDU was processed; `painted_rects` regions were blitted
    /// into the session framebuffer (0 = a control PDU, nothing visual).
    Processed {
        /// Number of updated regions pushed through [`RdpSession::apply_rect`].
        painted_rects: usize,
    },
    /// Nothing arrived inside the timeout — recorded on the session as a
    /// link stall probe (E12-10), not an error.
    TimedOut,
    /// The server ended the session gracefully.
    Terminated {
        /// The server's stated reason.
        reason: String,
    },
}

/// What the live connection actually negotiated — the evidence surface for
/// the shell's HUD and for the live proof test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// The desktop geometry the server granted (normally the requested one;
    /// if it differs, rebuild the [`RdpSession`] at this size).
    pub desktop_size: (u16, u16),
    /// The bulk compression the server accepted, if any (tier-driven).
    pub compression: Option<ironrdp_pdu::rdp::client_info::CompressionType>,
    /// The [`QualityTier`] whose [`RdpTierSettings`] built this connection.
    pub tier: QualityTier,
    /// The MCS I/O channel id (diagnostic evidence).
    pub io_channel_id: u16,
    /// The MCS user channel id (diagnostic evidence).
    pub user_channel_id: u16,
}

/// CredSSP network client that honestly refuses out-of-band round trips: the
/// mesh connect layer speaks in-band NTLM only (no Kerberos KDC reachable
/// from a thin client), and pretending otherwise would hang the handshake.
struct NoNetworkClient;

impl NetworkClient for NoNetworkClient {
    fn send(
        &mut self,
        _network_request: &NetworkRequest,
    ) -> impl core::future::Future<Output = Result<Vec<u8>, ConnectorError>> {
        core::future::ready(Err(ConnectorError::general(
            "CredSSP requested an out-of-band network round trip (KDC); \
             the mde connect layer supports in-band NTLM only",
        )))
    }
}

/// The [`ironrdp_connector::Config`] for one connect attempt: the caller's
/// [`RdpConfig`] (host / credentials / geometry) + the session's tier-derived
/// [`RdpTierSettings`] (colour depth, codecs, performance flags, bulk
/// compression — the E12-10 knobs). TLS and CredSSP are both offered; the
/// negotiation picks what the server actually speaks (xrdp: TLS, Windows:
/// CredSSP).
#[must_use]
pub fn connector_config_for(
    config: &RdpConfig,
    settings: &RdpTierSettings,
) -> ironrdp_connector::Config {
    ironrdp_connector::Config {
        desktop_size: DesktopSize {
            width: config.width,
            height: config.height,
        },
        desktop_scale_factor: 100, // 100% — the protocol-legal "no scaling"
        enable_tls: true,
        enable_credssp: true,
        credentials: Credentials::UsernamePassword {
            username: config.username.clone(),
            password: config.password.clone(),
        },
        domain: config.domain.clone(),
        client_build: 0,
        client_name: CLIENT_NAME.to_owned(),
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0, // 0 = the server's active layout
        ime_file_name: String::new(),
        bitmap: Some(settings.bitmap_config()),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon: true, // credentials are supplied — go straight to the desktop
        enable_audio_playback: false,
        performance_flags: settings.performance_flags,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        compression_type: settings.bulk_compression,
        // Composite the remote pointer into the framebuffer: the shell renders
        // ONE texture, egui-native, with no client-side cursor plumbing.
        enable_server_pointer: true,
        pointer_software_rendering: true,
        multitransport_flags: None,
    }
}

/// Translate one queued [`RdpInputEvent`] intent into `ironrdp` fast-path
/// input events, appending to `out`. `pointer` is the session's last pointer
/// position — wheel events carry a position on the RDP wire but the egui
/// wheel intent (correctly) does not.
pub fn push_fastpath_events(
    intent: RdpInputEvent,
    pointer: (u16, u16),
    out: &mut Vec<FastPathInputEvent>,
) {
    match intent {
        RdpInputEvent::PointerMove { x, y } => {
            out.push(FastPathInputEvent::MouseEvent(MousePdu {
                flags: PointerFlags::MOVE,
                number_of_wheel_rotation_units: 0,
                x_position: x,
                y_position: y,
            }));
        }
        RdpInputEvent::PointerButton { button, down, x, y } => {
            let down_flag = |flags| {
                if down {
                    flags | PointerFlags::DOWN
                } else {
                    flags
                }
            };
            let xdown_flag = |flags| {
                if down {
                    flags | PointerXFlags::DOWN
                } else {
                    flags
                }
            };
            match button {
                MouseButton::Left => out.push(FastPathInputEvent::MouseEvent(MousePdu {
                    flags: down_flag(PointerFlags::LEFT_BUTTON),
                    number_of_wheel_rotation_units: 0,
                    x_position: x,
                    y_position: y,
                })),
                MouseButton::Right => out.push(FastPathInputEvent::MouseEvent(MousePdu {
                    flags: down_flag(PointerFlags::RIGHT_BUTTON),
                    number_of_wheel_rotation_units: 0,
                    x_position: x,
                    y_position: y,
                })),
                MouseButton::Middle => out.push(FastPathInputEvent::MouseEvent(MousePdu {
                    flags: down_flag(PointerFlags::MIDDLE_BUTTON_OR_WHEEL),
                    number_of_wheel_rotation_units: 0,
                    x_position: x,
                    y_position: y,
                })),
                MouseButton::X1 => out.push(FastPathInputEvent::MouseEventEx(MouseXPdu {
                    flags: xdown_flag(PointerXFlags::BUTTON1),
                    x_position: x,
                    y_position: y,
                })),
                MouseButton::X2 => out.push(FastPathInputEvent::MouseEventEx(MouseXPdu {
                    flags: xdown_flag(PointerXFlags::BUTTON2),
                    x_position: x,
                    y_position: y,
                })),
            }
        }
        RdpInputEvent::Wheel { delta, horizontal } => {
            let axis = if horizontal {
                PointerFlags::HORIZONTAL_WHEEL
            } else {
                PointerFlags::VERTICAL_WHEEL
            };
            out.push(FastPathInputEvent::MouseEvent(MousePdu {
                flags: axis,
                // The encoder derives WHEEL_NEGATIVE from the sign.
                number_of_wheel_rotation_units: delta,
                x_position: pointer.0,
                y_position: pointer.1,
            }));
        }
        RdpInputEvent::Key { scancode, down } => {
            let mut flags = KeyboardFlags::empty();
            if !down {
                flags |= KeyboardFlags::RELEASE;
            }
            if scancode.extended {
                flags |= KeyboardFlags::EXTENDED;
            }
            out.push(FastPathInputEvent::KeyboardEvent(flags, scancode.code));
        }
        RdpInputEvent::Unicode(ch) => {
            // Fast-path unicode input carries UTF-16 code units; a non-BMP
            // char is its surrogate pair, each unit pressed then released.
            let mut units = [0_u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                out.push(FastPathInputEvent::UnicodeKeyboardEvent(
                    KeyboardFlags::empty(),
                    *unit,
                ));
                out.push(FastPathInputEvent::UnicodeKeyboardEvent(
                    KeyboardFlags::RELEASE,
                    *unit,
                ));
            }
        }
    }
}

/// The framed transport once TLS is up.
type TlsFramed = TokioFramed<ironrdp_tls::TlsStream<TcpStream>>;

/// A live RDP connection: the `ironrdp` active stage + decoded surface on one
/// side, the egui-facing [`RdpSession`] on the other.
pub struct RdpConnection {
    runtime: tokio::runtime::Runtime,
    framed: TlsFramed,
    active_stage: ActiveStage,
    image: DecodedImage,
    negotiated: Negotiated,
    started: Instant,
}

impl RdpConnection {
    /// Run the full RDP connection sequence for `session` — TCP connect, X.224
    /// negotiation, TLS upgrade, CredSSP if the server selected it, capability
    /// exchange and finalization — built from [`RdpSession::connect_settings`]
    /// so the session's target tier drives the negotiated encoding, then
    /// [`RdpSession::mark_tier_applied`] records that the tier is live.
    ///
    /// # Errors
    /// [`ConnectError`] — typed per phase: TCP/TLS transport, `ironrdp`
    /// negotiation (bad credentials surface here as a connector error), or
    /// timeout after [`CONNECT_TIMEOUT`].
    pub fn connect(session: &mut RdpSession) -> Result<Self, ConnectError> {
        let tier = session.quality_tier();
        let settings = session.connect_settings();
        let config = connector_config_for(session.config(), &settings);
        let host = session.config().host.clone();
        let port = session.config().port;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ConnectError::Runtime)?;

        let (framed, connection_result) = runtime.block_on(Self::handshake(config, &host, port))?;

        let negotiated = Negotiated {
            desktop_size: (
                connection_result.desktop_size.width,
                connection_result.desktop_size.height,
            ),
            compression: connection_result.compression_type,
            tier,
            io_channel_id: connection_result.io_channel_id,
            user_channel_id: connection_result.user_channel_id,
        };
        tracing::info!(
            desktop_width = negotiated.desktop_size.0,
            desktop_height = negotiated.desktop_size.1,
            tier = tier.label(),
            compression = ?negotiated.compression,
            "rdp connected"
        );

        let image = DecodedImage::new(
            IronPixelFormat::RgbA32,
            connection_result.desktop_size.width,
            connection_result.desktop_size.height,
        );
        let active_stage = ActiveStage::new(connection_result);

        // The connection was built from the target tier's settings: the
        // session's reconnect-gated tier change is now applied (E12-10).
        session.mark_tier_applied();

        Ok(Self {
            runtime,
            framed,
            active_stage,
            image,
            negotiated,
            started: Instant::now(),
        })
    }

    /// The connection sequence proper, on the runtime. Split out so every
    /// phase shares the one [`CONNECT_TIMEOUT`] policy.
    async fn handshake(
        config: ironrdp_connector::Config,
        host: &str,
        port: u16,
    ) -> Result<(TlsFramed, ironrdp_connector::ConnectionResult), ConnectError> {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| ConnectError::Timeout {
                phase: "TCP connect",
            })?
            .map_err(|source| ConnectError::Io {
                context: "TCP connect",
                source,
            })?;
        let client_addr = stream.local_addr().map_err(|source| ConnectError::Io {
            context: "local_addr",
            source,
        })?;

        let mut framed = TokioFramed::new(stream);
        let mut connector = ClientConnector::new(config, client_addr);

        let should_upgrade =
            tokio::time::timeout(CONNECT_TIMEOUT, connect_begin(&mut framed, &mut connector))
                .await
                .map_err(|_| ConnectError::Timeout {
                    phase: "negotiation",
                })?
                .map_err(ConnectError::Connector)?;

        let initial_stream = framed.into_inner_no_leftover();
        let (tls_stream, server_cert) =
            tokio::time::timeout(CONNECT_TIMEOUT, ironrdp_tls::upgrade(initial_stream, host))
                .await
                .map_err(|_| ConnectError::Timeout {
                    phase: "TLS upgrade",
                })?
                .map_err(ConnectError::Tls)?;
        let server_public_key = ironrdp_tls::extract_tls_server_public_key(&server_cert)
            .ok_or(ConnectError::NoServerPublicKey)?
            .to_vec();

        let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
        let mut framed = TokioFramed::new(tls_stream);
        let mut network_client = NoNetworkClient;
        let connection_result = tokio::time::timeout(
            CONNECT_TIMEOUT,
            connect_finalize(
                upgraded,
                connector,
                &mut framed,
                &mut network_client,
                ServerName::new(host),
                server_public_key,
                None::<KerberosConfig>,
            ),
        )
        .await
        .map_err(|_| ConnectError::Timeout {
            phase: "finalization",
        })?
        .map_err(ConnectError::Connector)?;

        Ok((framed, connection_result))
    }

    /// What the server actually granted (geometry, compression, tier).
    #[must_use]
    pub const fn negotiated(&self) -> &Negotiated {
        &self.negotiated
    }

    /// Milliseconds since the connection came up — the session's probe clock.
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Write one already-encoded frame to the wire.
    fn write_frame(&mut self, bytes: &[u8]) -> Result<(), ConnectError> {
        self.runtime
            .block_on(self.framed.write_all(bytes))
            .map_err(|source| ConnectError::Io {
                context: "write",
                source,
            })
    }

    /// Blit one updated region of the decoded surface into the session
    /// framebuffer through the same [`RdpSession::apply_rect`] the unit tests
    /// exercise. The decoded surface is RGBA (see [`RdpConnection::connect`]),
    /// so the copy is format-preserving.
    fn blit(
        session: &mut RdpSession,
        image: &DecodedImage,
        region: &InclusiveRectangle,
    ) -> Result<(), ConnectError> {
        // Guard before `data_for_rect` slices — a region escaping the surface
        // must surface as a typed error, never a panic.
        if usize::from(region.right) >= usize::from(image.width())
            || usize::from(region.bottom) >= usize::from(image.height())
        {
            return Err(ConnectError::Blit(FramebufferError::RectOutOfBounds {
                rect: (
                    usize::from(region.left),
                    usize::from(region.top),
                    usize::from(region.width()),
                    usize::from(region.height()),
                ),
                surface: (usize::from(image.width()), usize::from(image.height())),
            }));
        }
        session
            .apply_rect(
                usize::from(region.left),
                usize::from(region.top),
                usize::from(region.width()),
                usize::from(region.height()),
                PixelFormat::Rgba,
                image.data_for_rect(region),
                image.stride(),
            )
            .map_err(ConnectError::Blit)
    }

    /// Handle the non-graphics outputs of one active-stage step, writing
    /// response frames and blitting painted regions. Returns the number of
    /// regions painted, or the terminal outcome if the server ended things.
    fn apply_outputs(
        &mut self,
        session: &mut RdpSession,
        outputs: Vec<ActiveStageOutput>,
    ) -> Result<PumpOutcome, ConnectError> {
        let mut painted_rects = 0_usize;
        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(frame) => self.write_frame(&frame)?,
                ActiveStageOutput::GraphicsUpdate(region) => {
                    Self::blit(session, &self.image, &region)?;
                    painted_rects += 1;
                }
                ActiveStageOutput::Terminate(reason) => {
                    return Ok(PumpOutcome::Terminated {
                        reason: reason.description(),
                    });
                }
                ActiveStageOutput::DeactivateAll(_) => return Err(ConnectError::Reactivation),
                // Pointer styling is composited into the framebuffer
                // (pointer_software_rendering) — the styling hints and the
                // UDP sideband/autodetect offers need no action here.
                ActiveStageOutput::PointerDefault
                | ActiveStageOutput::PointerHidden
                | ActiveStageOutput::PointerPosition { .. }
                | ActiveStageOutput::PointerBitmap(_)
                | ActiveStageOutput::MultitransportRequest(_)
                | ActiveStageOutput::AutoDetect(_) => {}
            }
        }
        Ok(PumpOutcome::Processed { painted_rects })
    }

    /// Read and process **one** inbound PDU (bounded by `timeout`): decode
    /// graphics into the session framebuffer, answer protocol pings, and feed
    /// the session's link probes ([`RdpSession::record_frame`] on data,
    /// [`RdpSession::record_stall`] on timeout — the E12-10 signal).
    ///
    /// # Errors
    /// [`ConnectError`] on transport I/O, `ironrdp` processing, a blit that
    /// no longer fits the session, or a server-initiated reactivation.
    pub fn pump_once(
        &mut self,
        session: &mut RdpSession,
        timeout: Duration,
    ) -> Result<PumpOutcome, ConnectError> {
        let read = self
            .runtime
            .block_on(tokio::time::timeout(timeout, self.framed.read_pdu()));
        let (action, frame) = match read {
            Err(_elapsed) => {
                session.record_stall(self.now_ms());
                return Ok(PumpOutcome::TimedOut);
            }
            Ok(Err(source)) => {
                return Err(ConnectError::Io {
                    context: "read",
                    source,
                })
            }
            Ok(Ok(pdu)) => pdu,
        };
        session.record_frame(self.now_ms(), frame.len());

        let outputs = self
            .active_stage
            .process(&mut self.image, action, &frame)
            .map_err(ConnectError::Session)?;
        self.apply_outputs(session, outputs)
    }

    /// Drain the session's queued input intents ([`RdpSession::take_input`])
    /// onto the wire as fast-path input events. Returns how many wire events
    /// were sent (modifier synthesis means this can exceed the egui events).
    ///
    /// # Errors
    /// [`ConnectError`] on encoding or transport failure.
    pub fn flush_input(&mut self, session: &mut RdpSession) -> Result<usize, ConnectError> {
        let intents = session.take_input();
        if intents.is_empty() {
            return Ok(0);
        }
        let pointer = session.pointer_position();
        let mut events = Vec::with_capacity(intents.len());
        for intent in intents {
            push_fastpath_events(intent, pointer, &mut events);
        }
        let sent = events.len();

        let outputs = self
            .active_stage
            .process_fastpath_input(&mut self.image, &events)
            .map_err(ConnectError::Session)?;
        // Outputs are the encoded input frame(s) + possibly a local pointer
        // redraw region; a Terminate cannot arise from input.
        self.apply_outputs(session, outputs)?;
        Ok(sent)
    }

    /// Tell the server we are leaving and close the connection.
    ///
    /// # Errors
    /// [`ConnectError`] if the shutdown PDU cannot be built or written; the
    /// connection is consumed either way.
    pub fn shutdown(mut self, session: &mut RdpSession) -> Result<(), ConnectError> {
        let outputs = self
            .active_stage
            .graceful_shutdown()
            .map_err(ConnectError::Session)?;
        self.apply_outputs(session, outputs)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{connector_config_for, push_fastpath_events, ConnectError, PumpOutcome};
    use crate::config::RdpConfig;
    use crate::input::{MouseButton, RdpInputEvent, Scancode};
    use crate::link::QualityTier;
    use crate::pixel::FramebufferError;
    use crate::tier::RdpTierSettings;
    use ironrdp_connector::Credentials;
    use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
    use ironrdp_pdu::input::mouse::PointerFlags;
    use ironrdp_pdu::input::mouse_x::PointerXFlags;
    use ironrdp_pdu::rdp::client_info::CompressionType;

    fn events_for(intent: RdpInputEvent) -> Vec<FastPathInputEvent> {
        let mut out = Vec::new();
        push_fastpath_events(intent, (40, 50), &mut out);
        out
    }

    #[test]
    fn connector_config_carries_the_tier_and_credentials() {
        let cfg = RdpConfig::new("host", "operator", "hunter2")
            .with_resolution(1024, 768)
            .with_domain("MESH");
        let full = connector_config_for(&cfg, &RdpTierSettings::for_tier(QualityTier::Full));
        assert_eq!(
            (full.desktop_size.width, full.desktop_size.height),
            (1024, 768)
        );
        assert!(
            full.enable_tls && full.enable_credssp,
            "offer both; server picks"
        );
        assert!(matches!(
            &full.credentials,
            Credentials::UsernamePassword { username, password }
                if username == "operator" && password == "hunter2"
        ));
        assert_eq!(full.domain.as_deref(), Some("MESH"));
        let full_bitmap = full.bitmap.expect("tier always sets a bitmap config");
        assert_eq!(full_bitmap.color_depth, 32);
        assert_eq!(full.compression_type, None);

        let minimal = connector_config_for(&cfg, &RdpTierSettings::for_tier(QualityTier::Minimal));
        let minimal_bitmap = minimal.bitmap.expect("tier always sets a bitmap config");
        assert_eq!(minimal_bitmap.color_depth, 15, "the connector's floor");
        assert!(minimal_bitmap.codecs.0.is_empty(), "no RemoteFX on Minimal");
        assert_eq!(minimal.compression_type, Some(CompressionType::Rdp61));
    }

    #[test]
    fn pointer_move_maps_to_a_move_mouse_event() {
        let evs = events_for(RdpInputEvent::PointerMove { x: 7, y: 9 });
        assert!(matches!(
            evs.as_slice(),
            [FastPathInputEvent::MouseEvent(pdu)]
                if pdu.flags == PointerFlags::MOVE && pdu.x_position == 7 && pdu.y_position == 9
        ));
    }

    #[test]
    fn button_transitions_carry_the_down_flag_only_when_pressed() {
        let down = events_for(RdpInputEvent::PointerButton {
            button: MouseButton::Left,
            down: true,
            x: 1,
            y: 2,
        });
        assert!(matches!(
            down.as_slice(),
            [FastPathInputEvent::MouseEvent(pdu)]
                if pdu.flags == PointerFlags::LEFT_BUTTON | PointerFlags::DOWN
        ));
        let up = events_for(RdpInputEvent::PointerButton {
            button: MouseButton::Right,
            down: false,
            x: 1,
            y: 2,
        });
        assert!(matches!(
            up.as_slice(),
            [FastPathInputEvent::MouseEvent(pdu)] if pdu.flags == PointerFlags::RIGHT_BUTTON
        ));
    }

    #[test]
    fn extended_buttons_ride_the_mouse_x_pdu() {
        let evs = events_for(RdpInputEvent::PointerButton {
            button: MouseButton::X2,
            down: true,
            x: 3,
            y: 4,
        });
        assert!(matches!(
            evs.as_slice(),
            [FastPathInputEvent::MouseEventEx(pdu)]
                if pdu.flags == PointerXFlags::BUTTON2 | PointerXFlags::DOWN
        ));
    }

    #[test]
    fn wheel_uses_the_session_pointer_position_and_signed_delta() {
        let evs = events_for(RdpInputEvent::Wheel {
            delta: -120,
            horizontal: false,
        });
        assert!(matches!(
            evs.as_slice(),
            [FastPathInputEvent::MouseEvent(pdu)]
                if pdu.flags == PointerFlags::VERTICAL_WHEEL
                    && pdu.number_of_wheel_rotation_units == -120
                    && (pdu.x_position, pdu.y_position) == (40, 50)
        ));
    }

    #[test]
    fn key_events_map_scancode_release_and_extended_flags() {
        let down = events_for(RdpInputEvent::Key {
            scancode: Scancode {
                code: 0x1E,
                extended: false,
            },
            down: true,
        });
        assert!(matches!(
            down.as_slice(),
            [FastPathInputEvent::KeyboardEvent(flags, 0x1E)] if flags.is_empty()
        ));
        let up_extended = events_for(RdpInputEvent::Key {
            scancode: Scancode {
                code: 0x48, // up arrow: E0-extended
                extended: true,
            },
            down: false,
        });
        assert!(matches!(
            up_extended.as_slice(),
            [FastPathInputEvent::KeyboardEvent(flags, 0x48)]
                if *flags == KeyboardFlags::RELEASE | KeyboardFlags::EXTENDED
        ));
    }

    #[test]
    fn unicode_chars_press_and_release_each_utf16_unit() {
        let bmp = events_for(RdpInputEvent::Unicode('é'));
        assert_eq!(bmp.len(), 2, "one unit: press + release");
        assert!(matches!(
            bmp.as_slice(),
            [
                FastPathInputEvent::UnicodeKeyboardEvent(down, unit_down),
                FastPathInputEvent::UnicodeKeyboardEvent(up, unit_up),
            ] if down.is_empty() && *up == KeyboardFlags::RELEASE && unit_down == unit_up
        ));
        let astral = events_for(RdpInputEvent::Unicode('🦀'));
        assert_eq!(astral.len(), 4, "surrogate pair: 2 units × press+release");
    }

    #[test]
    fn errors_render_operator_readable_messages() {
        let blit = ConnectError::Blit(FramebufferError::RectOutOfBounds {
            rect: (0, 0, 10, 10),
            surface: (4, 4),
        });
        assert!(blit.to_string().contains("does not fit"));
        let timeout = ConnectError::Timeout {
            phase: "TLS upgrade",
        };
        assert!(timeout.to_string().contains("TLS upgrade"));
        assert!(ConnectError::Reactivation.to_string().contains("reconnect"));
    }

    #[test]
    fn pump_outcomes_distinguish_paint_from_silence() {
        assert_ne!(
            PumpOutcome::Processed { painted_rects: 0 },
            PumpOutcome::TimedOut
        );
    }
}
