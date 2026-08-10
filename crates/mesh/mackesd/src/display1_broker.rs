//! WL-ARCH-010 — native QEMU Display1 DMA-BUF ingress.
//!
//! QEMU's `org.qemu.Display1.Listener.ScanoutDMABUF` delivers a borrowed
//! display frame as a D-Bus Unix file descriptor plus explicit geometry and
//! modifier metadata. This module is the daemon-side protocol boundary: it
//! validates the bounded descriptor before handing ownership to the node-local
//! display sink. It does not copy frames through VNC/SPICE or a network socket.
//! The shell's DRM/EGL layer consumes the descriptor and can import it through
//! PRIME/KMS; recovery remains a typed fallback when the native path is absent.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::fs;
use std::io::{IoSlice, IoSliceMut, Read};
use std::os::fd::{AsFd, OwnedFd as StdOwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mackes_mesh_types::workloads::WorkloadAttachmentLease;
use rustix::net::{
    recvmsg, sendmsg, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};
use serde::{Deserialize, Serialize};
use zbus::zvariant::OwnedFd as ZvariantOwnedFd;

/// QEMU's native Display1 listener object path.
pub const DISPLAY1_LISTENER_PATH: &str = "/org/qemu/Display1/Listener";
/// QEMU's native Display1 listener interface.
pub const DISPLAY1_LISTENER_INTERFACE: &str = "org.qemu.Display1.Listener";
/// Maximum accepted frame dimension, preventing hostile allocation geometry.
pub const MAX_SCANOUT_DIMENSION: u32 = 16_384;
/// Maximum accepted stride (64 KiB per row is ample for 16K RGBA scanout).
pub const MAX_SCANOUT_STRIDE: u32 = 65_536;
/// QEMU's console object used for listener registration.
pub const DISPLAY1_CONSOLE_PATH: &str = "/org/qemu/Display1/Console_0";
/// Root for per-lease node-local attachment sockets. The lease id is already
/// path-safe at the Workload contract boundary; callers must not concatenate
/// arbitrary input here.
pub const DISPLAY1_SOCKET_ROOT: &str = "/run/mde/display1";
/// Version for the bounded shell-to-daemon attachment handshake.
pub const DISPLAY1_HANDSHAKE_SCHEMA_VERSION: u16 = 1;
const MAX_DISPLAY1_HANDSHAKE_BYTES: usize = 4 * 1024;
const DISPLAY1_PRESENT_ACK: u8 = 0xA5;

/// Derive the broker endpoint from a validated attachment lease id. The path
/// never crosses mde-bus; only the opaque lease remains in the projection.
#[must_use]
pub fn display1_socket_path(lease_id: &str) -> Option<PathBuf> {
    display1_socket_path_at(Path::new(DISPLAY1_SOCKET_ROOT), lease_id)
}

/// Derive a per-lease endpoint below an injected root. Tests use a temporary
/// root; production passes [`DISPLAY1_SOCKET_ROOT`].
#[must_use]
pub fn display1_socket_path_at(root: &Path, lease_id: &str) -> Option<PathBuf> {
    if lease_id.is_empty()
        || lease_id.len() > mackes_mesh_types::workloads::MAX_WORKLOAD_IDENTIFIER_BYTES
        || !lease_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    Some(root.join(format!("{lease_id}.sock")))
}

/// Kernel-authenticated credentials observed on a local attachment socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    /// Peer process id.
    pub pid: i32,
    /// Peer user id.
    pub uid: u32,
    /// Peer group id.
    pub gid: u32,
}

/// Read `SO_PEERCRED` without trusting a caller-supplied pid or uid.
pub fn peer_credentials<Fd: AsFd>(fd: Fd) -> Result<PeerCredentials, std::io::Error> {
    let credentials = rustix::net::sockopt::get_socket_peercred(fd)?;
    Ok(PeerCredentials {
        pid: rustix::process::Pid::as_raw(Some(credentials.pid)),
        uid: credentials.uid.as_raw(),
        gid: credentials.gid.as_raw(),
    })
}

/// A validated native QEMU DMA-BUF frame. The fd is owned by the receiver and
/// is closed when the sink drops the frame.
pub struct Display1DmaBufFrame {
    /// The received DMA-BUF descriptor.
    pub dmabuf: StdOwnedFd,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// DRM fourcc pixel format.
    pub fourcc: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Whether the first row is the top of the image.
    pub y0_top: bool,
}

/// Errors produced before a native frame reaches the KMS/EGL sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display1Error {
    /// Geometry or stride was outside the bounded protocol contract.
    InvalidGeometry(&'static str),
    /// The sink rejected the frame (for example, a KMS import failure).
    Sink(String),
    /// The node-local SCM_RIGHTS attachment was not authenticated or usable.
    Attachment(String),
}

impl std::fmt::Display for Display1Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGeometry(field) => write!(f, "invalid Display1 DMA-BUF {field}"),
            Self::Sink(error) => write!(f, "Display1 frame sink rejected frame: {error}"),
            Self::Attachment(error) => write!(f, "Display1 attachment rejected frame: {error}"),
        }
    }
}

impl std::error::Error for Display1Error {}

/// The native frame handoff. Production wires this to the node-local DRM/EGL
/// importer; tests use a recorder. No shell or network code implements this
/// interface.
pub trait Display1FrameSink: Send + Sync + 'static {
    /// Consume one validated frame.
    fn accept(&self, frame: Display1DmaBufFrame) -> Result<(), Display1Error>;
    /// Clear the current native scanout.
    fn disable(&self) -> Result<(), Display1Error>;
}

/// Bounded metadata sent alongside one Display1 DMA-BUF descriptor. The
/// descriptor itself never enters mde-bus or JSON; it travels only in the
/// SCM_RIGHTS ancillary message on the authenticated local socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display1FrameEnvelope {
    /// Expiring Workload attachment lease name.
    pub lease_id: String,
    /// Stable Workload identity bound by the reconciler.
    pub workload_id: String,
    /// Desired-state generation bound by the reconciler.
    pub generation: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// DRM fourcc pixel format.
    pub fourcc: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Whether the first row is the top of the image.
    pub y0_top: bool,
}

/// The first message on a per-lease socket. It proves that the shell holds
/// the exact one-use nonce from the Workload projection before the daemon
/// accepts the peer as a frame relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display1AttachHello {
    /// Local broker protocol version.
    pub schema_version: u16,
    /// Lease named by the Workload projection.
    pub lease_id: String,
    /// One-use nonce bound to the lease.
    pub nonce: String,
    /// Workload identity bound to the lease.
    pub workload_id: String,
    /// Desired-state generation bound to the lease.
    pub generation: u64,
}

impl Display1AttachHello {
    fn from_lease(lease: &WorkloadAttachmentLease) -> Self {
        Self {
            schema_version: DISPLAY1_HANDSHAKE_SCHEMA_VERSION,
            lease_id: lease.lease_id.clone(),
            nonce: lease.nonce.clone(),
            workload_id: lease.workload_id.as_str().to_owned(),
            generation: lease.generation,
        }
    }

    fn validate(&self, lease: &WorkloadAttachmentLease) -> Result<(), Display1Error> {
        if self.schema_version != DISPLAY1_HANDSHAKE_SCHEMA_VERSION
            || self.lease_id != lease.lease_id
            || self.workload_id != lease.workload_id.as_str()
            || self.generation != lease.generation
        {
            return Err(Display1Error::Attachment(
                "Display1 handshake lease mismatch".into(),
            ));
        }
        if self.nonce != lease.nonce {
            return Err(Display1Error::Attachment(
                "Display1 handshake nonce mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// A one-use, peer-credential-bound local attachment capability. Constructing
/// a relay consumes the nonce; the resulting lease may carry multiple frames
/// until expiry, but a second attach attempt with the same nonce is impossible
/// for the caller because the broker does not retain it.
#[derive(Debug)]
pub struct Display1ScmRightsRelay {
    stream: UnixStream,
    peer: PeerCredentials,
    lease: WorkloadAttachmentLease,
}

/// Node-local broker that consumes attachment nonces exactly once before it
/// creates a peer-bound relay. The nonce store is deliberately process-local;
/// the capability itself is already authenticated by the Workload worker and
/// never crosses the mesh bus.
#[derive(Debug, Default)]
pub struct Display1ScmRightsBroker {
    used_nonces: Mutex<HashSet<String>>,
}

impl Display1ScmRightsBroker {
    /// Create an empty broker nonce set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume `presented_nonce` once, then bind the socket to its kernel peer
    /// and expiring Workload lease.
    pub fn attach(
        &self,
        stream: UnixStream,
        expected_peer: PeerCredentials,
        lease: WorkloadAttachmentLease,
        presented_nonce: &str,
        expected_nonce: &str,
        now_ms: u64,
    ) -> Result<Display1ScmRightsRelay, Display1Error> {
        if presented_nonce.is_empty() || presented_nonce != expected_nonce {
            return Err(Display1Error::Attachment("one-use nonce rejected".into()));
        }
        {
            let mut used = self
                .used_nonces
                .lock()
                .map_err(|_| Display1Error::Attachment("nonce store poisoned".into()))?;
            if !used.insert(presented_nonce.to_owned()) {
                return Err(Display1Error::Attachment("one-use nonce replayed".into()));
            }
        }
        match Display1ScmRightsRelay::attach(
            stream,
            expected_peer,
            lease,
            presented_nonce,
            expected_nonce,
            now_ms,
        ) {
            Ok(relay) => Ok(relay),
            Err(error) => {
                if let Ok(mut used) = self.used_nonces.lock() {
                    used.remove(presented_nonce);
                }
                Err(error)
            }
        }
    }

    /// Attach using the nonce bound into the Workload lease. This is the
    /// production call shape; callers cannot substitute a nonce from another
    /// workload or generation.
    pub fn attach_for_lease(
        &self,
        stream: UnixStream,
        expected_peer: PeerCredentials,
        lease: WorkloadAttachmentLease,
        presented_nonce: &str,
        now_ms: u64,
    ) -> Result<Display1ScmRightsRelay, Display1Error> {
        let expected_nonce = lease.nonce.clone();
        self.attach(
            stream,
            expected_peer,
            lease,
            presented_nonce,
            &expected_nonce,
            now_ms,
        )
    }
}

impl Display1ScmRightsRelay {
    /// Bind a socket endpoint to the kernel peer and consume the exact nonce.
    pub fn attach(
        stream: UnixStream,
        expected_peer: PeerCredentials,
        lease: WorkloadAttachmentLease,
        presented_nonce: &str,
        expected_nonce: &str,
        now_ms: u64,
    ) -> Result<Self, Display1Error> {
        if presented_nonce.is_empty() || presented_nonce != expected_nonce {
            return Err(Display1Error::Attachment("one-use nonce rejected".into()));
        }
        lease
            .validate(now_ms)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        let peer = peer_credentials(&stream)
            .map_err(|error| Display1Error::Attachment(format!("SO_PEERCRED: {error}")))?;
        if peer != expected_peer {
            return Err(Display1Error::Attachment(
                "socket peer credentials changed".into(),
            ));
        }
        Ok(Self {
            stream,
            peer,
            lease,
        })
    }

    /// Send one validated frame by SCM_RIGHTS, after rechecking the kernel
    /// peer and lease expiry. The metadata is bounded and the descriptor is
    /// borrowed for the send; ownership remains with the sender.
    pub fn send_frame(
        &self,
        frame: &Display1DmaBufFrame,
        now_ms: u64,
    ) -> Result<(), Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        self.lease
            .validate(now_ms)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        validate_scanout(
            frame.width,
            frame.height,
            frame.stride,
            frame.fourcc,
            frame.modifier,
        )?;
        let envelope = serde_json::to_vec(&Display1FrameEnvelope {
            lease_id: self.lease.lease_id.clone(),
            workload_id: self.lease.workload_id.as_str().to_owned(),
            generation: self.lease.generation,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            fourcc: frame.fourcc,
            modifier: frame.modifier,
            y0_top: frame.y0_top,
        })
        .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        let fd = frame.dmabuf.as_fd();
        let fds = [fd];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        if !ancillary.push(SendAncillaryMessage::ScmRights(&fds)) {
            return Err(Display1Error::Attachment(
                "SCM_RIGHTS buffer too small".into(),
            ));
        }
        let sent = sendmsg(
            &self.stream,
            &[IoSlice::new(&envelope)],
            &mut ancillary,
            // The QEMU callback must never block behind a slow shell. A
            // full local socket is reported as bounded attachment pressure;
            // the reconciler then keeps the operation in first-frame state.
            SendFlags::DONTWAIT,
        )
        .map_err(|error| Display1Error::Attachment(format!("SCM_RIGHTS send: {error}")))?;
        if sent != envelope.len() {
            return Err(Display1Error::Attachment("short SCM_RIGHTS frame".into()));
        }
        Ok(())
    }

    /// Poll the one-byte shell acknowledgement emitted only after a
    /// successful KMS modeset/page-flip. An idle socket is not readiness, and
    /// EOF is a disconnect rather than an acknowledgement.
    fn presentation_acknowledged(&self) -> Result<bool, Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        let mut ack = [0_u8; 1];
        let mut stream = &self.stream;
        match stream.read(&mut ack) {
            Ok(0) => Ok(false),
            Ok(1) if ack[0] == DISPLAY1_PRESENT_ACK => Ok(true),
            Ok(1) => Err(Display1Error::Attachment(
                "invalid Display1 presentation acknowledgement".into(),
            )),
            Ok(_) => unreachable!("one-byte acknowledgement buffer"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(error) => Err(Display1Error::Attachment(format!(
                "read Display1 presentation acknowledgement: {error}"
            ))),
        }
    }
}

impl Display1FrameSink for Display1ScmRightsRelay {
    fn accept(&self, frame: Display1DmaBufFrame) -> Result<(), Display1Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        self.send_frame(&frame, now_ms)
    }

    fn disable(&self) -> Result<(), Display1Error> {
        // Dropping the relay closes the authenticated local socket and all
        // in-flight descriptors. The listener owner decides when to drop it.
        Ok(())
    }
}

/// Shared sink used by the node-local socket server and the QEMU Display1
/// listener. It keeps only one authenticated relay and records the first
/// successfully delivered frame for the Workload reconciler.
pub struct Display1AttachmentSink {
    relay: Mutex<Option<Display1ScmRightsRelay>>,
    frame_delivered: AtomicBool,
    first_frame: AtomicBool,
}

impl Display1AttachmentSink {
    fn new() -> Self {
        Self {
            relay: Mutex::new(None),
            frame_delivered: AtomicBool::new(false),
            first_frame: AtomicBool::new(false),
        }
    }

    fn install(&self, relay: Display1ScmRightsRelay) -> Result<(), Display1Error> {
        let mut current = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        *current = Some(relay);
        self.frame_delivered.store(false, Ordering::Release);
        self.first_frame.store(false, Ordering::Release);
        Ok(())
    }

    /// Whether the authenticated shell acknowledged that KMS successfully
    /// presented a delivered QEMU frame.
    ///
    /// The shell sends one fixed byte after the successful modeset/page-flip.
    /// EOF remains a disconnect and an idle socket remains not-ready, so a
    /// shell crash after receive cannot be mistaken for presentation.
    #[must_use]
    pub fn first_frame_seen(&self) -> bool {
        if self.first_frame.load(Ordering::Acquire) {
            return true;
        }
        if !self.frame_delivered.load(Ordering::Acquire) {
            return false;
        }
        let acknowledged = self
            .relay
            .lock()
            .ok()
            .and_then(|relay| {
                relay
                    .as_ref()
                    .map(Display1ScmRightsRelay::presentation_acknowledged)
            })
            .and_then(Result::ok)
            .unwrap_or(false);
        if acknowledged {
            self.first_frame.store(true, Ordering::Release);
        }
        acknowledged
    }
}

impl Display1FrameSink for Display1AttachmentSink {
    fn accept(&self, frame: Display1DmaBufFrame) -> Result<(), Display1Error> {
        let relay = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        let relay = relay.as_ref().ok_or_else(|| {
            Display1Error::Attachment("no authenticated shell relay is attached".into())
        })?;
        relay.send_frame(&frame, display1_now_ms())?;
        self.frame_delivered.store(true, Ordering::Release);
        Ok(())
    }

    fn disable(&self) -> Result<(), Display1Error> {
        let mut relay = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        *relay = None;
        self.frame_delivered.store(false, Ordering::Release);
        self.first_frame.store(false, Ordering::Release);
        Ok(())
    }
}

/// A lease-bound, root-owned Unix socket that authenticates the shell before
/// any SCM_RIGHTS frame is accepted. The listener is bounded by lease expiry
/// and is retired on drop, stop, or broker shutdown.
pub struct Display1AttachmentServer {
    lease: WorkloadAttachmentLease,
    socket_path: PathBuf,
    sink: Arc<Display1AttachmentSink>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Display1AttachmentServer {
    /// Start a production broker below `/run/mde/display1`.
    pub fn start(lease: WorkloadAttachmentLease) -> Result<Self, Display1Error> {
        Self::start_at(Path::new(DISPLAY1_SOCKET_ROOT), lease)
    }

    /// Start a broker below an injected root. This is also the seam used by
    /// farm tests; no test needs to mutate the production `/run` hierarchy.
    pub fn start_at(root: &Path, lease: WorkloadAttachmentLease) -> Result<Self, Display1Error> {
        lease
            .validate(display1_now_ms())
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        let socket_path = display1_socket_path_at(root, &lease.lease_id).ok_or_else(|| {
            Display1Error::Attachment("lease id cannot name a Display1 socket".into())
        })?;
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                Display1Error::Attachment(format!("create broker root: {error}"))
            })?;
        }
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                match UnixStream::connect(&socket_path) {
                    Ok(_) => {
                        return Err(Display1Error::Attachment(
                            "Display1 broker socket is already active".into(),
                        ));
                    }
                    Err(probe) if probe.kind() == std::io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(&socket_path).map_err(|error| {
                            Display1Error::Attachment(format!(
                                "remove stale broker socket: {error}"
                            ))
                        })?;
                        UnixListener::bind(&socket_path).map_err(|error| {
                            Display1Error::Attachment(format!("bind broker socket: {error}"))
                        })?
                    }
                    Err(probe) if probe.kind() == std::io::ErrorKind::NotFound => {
                        UnixListener::bind(&socket_path).map_err(|error| {
                            Display1Error::Attachment(format!("bind broker socket: {error}"))
                        })?
                    }
                    Err(probe) => {
                        return Err(Display1Error::Attachment(format!(
                            "probe existing broker socket: {probe}"
                        )));
                    }
                }
            }
            Err(error) => {
                return Err(Display1Error::Attachment(format!(
                    "bind broker socket: {error}"
                )));
            }
        };
        listener.set_nonblocking(true).map_err(|error| {
            Display1Error::Attachment(format!("configure broker socket: {error}"))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o660));
        }
        let sink = Arc::new(Display1AttachmentSink::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_sink = Arc::clone(&sink);
        let thread_lease = lease.clone();
        let thread_socket_path = socket_path.clone();
        let thread = thread::Builder::new()
            .name(format!("display1-{}", lease.lease_id))
            .spawn(move || {
                serve_attachment_socket(
                    listener,
                    thread_lease,
                    thread_sink,
                    thread_shutdown,
                    thread_socket_path,
                );
            })
            .map_err(|error| {
                Display1Error::Attachment(format!("spawn broker listener: {error}"))
            })?;
        Ok(Self {
            lease,
            socket_path,
            sink,
            shutdown,
            thread: Some(thread),
        })
    }

    /// Lease projected to the shell and bound to this broker.
    #[must_use]
    pub fn lease(&self) -> &WorkloadAttachmentLease {
        &self.lease
    }

    /// Socket endpoint advertised only through node-local state.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Sink to pass to `register_display1_listener` once QEMU's D-Bus address
    /// is available.
    #[must_use]
    pub fn frame_sink(&self) -> Arc<dyn Display1FrameSink> {
        Arc::clone(&self.sink) as Arc<dyn Display1FrameSink>
    }

    /// Whether QEMU has delivered at least one frame to the attached shell.
    #[must_use]
    pub fn first_frame_seen(&self) -> bool {
        self.sink.first_frame_seen()
    }
}

impl Drop for Display1AttachmentServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn serve_attachment_socket(
    listener: UnixListener,
    lease: WorkloadAttachmentLease,
    sink: Arc<Display1AttachmentSink>,
    shutdown: Arc<AtomicBool>,
    socket_path: PathBuf,
) {
    let broker = Display1ScmRightsBroker::new();
    while !shutdown.load(Ordering::Acquire) && display1_now_ms() < lease.expires_at_ms {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let hello = match read_display1_hello(&mut stream, &lease) {
                    Ok(hello) => hello,
                    Err(_) => continue,
                };
                let peer = match peer_credentials(&stream) {
                    Ok(peer) => peer,
                    Err(_) => continue,
                };
                let _ = stream.set_nonblocking(true);
                match broker.attach_for_lease(
                    stream,
                    peer,
                    lease.clone(),
                    &hello.nonce,
                    display1_now_ms(),
                ) {
                    Ok(relay) => {
                        let _ = sink.install(relay);
                    }
                    Err(_) => continue,
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    // Expiry is a revocation boundary, not merely an admission-loop stop. A
    // runtime can outlive its operation poll, so clear the relay/readiness
    // state and unlink the endpoint here instead of waiting for Arc teardown.
    let _ = sink.disable();
    let _ = fs::remove_file(socket_path);
}

fn read_display1_hello(
    stream: &mut UnixStream,
    lease: &WorkloadAttachmentLease,
) -> Result<Display1AttachHello, Display1Error> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).map_err(|error| {
        Display1Error::Attachment(format!("Display1 handshake length: {error}"))
    })?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_DISPLAY1_HANDSHAKE_BYTES {
        return Err(Display1Error::Attachment(
            "Display1 handshake is outside the bounded size".into(),
        ));
    }
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| Display1Error::Attachment(format!("Display1 handshake body: {error}")))?;
    let hello = serde_json::from_slice::<Display1AttachHello>(&body)
        .map_err(|error| Display1Error::Attachment(format!("Display1 handshake JSON: {error}")))?;
    hello.validate(lease)?;
    Ok(hello)
}

fn display1_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Receive one SCM_RIGHTS frame on the shell side and validate its lease and
/// peer credentials before returning an owned DMA-BUF descriptor.
pub fn receive_display1_frame(
    stream: &UnixStream,
    expected_peer: PeerCredentials,
    lease: &WorkloadAttachmentLease,
    now_ms: u64,
) -> Result<Display1DmaBufFrame, Display1Error> {
    if peer_credentials(stream).map_err(|error| Display1Error::Attachment(error.to_string()))?
        != expected_peer
    {
        return Err(Display1Error::Attachment("peer credential mismatch".into()));
    }
    lease
        .validate(now_ms)
        .map_err(|error| Display1Error::Attachment(error.to_string()))?;
    let mut bytes = [0_u8; 16 * 1024];
    let mut iov = [IoSliceMut::new(&mut bytes)];
    let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut control);
    let received = recvmsg(stream, &mut iov, &mut ancillary, RecvFlags::empty())
        .map_err(|error| Display1Error::Attachment(format!("SCM_RIGHTS receive: {error}")))?;
    let envelope: Display1FrameEnvelope = serde_json::from_slice(&bytes[..received.bytes])
        .map_err(|error| Display1Error::Attachment(format!("frame envelope: {error}")))?;
    if envelope.lease_id != lease.lease_id
        || envelope.workload_id != lease.workload_id.as_str()
        || envelope.generation != lease.generation
    {
        return Err(Display1Error::Attachment(
            "frame lease or generation mismatch".into(),
        ));
    }
    validate_scanout(
        envelope.width,
        envelope.height,
        envelope.stride,
        envelope.fourcc,
        envelope.modifier,
    )?;
    let mut descriptor = None;
    for message in ancillary.drain() {
        if let RecvAncillaryMessage::ScmRights(mut fds) = message {
            descriptor = fds.next();
            break;
        }
    }
    let dmabuf =
        descriptor.ok_or_else(|| Display1Error::Attachment("missing DMA-BUF FD".into()))?;
    Ok(Display1DmaBufFrame {
        dmabuf,
        width: envelope.width,
        height: envelope.height,
        stride: envelope.stride,
        fourcc: envelope.fourcc,
        modifier: envelope.modifier,
        y0_top: envelope.y0_top,
    })
}

/// Validate the metadata QEMU supplied before touching a graphics API.
pub fn validate_scanout(
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    _modifier: u64,
) -> Result<(), Display1Error> {
    if width == 0 || width > MAX_SCANOUT_DIMENSION {
        return Err(Display1Error::InvalidGeometry("width"));
    }
    if height == 0 || height > MAX_SCANOUT_DIMENSION {
        return Err(Display1Error::InvalidGeometry("height"));
    }
    if stride < width.saturating_mul(4) || stride > MAX_SCANOUT_STRIDE {
        return Err(Display1Error::InvalidGeometry("stride"));
    }
    if fourcc == 0 {
        return Err(Display1Error::InvalidGeometry("fourcc"));
    }
    Ok(())
}

/// D-Bus object implementing QEMU's client-side Display1 listener interface.
pub struct Display1Listener {
    sink: Arc<dyn Display1FrameSink>,
}

impl Display1Listener {
    /// Build a listener that forwards validated frames to `sink`.
    #[must_use]
    pub fn new(sink: Arc<dyn Display1FrameSink>) -> Self {
        Self { sink }
    }
}

/// A registered native Display1 peer. Keeping both ends alive is essential:
/// QEMU owns the transferred socket end while zbus serves the listener end.
pub struct Display1Peer {
    /// The peer-to-peer zbus connection that serves the listener object.
    pub connection: zbus::Connection,
    /// The QEMU control-bus connection used for registration and input calls.
    pub qemu: zbus::Connection,
    /// Kernel credentials of the QEMU endpoint that received the listener FD.
    /// The value is retained for lease/audit binding; callers must not accept
    /// a pid or uid supplied on mde-bus.
    pub qemu_peer: PeerCredentials,
}

/// Register a real peer-to-peer Display1 listener with QEMU.
///
/// The QEMU control address is supplied by the already-admitted Workload
/// adapter; no D-Bus address is ever placed on mde-bus. The socket descriptor
/// is transferred through the D-Bus `h` argument, which is SCM_RIGHTS under the
/// transport, and the listener object is served on the peer connection.
pub async fn register_display1_listener(
    qemu_address: &str,
    sink: Arc<dyn Display1FrameSink>,
) -> Result<Display1Peer, Box<dyn std::error::Error + Send + Sync>> {
    let (client_stream, qemu_stream) = tokio::net::UnixStream::pair()?;
    let listener = Display1Listener::new(sink);
    let peer = zbus::connection::Builder::unix_stream(client_stream)
        .p2p()
        .serve_at(DISPLAY1_LISTENER_PATH, listener)?
        .build()
        .await?;
    let qemu = zbus::connection::Builder::address(qemu_address)?
        .build()
        .await?;
    let proxy = zbus::Proxy::new(
        &qemu,
        "org.qemu",
        DISPLAY1_CONSOLE_PATH,
        "org.qemu.Display1.Console",
    )
    .await?;
    let std_stream = qemu_stream.into_std()?;
    let qemu_peer = peer_credentials(&std_stream)?;
    let duplicated = rustix::io::dup(std_stream.as_fd())?;
    let fd = ZvariantOwnedFd::from(duplicated);
    proxy
        .call::<_, _, ()>("RegisterListener", &(Vec::<u8>::new(), fd))
        .await?;
    Ok(Display1Peer {
        connection: peer,
        qemu,
        qemu_peer,
    })
}

#[zbus::interface(name = "org.qemu.Display1.Listener")]
impl Display1Listener {
    /// Receive a zero-copy QEMU scanout frame.
    #[zbus(name = "ScanoutDMABUF")]
    async fn scanout_dmabuf(
        &self,
        dmabuf: ZvariantOwnedFd,
        width: u32,
        height: u32,
        stride: u32,
        fourcc: u32,
        modifier: u64,
        y0_top: bool,
    ) -> zbus::fdo::Result<()> {
        validate_scanout(width, height, stride, fourcc, modifier)
            .map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
        self.sink
            .accept(Display1DmaBufFrame {
                dmabuf: dmabuf.into(),
                width,
                height,
                stride,
                fourcc,
                modifier,
                y0_top,
            })
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    /// Ask the KMS sink to refresh the current DMA-BUF region.
    #[zbus(name = "UpdateDMABUF")]
    async fn update_dmabuf(
        &self,
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
    ) -> zbus::fdo::Result<()> {
        Ok(())
    }

    /// Disable native scanout and return to the shell's normal surface.
    #[zbus(name = "Disable")]
    async fn disable(&self) -> zbus::fdo::Result<()> {
        self.sink
            .disable()
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{
        WorkloadAttachmentProtocol, WorkloadId, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    };
    use std::io::Write;
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Default)]
    struct Sink {
        frames: Mutex<Vec<(u32, u32, u32, u32)>>,
        disabled: Mutex<u32>,
    }

    impl Display1FrameSink for Sink {
        fn accept(&self, frame: Display1DmaBufFrame) -> Result<(), Display1Error> {
            let _fd = frame.dmabuf.as_fd();
            self.frames.lock().expect("frames").push((
                frame.width,
                frame.height,
                frame.stride,
                frame.fourcc,
            ));
            Ok(())
        }

        fn disable(&self) -> Result<(), Display1Error> {
            *self.disabled.lock().expect("disabled") += 1;
            Ok(())
        }
    }

    #[test]
    fn rejects_unbounded_or_unusable_geometry() {
        assert!(matches!(
            validate_scanout(0, 100, 400, 0x34325258, 0),
            Err(Display1Error::InvalidGeometry("width"))
        ));
        assert!(matches!(
            validate_scanout(100, 100, 100, 0x34325258, 0),
            Err(Display1Error::InvalidGeometry("stride"))
        ));
        assert!(matches!(
            validate_scanout(100, 100, 400, 0, 0),
            Err(Display1Error::InvalidGeometry("fourcc"))
        ));
        assert!(validate_scanout(1920, 1080, 7680, 0x34325258, 0).is_ok());
    }

    #[test]
    fn listener_contract_names_the_native_qemu_surface() {
        assert_eq!(DISPLAY1_LISTENER_PATH, "/org/qemu/Display1/Listener");
        assert_eq!(DISPLAY1_LISTENER_INTERFACE, "org.qemu.Display1.Listener");
        assert_eq!(
            display1_socket_path("lease-display1"),
            Some(std::path::PathBuf::from(
                "/run/mde/display1/lease-display1.sock",
            ))
        );
        assert!(display1_socket_path("../escape").is_none());
        let (_left, right) = UnixStream::pair().expect("fd");
        assert!(right.as_fd().as_raw_fd() >= 0);
    }

    #[test]
    fn peer_credentials_are_kernel_derived() {
        let (left, right) = UnixStream::pair().expect("socketpair");
        let credentials = peer_credentials(&left).expect("peer credentials");
        assert!(credentials.pid > 0);
        assert_eq!(credentials.uid, rustix::process::getuid().as_raw());
        drop(right);
    }

    #[test]
    fn scm_rights_relay_binds_frame_to_one_use_lease_and_peer() {
        let (sender, receiver) = UnixStream::pair().expect("socketpair");
        let sender_peer = peer_credentials(&sender).expect("sender peer");
        let receiver_peer = peer_credentials(&receiver).expect("receiver peer");
        let workload_id = WorkloadId::new("browser-seat15").expect("workload id");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-display1".into(),
            nonce: "nonce-1".into(),
            workload_id,
            generation: 7,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 2_000,
        };
        let relay = Display1ScmRightsRelay::attach(
            sender,
            sender_peer,
            lease.clone(),
            "nonce-1",
            "nonce-1",
            1_000,
        )
        .expect("attach");
        let frame = Display1DmaBufFrame {
            dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
            width: 64,
            height: 32,
            stride: 256,
            fourcc: 0x3432_5258,
            modifier: 0,
            y0_top: true,
        };
        relay.send_frame(&frame, 1_001).expect("send frame");
        let received =
            receive_display1_frame(&receiver, receiver_peer, &lease, 1_002).expect("receive frame");
        assert_eq!(received.width, 64);
        assert_eq!(received.height, 32);
        assert!(received.dmabuf.as_fd().as_raw_fd() >= 0);
        assert!(matches!(
            Display1ScmRightsRelay::attach(
                UnixStream::pair().expect("second socketpair").0,
                sender_peer,
                lease,
                "nonce-1",
                "different-nonce",
                1_000,
            ),
            Err(Display1Error::Attachment(message)) if message.contains("one-use nonce")
        ));
    }

    #[test]
    fn broker_rejects_nonce_replay_before_socket_attachment() {
        let broker = Display1ScmRightsBroker::new();
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-replay".into(),
            nonce: "nonce-replay".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 1,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 2_000,
        };
        let (first, _) = UnixStream::pair().expect("socketpair");
        let expected_peer = peer_credentials(&first).expect("peer");
        let relay = broker
            .attach(
                first,
                expected_peer,
                lease.clone(),
                "nonce-replay",
                "nonce-replay",
                1_000,
            )
            .expect("first attach");
        drop(relay);
        let (second, _) = UnixStream::pair().expect("socketpair");
        let error = broker
            .attach(
                second,
                expected_peer,
                lease,
                "nonce-replay",
                "nonce-replay",
                1_000,
            )
            .expect_err("nonce replay must fail");
        assert!(
            matches!(error, Display1Error::Attachment(message) if message.contains("replayed"))
        );
    }

    #[test]
    fn duplicate_server_cannot_replace_an_active_lease_socket() {
        let temp = tempfile::tempdir().expect("temp");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-duplicate-server".into(),
            nonce: "nonce-duplicate-server".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 2,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let original = Display1AttachmentServer::start_at(temp.path(), lease.clone())
            .expect("start original broker");

        let duplicate = match Display1AttachmentServer::start_at(temp.path(), lease) {
            Ok(_) => panic!("duplicate broker replaced an active lease socket"),
            Err(error) => error,
        };
        assert!(matches!(
            duplicate,
            Display1Error::Attachment(message) if message.contains("already active")
        ));
        assert!(original.socket_path().exists());
        UnixStream::connect(original.socket_path()).expect("original broker remains reachable");
    }

    #[test]
    fn stale_socket_is_replaced_after_the_previous_listener_is_gone() {
        let temp = tempfile::tempdir().expect("temp");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-stale-server".into(),
            nonce: "nonce-stale-server".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 2,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let socket_path =
            display1_socket_path_at(temp.path(), &lease.lease_id).expect("bounded socket path");
        fs::create_dir_all(socket_path.parent().expect("socket parent")).expect("broker root");
        let stale = UnixListener::bind(&socket_path).expect("stale listener");
        drop(stale);

        let replacement = Display1AttachmentServer::start_at(temp.path(), lease)
            .expect("replace connection-refused stale socket");
        UnixStream::connect(replacement.socket_path()).expect("replacement broker is reachable");
    }

    #[test]
    fn socket_server_authenticates_handshake_and_tracks_first_frame() {
        let temp = tempfile::tempdir().expect("temp");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-server".into(),
            nonce: "nonce-server".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 3,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let server =
            Display1AttachmentServer::start_at(temp.path(), lease.clone()).expect("start broker");
        let mut stream = UnixStream::connect(server.socket_path()).expect("connect broker");
        let hello = serde_json::to_vec(&Display1AttachHello::from_lease(&lease)).expect("hello");
        let length = u32::try_from(hello.len()).expect("bounded hello");
        stream
            .write_all(&length.to_be_bytes())
            .expect("hello length");
        stream.write_all(&hello).expect("hello body");

        let sink = server.frame_sink();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let frame = Display1DmaBufFrame {
                dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
                width: 64,
                height: 32,
                stride: 256,
                fourcc: 0x3432_5258,
                modifier: 0,
                y0_top: true,
            };
            if sink.accept(frame).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                panic!("broker did not accept the authenticated shell handshake");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let peer = peer_credentials(&stream).expect("peer credentials");
        let received =
            receive_display1_frame(&stream, peer, &lease, display1_now_ms()).expect("first frame");
        assert_eq!(received.width, 64);
        assert!(
            !server.first_frame_seen(),
            "socket delivery must not complete Workload readiness"
        );
        stream
            .write_all(&[DISPLAY1_PRESENT_ACK])
            .expect("presentation acknowledgement");
        while !server.first_frame_seen() {
            if Instant::now() >= deadline {
                panic!("broker did not observe the KMS presentation acknowledgement");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn expired_server_revokes_relay_readiness_and_socket() {
        let temp = tempfile::tempdir().expect("temp");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-expiry-cleanup".into(),
            nonce: "nonce-expiry-cleanup".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 4,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(500),
        };
        let server =
            Display1AttachmentServer::start_at(temp.path(), lease.clone()).expect("start broker");
        let socket_path = server.socket_path().to_path_buf();
        let mut stream = UnixStream::connect(&socket_path).expect("connect broker");
        let hello = serde_json::to_vec(&Display1AttachHello::from_lease(&lease)).expect("hello");
        let length = u32::try_from(hello.len()).expect("bounded hello");
        stream
            .write_all(&length.to_be_bytes())
            .expect("hello length");
        stream.write_all(&hello).expect("hello body");

        let sink = server.frame_sink();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let frame = Display1DmaBufFrame {
                dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
                width: 64,
                height: 32,
                stride: 256,
                fourcc: 0x3432_5258,
                modifier: 0,
                y0_top: true,
            };
            if sink.accept(frame).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                panic!("broker did not install the authenticated relay");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let peer = peer_credentials(&stream).expect("peer credentials");
        receive_display1_frame(&stream, peer, &lease, display1_now_ms()).expect("first frame");
        stream
            .write_all(&[DISPLAY1_PRESENT_ACK])
            .expect("presentation acknowledgement");
        while !server.first_frame_seen() {
            if Instant::now() >= deadline {
                panic!("broker did not observe readiness before expiry");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        while socket_path.exists() || server.first_frame_seen() {
            if Instant::now() >= deadline {
                panic!("expired broker did not revoke its socket and readiness");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
