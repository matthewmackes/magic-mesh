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

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{AsFd, OwnedFd as StdOwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fs2::FileExt as _;
use mackes_mesh_types::workloads::WorkloadAttachmentLease;
use rustix::net::{
    accept_with, bind_unix, connect_unix, listen, recvmsg, send, sendmsg, socket_with,
    AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketFlags, SocketType,
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
const MAX_DISPLAY1_ENVELOPE_BYTES: usize = 16 * 1024;
const MAX_DISPLAY1_INPUT_BYTES: usize = 4 * 1024;
const DISPLAY1_INPUT_DBUS_TIMEOUT: Duration = Duration::from_millis(500);
const DISPLAY1_PRESENT_ACK: u8 = 0xA5;
// Linux's stable MSG_CTRUNC ABI bit. rustix 0.38 preserves unknown receive
// flags but does not expose a named constant for this result-only flag.
const DISPLAY1_MSG_CTRUNC: u32 = 0x08;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Display1PresentationPoll {
    Pending,
    Acknowledged,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Display1SocketIdentity {
    device: u64,
    inode: u64,
}

impl Display1SocketIdentity {
    fn from_path(path: &Path) -> Result<Self, Display1Error> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| Display1Error::Attachment(format!("stat broker socket: {error}")))?;
        if !metadata.file_type().is_socket() {
            return Err(Display1Error::Attachment(
                "broker endpoint is not a Unix socket".into(),
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn remove_if_owned(self, path: &Path) -> bool {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return false;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            return fs::remove_file(path).is_ok();
        }
        false
    }
}

fn lock_display1_root(socket_path: &Path) -> Result<fs::File, Display1Error> {
    let root = socket_path.parent().ok_or_else(|| {
        Display1Error::Attachment("Display1 broker socket has no root directory".into())
    })?;
    let lock = fs::File::open(root)
        .map_err(|error| Display1Error::Attachment(format!("open broker root: {error}")))?;
    lock.lock_exclusive()
        .map_err(|error| Display1Error::Attachment(format!("lock broker root: {error}")))?;
    Ok(lock)
}

fn require_seqpacket(stream: &UnixStream) -> Result<(), Display1Error> {
    let socket_type = rustix::net::sockopt::get_socket_type(stream)
        .map_err(|error| Display1Error::Attachment(format!("SO_TYPE: {error}")))?;
    if socket_type != SocketType::SEQPACKET {
        return Err(Display1Error::Attachment(
            "Display1 relay requires Unix SOCK_SEQPACKET".into(),
        ));
    }
    Ok(())
}

fn seqpacket_connect(path: &Path) -> Result<UnixStream, std::io::Error> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )?;
    let address = SocketAddrUnix::new(path)?;
    connect_unix(&socket, &address)?;
    Ok(socket.into())
}

fn seqpacket_listener(path: &Path) -> Result<UnixStream, std::io::Error> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    let address = SocketAddrUnix::new(path)?;
    bind_unix(&socket, &address)?;
    listen(&socket, 8)?;
    Ok(socket.into())
}

fn reject_truncated_packet(flags: RecvFlags, context: &str) -> Result<(), Display1Error> {
    if flags.contains(RecvFlags::TRUNC) {
        return Err(Display1Error::Attachment(format!(
            "truncated Display1 {context} packet"
        )));
    }
    if flags.bits() & DISPLAY1_MSG_CTRUNC != 0 {
        return Err(Display1Error::Attachment(format!(
            "truncated Display1 {context} ancillary data"
        )));
    }
    Ok(())
}

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

/// One bounded damage rectangle for the most recently accepted scanout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display1Damage {
    /// Left edge in scanout pixels.
    pub x: u32,
    /// Top edge in scanout pixels.
    pub y: u32,
    /// Non-zero damaged width in pixels.
    pub width: u32,
    /// Non-zero damaged height in pixels.
    pub height: u32,
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
    /// Refresh one validated region of the current native scanout.
    fn damage(&self, damage: Display1Damage) -> Result<(), Display1Error>;
    /// Clear the current native scanout.
    fn disable(&self) -> Result<(), Display1Error>;
}

/// Bounded metadata sent alongside one Display1 DMA-BUF descriptor. The
/// descriptor itself never enters mde-bus or JSON; it travels only in the
/// SCM_RIGHTS ancillary message on the authenticated local socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display1FrameEnvelope {
    /// Closed message discriminator.
    pub kind: String,
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

/// Bounded metadata for one same-buffer damage notification. No descriptor is
/// transferred; the shell retains the frame imported by the preceding frame
/// envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display1DamageEnvelope {
    /// Closed message discriminator.
    pub kind: String,
    /// Expiring Workload attachment lease name.
    pub lease_id: String,
    /// Stable Workload identity bound by the reconciler.
    pub workload_id: String,
    /// Desired-state generation bound by the reconciler.
    pub generation: u64,
    /// Validated damage rectangle.
    pub damage: Display1Damage,
}

/// One shell-to-daemon input packet after exact lease binding has been
/// validated. No input packet is permitted to carry ancillary descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display1InputMessage {
    /// Strictly increasing sequence within one authenticated relay epoch.
    pub sequence: u64,
    /// Bounded guest input action.
    pub input: Display1InputAction,
}

/// Closed Display1 input action set. Linux evdev codes are translated to QEMU
/// key numbers only at the retained QEMU peer boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Display1InputAction {
    /// Acquire or relinquish explicit guest focus.
    Focus {
        /// Requested focus state.
        focused: bool,
    },
    /// One guest keyboard edge.
    Key {
        /// Linux evdev code from the DRM seat.
        code: u32,
        /// Press (`true`) or release (`false`).
        pressed: bool,
    },
    /// Absolute and relative console-pixel pointer data.
    PointerMotion {
        /// Absolute console x pixel.
        x: u32,
        /// Absolute console y pixel.
        y: u32,
        /// Relative x pixel delta.
        dx: i32,
        /// Relative y pixel delta.
        dy: i32,
    },
    /// One guest pointer button edge.
    PointerButton {
        /// QEMU button number.
        button: u32,
        /// Press (`true`) or release (`false`).
        pressed: bool,
    },
    /// One bounded vertical wheel step.
    Wheel {
        /// Signed step (`-1` or `1`).
        steps: i32,
    },
    /// Release all retained guest edges and focus.
    ReleaseAll,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Display1InputEnvelope {
    kind: String,
    lease_id: String,
    workload_id: String,
    generation: u64,
    sequence: u64,
    input: Display1InputAction,
}

/// One non-blocking result from the authenticated shell input channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display1InputPoll {
    /// No complete input packet is ready.
    Idle,
    /// The authenticated shell relay reached EOF or failed validation.
    Disconnected,
    /// One validated exact-lease input action.
    Input(Display1InputMessage),
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
    #[cfg(test)]
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

/// A peer-credential-bound local attachment capability. The first attach binds
/// the nonce to its kernel process. That process may reconnect after a dead
/// transport, but a different process cannot replay the capability.
#[derive(Debug)]
pub struct Display1ScmRightsRelay {
    stream: UnixStream,
    peer: PeerCredentials,
    lease: WorkloadAttachmentLease,
    scanout: Mutex<Option<(u32, u32)>>,
}

/// Node-local broker that binds each attachment nonce to the first kernel peer
/// that presents it. The same process may reconnect after transport loss, but
/// another process can never replay the nonce or substitute itself as owner.
/// The nonce store is deliberately process-local; the capability itself is
/// already authenticated by the Workload worker and never crosses the mesh bus.
#[derive(Debug, Default)]
pub struct Display1ScmRightsBroker {
    nonce_owners: Mutex<HashMap<String, PeerCredentials>>,
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
        let claimed_nonce = {
            let mut owners = self
                .nonce_owners
                .lock()
                .map_err(|_| Display1Error::Attachment("nonce store poisoned".into()))?;
            match owners.get(presented_nonce) {
                Some(owner) if *owner != expected_peer => {
                    return Err(Display1Error::Attachment(
                        "attachment nonce replayed by a different kernel peer".into(),
                    ));
                }
                Some(_) => false,
                None => {
                    owners.insert(presented_nonce.to_owned(), expected_peer);
                    true
                }
            }
        };
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
                if claimed_nonce {
                    if let Ok(mut owners) = self.nonce_owners.lock() {
                        owners.remove(presented_nonce);
                    }
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
        require_seqpacket(&stream)?;
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
            scanout: Mutex::new(None),
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
            kind: "frame".into(),
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
        if envelope.is_empty() || envelope.len() > MAX_DISPLAY1_ENVELOPE_BYTES {
            return Err(Display1Error::Attachment(
                "Display1 frame envelope exceeds the bounded packet size".into(),
            ));
        }
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
        *self
            .scanout
            .lock()
            .map_err(|_| Display1Error::Attachment("scanout state poisoned".into()))? =
            Some((frame.width, frame.height));
        Ok(())
    }

    /// Send one same-buffer damage notification without transferring another
    /// descriptor. The QEMU callback remains non-blocking under socket pressure.
    pub fn send_damage(&self, damage: Display1Damage, now_ms: u64) -> Result<(), Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        self.lease
            .validate(now_ms)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        let (scanout_width, scanout_height) = self
            .scanout
            .lock()
            .map_err(|_| Display1Error::Attachment("scanout state poisoned".into()))?
            .ok_or_else(|| {
                Display1Error::Attachment("damage arrived before an accepted scanout".into())
            })?;
        validate_damage(damage, scanout_width, scanout_height)?;
        let envelope = serde_json::to_vec(&Display1DamageEnvelope {
            kind: "damage".into(),
            lease_id: self.lease.lease_id.clone(),
            workload_id: self.lease.workload_id.as_str().to_owned(),
            generation: self.lease.generation,
            damage,
        })
        .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        if envelope.is_empty() || envelope.len() > MAX_DISPLAY1_ENVELOPE_BYTES {
            return Err(Display1Error::Attachment(
                "Display1 damage envelope exceeds the bounded packet size".into(),
            ));
        }
        let sent = send(&self.stream, &envelope, SendFlags::DONTWAIT)
            .map_err(|error| Display1Error::Attachment(format!("damage send: {error}")))?;
        if sent != envelope.len() {
            return Err(Display1Error::Attachment(
                "short Display1 damage message".into(),
            ));
        }
        Ok(())
    }

    /// Poll the one-byte shell acknowledgement emitted only after a
    /// successful KMS modeset/page-flip. An idle socket is not readiness, and
    /// EOF is a disconnect rather than an acknowledgement.
    fn poll_presentation(&self) -> Result<Display1PresentationPoll, Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        let mut ack = [0_u8; 1];
        let mut iov = [IoSliceMut::new(&mut ack)];
        let mut control = [];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        let received = match recvmsg(&self.stream, &mut iov, &mut ancillary, RecvFlags::DONTWAIT) {
            Ok(received) => received,
            Err(rustix::io::Errno::AGAIN) => return Ok(Display1PresentationPoll::Pending),
            Err(error) => {
                return Err(Display1Error::Attachment(format!(
                    "read Display1 presentation acknowledgement: {error}"
                )))
            }
        };
        reject_truncated_packet(received.flags, "presentation acknowledgement")?;
        if received.bytes == 0 {
            return Ok(Display1PresentationPoll::Disconnected);
        }
        if received.bytes == 1 && ack[0] == DISPLAY1_PRESENT_ACK {
            Ok(Display1PresentationPoll::Acknowledged)
        } else {
            Err(Display1Error::Attachment(
                "invalid Display1 presentation acknowledgement".into(),
            ))
        }
    }

    /// Revalidate an already-acknowledged presentation without consuming a
    /// queued input packet. A successful page flip is not durable authority:
    /// the shell endpoint must remain connected and the lease must remain live.
    fn presentation_connected(&self, now_ms: u64) -> Result<bool, Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        self.lease
            .validate(now_ms)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;

        let mut byte = [0_u8; 1];
        let mut iov = [IoSliceMut::new(&mut byte)];
        let mut control = [];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        match recvmsg(
            &self.stream,
            &mut iov,
            &mut ancillary,
            RecvFlags::DONTWAIT | RecvFlags::PEEK,
        ) {
            Ok(received) => Ok(received.bytes != 0),
            Err(rustix::io::Errno::AGAIN) => Ok(true),
            Err(error) => Err(Display1Error::Attachment(format!(
                "probe Display1 presentation relay: {error}"
            ))),
        }
    }

    fn poll_input(&self, now_ms: u64) -> Result<Display1InputPoll, Display1Error> {
        if peer_credentials(&self.stream)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?
            != self.peer
        {
            return Err(Display1Error::Attachment("peer credential mismatch".into()));
        }
        self.lease
            .validate(now_ms)
            .map_err(|error| Display1Error::Attachment(error.to_string()))?;
        let mut body = [0_u8; MAX_DISPLAY1_INPUT_BYTES];
        let mut iov = [IoSliceMut::new(&mut body)];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        let received = match recvmsg(&self.stream, &mut iov, &mut ancillary, RecvFlags::DONTWAIT) {
            Ok(received) => received,
            Err(rustix::io::Errno::AGAIN) => return Ok(Display1InputPoll::Idle),
            Err(error) => {
                return Err(Display1Error::Attachment(format!(
                    "read Display1 input: {error}"
                )))
            }
        };
        reject_truncated_packet(received.flags, "input")?;
        if ancillary.drain().next().is_some() {
            return Err(Display1Error::Attachment(
                "Display1 input must not carry ancillary data".into(),
            ));
        }
        if received.bytes == 0 {
            return Ok(Display1InputPoll::Disconnected);
        }
        let envelope: Display1InputEnvelope = serde_json::from_slice(&body[..received.bytes])
            .map_err(|error| Display1Error::Attachment(format!("Display1 input JSON: {error}")))?;
        if envelope.kind != "input"
            || envelope.lease_id != self.lease.lease_id
            || envelope.workload_id != self.lease.workload_id.as_str()
            || envelope.generation != self.lease.generation
            || envelope.sequence == 0
        {
            return Err(Display1Error::Attachment(
                "Display1 input lease, generation, kind, or sequence mismatch".into(),
            ));
        }
        let scanout = *self
            .scanout
            .lock()
            .map_err(|_| Display1Error::Attachment("scanout state poisoned".into()))?;
        validate_input_action(&envelope.input, scanout)?;
        Ok(Display1InputPoll::Input(Display1InputMessage {
            sequence: envelope.sequence,
            input: envelope.input,
        }))
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

    fn damage(&self, damage: Display1Damage) -> Result<(), Display1Error> {
        self.send_damage(damage, display1_now_ms())
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
    input_epoch: AtomicU64,
}

impl Display1AttachmentSink {
    fn new() -> Self {
        Self {
            relay: Mutex::new(None),
            frame_delivered: AtomicBool::new(false),
            first_frame: AtomicBool::new(false),
            input_epoch: AtomicU64::new(0),
        }
    }

    fn install(&self, relay: Display1ScmRightsRelay) -> Result<(), Display1Error> {
        let mut current = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        if current.as_ref().is_some_and(|existing| {
            existing
                .presentation_connected(display1_now_ms())
                .unwrap_or(false)
        }) {
            return Err(Display1Error::Attachment(
                "an authenticated Display1 relay is already active".into(),
            ));
        }
        if current.replace(relay).is_some() {
            self.input_epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.frame_delivered.store(false, Ordering::Release);
        self.first_frame.store(false, Ordering::Release);
        Ok(())
    }

    fn poll_input(&self) -> Result<Display1InputPoll, Display1Error> {
        if !self.first_frame_seen() {
            return Ok(Display1InputPoll::Idle);
        }
        let mut relay = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        let Some(current) = relay.as_ref() else {
            return Ok(Display1InputPoll::Idle);
        };
        match current.poll_input(display1_now_ms()) {
            Ok(Display1InputPoll::Disconnected) | Err(_) => {
                *relay = None;
                self.frame_delivered.store(false, Ordering::Release);
                self.first_frame.store(false, Ordering::Release);
                self.input_epoch.fetch_add(1, Ordering::AcqRel);
                Ok(Display1InputPoll::Disconnected)
            }
            result => result,
        }
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
            let mut relay = match self.relay.lock() {
                Ok(relay) => relay,
                Err(_) => return false,
            };
            if relay.as_ref().is_some_and(|relay| {
                relay
                    .presentation_connected(display1_now_ms())
                    .unwrap_or(false)
            }) {
                return true;
            }
            relay.take();
            self.frame_delivered.store(false, Ordering::Release);
            self.first_frame.store(false, Ordering::Release);
            self.input_epoch.fetch_add(1, Ordering::AcqRel);
            return false;
        }
        if !self.frame_delivered.load(Ordering::Acquire) {
            return false;
        }
        let mut relay = match self.relay.lock() {
            Ok(relay) => relay,
            Err(_) => return false,
        };
        let presentation = relay
            .as_ref()
            .map(Display1ScmRightsRelay::poll_presentation);
        match presentation {
            Some(Ok(Display1PresentationPoll::Acknowledged)) => {
                self.first_frame.store(true, Ordering::Release);
                true
            }
            Some(Ok(Display1PresentationPoll::Pending)) | None => false,
            Some(Ok(Display1PresentationPoll::Disconnected) | Err(_)) => {
                // Before presentation acknowledgement, poll_input deliberately
                // admits nothing. Therefore this is the only production path
                // that can observe a shell which received the DMA-BUF and then
                // vanished. Retaining that relay would keep sending QEMU frames
                // and descriptors into a dead attachment until lease expiry.
                relay.take();
                self.frame_delivered.store(false, Ordering::Release);
                self.first_frame.store(false, Ordering::Release);
                self.input_epoch.fetch_add(1, Ordering::AcqRel);
                false
            }
        }
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

    fn damage(&self, damage: Display1Damage) -> Result<(), Display1Error> {
        let relay = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        relay
            .as_ref()
            .ok_or_else(|| {
                Display1Error::Attachment("no authenticated shell relay is attached".into())
            })?
            .send_damage(damage, display1_now_ms())
    }

    fn disable(&self) -> Result<(), Display1Error> {
        let mut relay = self
            .relay
            .lock()
            .map_err(|_| Display1Error::Attachment("relay store poisoned".into()))?;
        if relay.take().is_some() {
            self.input_epoch.fetch_add(1, Ordering::AcqRel);
        }
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
    socket_identity: Display1SocketIdentity,
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
        // Serialize bind/probe/stale cleanup across daemon instances. The
        // endpoint inode check below then cannot race another cooperative
        // broker replacing the stable lease pathname between stat and unlink.
        let _root_lock = lock_display1_root(&socket_path)?;
        let listener = match seqpacket_listener(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                let existing_identity = Display1SocketIdentity::from_path(&socket_path)?;
                match seqpacket_connect(&socket_path) {
                    Ok(_) => {
                        return Err(Display1Error::Attachment(
                            "Display1 broker socket is already active".into(),
                        ));
                    }
                    Err(probe) if probe.kind() == std::io::ErrorKind::ConnectionRefused => {
                        if !existing_identity.remove_if_owned(&socket_path) {
                            return Err(Display1Error::Attachment(
                                "stale broker socket changed during restart cleanup".into(),
                            ));
                        }
                        seqpacket_listener(&socket_path).map_err(|error| {
                            Display1Error::Attachment(format!("bind broker socket: {error}"))
                        })?
                    }
                    Err(probe) if probe.kind() == std::io::ErrorKind::NotFound => {
                        seqpacket_listener(&socket_path).map_err(|error| {
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o660));
        }
        let socket_identity = Display1SocketIdentity::from_path(&socket_path)?;
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
                    socket_identity,
                );
            })
            .map_err(|error| {
                Display1Error::Attachment(format!("spawn broker listener: {error}"))
            })?;
        Ok(Self {
            lease,
            socket_path,
            socket_identity,
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

    /// Poll one bounded, exact-lease shell input packet without blocking.
    pub fn poll_input(&self) -> Result<Display1InputPoll, Display1Error> {
        self.sink.poll_input()
    }

    /// Changes whenever a relay is replaced, disconnected, expired, or revoked.
    #[must_use]
    pub fn input_epoch(&self) -> u64 {
        self.sink.input_epoch.load(Ordering::Acquire)
    }
}

impl Drop for Display1AttachmentServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(_root_lock) = lock_display1_root(&self.socket_path) {
            self.socket_identity.remove_if_owned(&self.socket_path);
        }
    }
}

fn serve_attachment_socket(
    listener: UnixStream,
    lease: WorkloadAttachmentLease,
    sink: Arc<Display1AttachmentSink>,
    shutdown: Arc<AtomicBool>,
    socket_path: PathBuf,
    socket_identity: Display1SocketIdentity,
) {
    let broker = Display1ScmRightsBroker::new();
    while !shutdown.load(Ordering::Acquire) && display1_now_ms() < lease.expires_at_ms {
        match accept_with(&listener, SocketFlags::CLOEXEC | SocketFlags::NONBLOCK) {
            Ok(stream) => {
                let mut stream: UnixStream = stream.into();
                let _ = stream.set_nonblocking(false);
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
            Err(error) if error == rustix::io::Errno::AGAIN => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    // Expiry is a revocation boundary, not merely an admission-loop stop. A
    // runtime can outlive its operation poll, so clear the relay/readiness
    // state and unlink the exact endpoint this server created instead of
    // waiting for Arc teardown. A stale generation must never unlink a newer
    // server that has already reclaimed the same lease pathname.
    let _ = sink.disable();
    if let Ok(_root_lock) = lock_display1_root(&socket_path) {
        socket_identity.remove_if_owned(&socket_path);
    }
}

fn read_display1_hello(
    stream: &mut UnixStream,
    lease: &WorkloadAttachmentLease,
) -> Result<Display1AttachHello, Display1Error> {
    require_seqpacket(stream)?;
    let mut body = [0_u8; MAX_DISPLAY1_HANDSHAKE_BYTES];
    let mut iov = [IoSliceMut::new(&mut body)];
    let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut control);
    let received = recvmsg(stream, &mut iov, &mut ancillary, RecvFlags::empty())
        .map_err(|error| Display1Error::Attachment(format!("Display1 handshake: {error}")))?;
    reject_truncated_packet(received.flags, "handshake")?;
    if received.bytes == 0 {
        return Err(Display1Error::Attachment(
            "Display1 handshake is an empty packet".into(),
        ));
    }
    if ancillary.drain().next().is_some() {
        return Err(Display1Error::Attachment(
            "Display1 handshake must not carry ancillary data".into(),
        ));
    }
    let hello = serde_json::from_slice::<Display1AttachHello>(&body[..received.bytes])
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
    require_seqpacket(stream)?;
    if peer_credentials(stream).map_err(|error| Display1Error::Attachment(error.to_string()))?
        != expected_peer
    {
        return Err(Display1Error::Attachment("peer credential mismatch".into()));
    }
    lease
        .validate(now_ms)
        .map_err(|error| Display1Error::Attachment(error.to_string()))?;
    let mut bytes = [0_u8; MAX_DISPLAY1_ENVELOPE_BYTES];
    let mut iov = [IoSliceMut::new(&mut bytes)];
    let mut control = [0_u8; rustix::cmsg_space!(ScmRights(2))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut control);
    let received = recvmsg(stream, &mut iov, &mut ancillary, RecvFlags::empty())
        .map_err(|error| Display1Error::Attachment(format!("SCM_RIGHTS receive: {error}")))?;
    reject_truncated_packet(received.flags, "frame")?;
    if received.bytes == 0 {
        return Err(Display1Error::Attachment(
            "empty Display1 frame packet".into(),
        ));
    }
    let envelope: Display1FrameEnvelope = serde_json::from_slice(&bytes[..received.bytes])
        .map_err(|error| Display1Error::Attachment(format!("frame envelope: {error}")))?;
    if envelope.kind != "frame"
        || envelope.lease_id != lease.lease_id
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
            let first = fds.next();
            if descriptor.is_some() || fds.next().is_some() {
                return Err(Display1Error::Attachment(
                    "Display1 frame carried multiple descriptors".into(),
                ));
            }
            descriptor = first;
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

/// Validate a QEMU damage rectangle against the exact retained scanout.
pub fn validate_damage(
    damage: Display1Damage,
    scanout_width: u32,
    scanout_height: u32,
) -> Result<(), Display1Error> {
    if damage.width == 0
        || damage.height == 0
        || damage
            .x
            .checked_add(damage.width)
            .is_none_or(|right| right > scanout_width)
        || damage
            .y
            .checked_add(damage.height)
            .is_none_or(|bottom| bottom > scanout_height)
    {
        return Err(Display1Error::InvalidGeometry("damage"));
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
    /// Mouse mode read from QEMU after Console.Interfaces proved support.
    pub mouse_is_absolute: bool,
}

/// Daemon-owned held-input state for one retained QEMU peer. All lifecycle
/// release paths converge here so every admitted edge is released at most once.
#[derive(Debug, Default)]
pub struct Display1InputState {
    focused: bool,
    last_sequence: u64,
    held_keys: BTreeSet<u32>,
    held_buttons: BTreeSet<u32>,
}

macro_rules! qemu_input_call {
    ($proxy:expr, $method:literal, $args:expr) => {{
        tokio::time::timeout(
            DISPLAY1_INPUT_DBUS_TIMEOUT,
            $proxy.call::<_, _, ()>($method, $args),
        )
        .await
        .map_err(|_| {
            Display1Error::Attachment(format!("QEMU Display1 input {} timed out", $method))
        })?
        .map_err(input_dbus_error)
    }};
}

impl Display1InputState {
    /// Forget every edge and sequence tied to a relay that is no longer the
    /// active QEMU endpoint. Release attempts are best effort because the old
    /// endpoint may already have vanished; retaining failed edges would make
    /// them stale state for the next relay and can reject legitimate input.
    fn reset_after_relay_loss(&mut self) {
        self.focused = false;
        self.last_sequence = 0;
        self.held_keys.clear();
        self.held_buttons.clear();
    }

    fn admit_sequence(&mut self, sequence: u64) -> Result<(), Display1Error> {
        if sequence == 0 || sequence <= self.last_sequence {
            return Err(Display1Error::Attachment(
                "replayed or non-monotonic Display1 input sequence".into(),
            ));
        }
        self.last_sequence = sequence;
        Ok(())
    }

    /// Reset the per-relay sequence after releasing state held by the old relay.
    pub async fn replace_relay(&mut self, peer: &Display1Peer) -> Result<(), Display1Error> {
        let result = self.release_all(peer).await;
        // A vanished old QEMU endpoint must not retain stale sequence/focus or
        // prevent a subsequently registered relay from starting cleanly. This
        // also clears edges whose best-effort release failed above.
        self.reset_after_relay_loss();
        result
    }

    /// Apply one validated exact-lease input action through QEMU's retained
    /// Keyboard/Mouse interfaces.
    pub async fn apply(
        &mut self,
        peer: &Display1Peer,
        message: Display1InputMessage,
    ) -> Result<(), Display1Error> {
        self.admit_sequence(message.sequence)?;
        match message.input {
            Display1InputAction::Focus { focused: true } => {
                self.focused = true;
                Ok(())
            }
            Display1InputAction::Focus { focused: false } | Display1InputAction::ReleaseAll => {
                self.release_all(peer).await
            }
            Display1InputAction::Key { code, pressed } => {
                self.require_focus()?;
                let keycode = qemu_key_number(code).ok_or_else(|| {
                    Display1Error::Attachment("unsupported or host-reserved Display1 key".into())
                })?;
                let keyboard = zbus::Proxy::new(
                    &peer.qemu,
                    "org.qemu",
                    DISPLAY1_CONSOLE_PATH,
                    "org.qemu.Display1.Keyboard",
                )
                .await
                .map_err(input_dbus_error)?;
                if pressed {
                    if self.held_keys.contains(&keycode) {
                        return Err(Display1Error::Attachment(
                            "duplicate Display1 key press".into(),
                        ));
                    }
                    qemu_input_call!(keyboard, "Press", &(keycode,))?;
                    self.held_keys.insert(keycode);
                    Ok(())
                } else {
                    if !self.held_keys.contains(&keycode) {
                        return Err(Display1Error::Attachment(
                            "Display1 key release had no matching press".into(),
                        ));
                    }
                    qemu_input_call!(keyboard, "Release", &(keycode,))?;
                    self.held_keys.remove(&keycode);
                    Ok(())
                }
            }
            Display1InputAction::PointerMotion { x, y, dx, dy } => {
                self.require_focus()?;
                let mouse = zbus::Proxy::new(
                    &peer.qemu,
                    "org.qemu",
                    DISPLAY1_CONSOLE_PATH,
                    "org.qemu.Display1.Mouse",
                )
                .await
                .map_err(input_dbus_error)?;
                if peer.mouse_is_absolute {
                    qemu_input_call!(mouse, "SetAbsPosition", &(x, y))
                } else {
                    qemu_input_call!(mouse, "RelMotion", &(dx, dy))
                }
            }
            Display1InputAction::PointerButton { button, pressed } => {
                self.require_focus()?;
                if button > 2 {
                    return Err(Display1Error::Attachment(
                        "unsupported Display1 pointer button".into(),
                    ));
                }
                let mouse = zbus::Proxy::new(
                    &peer.qemu,
                    "org.qemu",
                    DISPLAY1_CONSOLE_PATH,
                    "org.qemu.Display1.Mouse",
                )
                .await
                .map_err(input_dbus_error)?;
                if pressed {
                    if self.held_buttons.contains(&button) {
                        return Err(Display1Error::Attachment(
                            "duplicate Display1 button press".into(),
                        ));
                    }
                    qemu_input_call!(mouse, "Press", &(button,))?;
                    self.held_buttons.insert(button);
                    Ok(())
                } else {
                    if !self.held_buttons.contains(&button) {
                        return Err(Display1Error::Attachment(
                            "Display1 button release had no matching press".into(),
                        ));
                    }
                    qemu_input_call!(mouse, "Release", &(button,))?;
                    self.held_buttons.remove(&button);
                    Ok(())
                }
            }
            Display1InputAction::Wheel { steps } => {
                self.require_focus()?;
                if !matches!(steps, -1 | 1) {
                    return Err(Display1Error::Attachment(
                        "unbounded Display1 wheel action".into(),
                    ));
                }
                let button = if steps > 0 { 3_u32 } else { 4_u32 };
                let mouse = zbus::Proxy::new(
                    &peer.qemu,
                    "org.qemu",
                    DISPLAY1_CONSOLE_PATH,
                    "org.qemu.Display1.Mouse",
                )
                .await
                .map_err(input_dbus_error)?;
                qemu_input_call!(mouse, "Press", &(button,))?;
                self.held_buttons.insert(button);
                qemu_input_call!(mouse, "Release", &(button,))?;
                self.held_buttons.remove(&button);
                Ok(())
            }
        }
    }

    fn require_focus(&self) -> Result<(), Display1Error> {
        if self.focused {
            Ok(())
        } else {
            Err(Display1Error::Attachment(
                "Display1 input requires explicit guest focus".into(),
            ))
        }
    }

    /// Release each held edge once, then clear explicit focus.
    pub async fn release_all(&mut self, peer: &Display1Peer) -> Result<(), Display1Error> {
        self.focused = false;
        let mut first_error = None;
        if !self.held_keys.is_empty() {
            match zbus::Proxy::new(
                &peer.qemu,
                "org.qemu",
                DISPLAY1_CONSOLE_PATH,
                "org.qemu.Display1.Keyboard",
            )
            .await
            {
                Ok(keyboard) => {
                    for keycode in self.held_keys.clone() {
                        match qemu_input_call!(keyboard, "Release", &(keycode,)) {
                            Ok(()) => {
                                self.held_keys.remove(&keycode);
                            }
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| input_dbus_error(error));
                }
            }
        }
        if !self.held_buttons.is_empty() {
            match zbus::Proxy::new(
                &peer.qemu,
                "org.qemu",
                DISPLAY1_CONSOLE_PATH,
                "org.qemu.Display1.Mouse",
            )
            .await
            {
                Ok(mouse) => {
                    for button in self.held_buttons.clone() {
                        match qemu_input_call!(mouse, "Release", &(button,)) {
                            Ok(()) => {
                                self.held_buttons.remove(&button);
                            }
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| input_dbus_error(error));
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn input_dbus_error(error: zbus::Error) -> Display1Error {
    Display1Error::Attachment(format!("QEMU Display1 input: {error}"))
}

/// Map Linux evdev keys to QEMU's xtkbd number encoding. Host-reserved keys
/// are deliberately absent. The extended set uses QEMU's documented high-bit
/// re-encoding of the E0 prefix.
fn qemu_key_number(code: u32) -> Option<u32> {
    match code {
        2..=88 => Some(code),
        96 => Some(0x9c),
        97 => Some(0x9d),
        98 => Some(0xb5),
        99 => Some(0xb7),
        100 => Some(0xb8),
        102 => Some(0xc7),
        103 => Some(0xc8),
        104 => Some(0xc9),
        105 => Some(0xcb),
        106 => Some(0xcd),
        107 => Some(0xcf),
        108 => Some(0xd0),
        109 => Some(0xd1),
        110 => Some(0xd2),
        111 => Some(0xd3),
        _ => None,
    }
}

fn validate_input_action(
    input: &Display1InputAction,
    scanout: Option<(u32, u32)>,
) -> Result<(), Display1Error> {
    match input {
        Display1InputAction::Focus { .. } | Display1InputAction::ReleaseAll => Ok(()),
        Display1InputAction::Key { code, .. } if qemu_key_number(*code).is_some() => Ok(()),
        Display1InputAction::Key { .. } => Err(Display1Error::Attachment(
            "unsupported or host-reserved Display1 key".into(),
        )),
        Display1InputAction::PointerMotion { x, y, dx, dy } => {
            let (width, height) = scanout.ok_or_else(|| {
                Display1Error::Attachment("Display1 pointer has no retained scanout".into())
            })?;
            if *x >= width
                || *y >= height
                || !(-32_768..=32_767).contains(dx)
                || !(-32_768..=32_767).contains(dy)
            {
                return Err(Display1Error::Attachment(
                    "Display1 pointer motion is outside the retained scanout".into(),
                ));
            }
            Ok(())
        }
        Display1InputAction::PointerButton { button, .. } if *button <= 2 => Ok(()),
        Display1InputAction::PointerButton { .. } => Err(Display1Error::Attachment(
            "unsupported Display1 pointer button".into(),
        )),
        Display1InputAction::Wheel { steps } if matches!(steps, -1 | 1) => Ok(()),
        Display1InputAction::Wheel { .. } => Err(Display1Error::Attachment(
            "unbounded Display1 wheel action".into(),
        )),
    }
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
    let interfaces = proxy.get_property::<Vec<String>>("Interfaces").await?;
    if !interfaces
        .iter()
        .any(|name| name == "org.qemu.Display1.Keyboard")
        || !interfaces
            .iter()
            .any(|name| name == "org.qemu.Display1.Mouse")
    {
        return Err("QEMU Display1 console lacks Keyboard or Mouse input support".into());
    }
    let mouse = zbus::Proxy::new(
        &qemu,
        "org.qemu",
        DISPLAY1_CONSOLE_PATH,
        "org.qemu.Display1.Mouse",
    )
    .await?;
    let mouse_is_absolute = mouse.get_property::<bool>("IsAbsolute").await?;
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
        mouse_is_absolute,
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
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> zbus::fdo::Result<()> {
        let damage = Display1Damage {
            x: u32::try_from(x)
                .map_err(|_| zbus::fdo::Error::InvalidArgs("negative damage x".into()))?,
            y: u32::try_from(y)
                .map_err(|_| zbus::fdo::Error::InvalidArgs("negative damage y".into()))?,
            width: u32::try_from(width)
                .map_err(|_| zbus::fdo::Error::InvalidArgs("negative damage width".into()))?,
            height: u32::try_from(height)
                .map_err(|_| zbus::fdo::Error::InvalidArgs("negative damage height".into()))?,
        };
        self.sink
            .damage(damage)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
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
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    fn seqpacket_pair() -> (UnixStream, UnixStream) {
        let (left, right) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .expect("seqpacket socketpair");
        (left.into(), right.into())
    }

    fn receive_damage_packet(receiver: &UnixStream) -> Display1DamageEnvelope {
        let mut bytes = [0_u8; 1024];
        let mut iov = [IoSliceMut::new(&mut bytes)];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        let packet = recvmsg(receiver, &mut iov, &mut ancillary, RecvFlags::empty())
            .expect("receive damage");
        reject_truncated_packet(packet.flags, "test damage").expect("complete packet");
        assert!(ancillary.drain().next().is_none());
        serde_json::from_slice(&bytes[..packet.bytes]).expect("damage envelope")
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
        let valid = Display1Damage {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert_eq!(validate_damage(valid, 100, 100), Ok(()));
        for hostile in [
            Display1Damage { width: 0, ..valid },
            Display1Damage { height: 0, ..valid },
            Display1Damage {
                x: 90,
                width: 11,
                ..valid
            },
            Display1Damage {
                y: 90,
                height: 11,
                ..valid
            },
            Display1Damage {
                x: u32::MAX,
                ..valid
            },
        ] {
            assert_eq!(
                validate_damage(hostile, 100, 100),
                Err(Display1Error::InvalidGeometry("damage"))
            );
        }
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
        let (sender, receiver) = seqpacket_pair();
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
        let first_damage = Display1Damage {
            x: 4,
            y: 5,
            width: 16,
            height: 8,
        };
        let second_damage = Display1Damage {
            x: 20,
            y: 10,
            width: 12,
            height: 6,
        };
        relay
            .send_damage(first_damage, 1_002)
            .expect("send first damage");
        relay
            .send_damage(second_damage, 1_003)
            .expect("send second damage");
        let received =
            receive_display1_frame(&receiver, receiver_peer, &lease, 1_004).expect("receive frame");
        assert_eq!(received.width, 64);
        assert_eq!(received.height, 32);
        assert!(received.dmabuf.as_fd().as_raw_fd() >= 0);
        let first_envelope = receive_damage_packet(&receiver);
        let second_envelope = receive_damage_packet(&receiver);
        assert_eq!(first_envelope.kind, "damage");
        assert_eq!(first_envelope.lease_id, "lease-display1");
        assert_eq!(first_envelope.generation, 7);
        assert_eq!(first_envelope.damage, first_damage);
        assert_eq!(second_envelope.damage, second_damage);
        assert!(matches!(
            relay.send_damage(first_damage, 2_001),
            Err(Display1Error::Attachment(message)) if message.contains("lease")
        ));
        assert!(matches!(
            Display1ScmRightsRelay::attach(
                seqpacket_pair().0,
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
    fn input_packets_are_exact_lease_bounded_and_fd_free() {
        let (daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-input".into(),
            nonce: "nonce-input".into(),
            workload_id: WorkloadId::new("browser-input").expect("workload id"),
            generation: 13,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 2_000,
        };
        let relay = Display1ScmRightsRelay::attach(
            daemon,
            peer,
            lease,
            "nonce-input",
            "nonce-input",
            1_000,
        )
        .expect("relay");
        let packet = serde_json::to_vec(&serde_json::json!({
            "kind": "input",
            "lease_id": "lease-input",
            "workload_id": "browser-input",
            "generation": 13,
            "sequence": 1,
            "input": {"action": "focus", "focused": true}
        }))
        .expect("input JSON");
        assert_eq!(
            send(&shell, &packet, SendFlags::empty()).expect("send input"),
            packet.len()
        );
        assert_eq!(
            relay.poll_input(1_001),
            Ok(Display1InputPoll::Input(Display1InputMessage {
                sequence: 1,
                input: Display1InputAction::Focus { focused: true },
            }))
        );

        let mismatched = serde_json::to_vec(&serde_json::json!({
            "kind": "input",
            "lease_id": "lease-input",
            "workload_id": "browser-other",
            "generation": 13,
            "sequence": 2,
            "input": {"action": "release_all"}
        }))
        .expect("mismatch JSON");
        assert_eq!(
            send(&shell, &mismatched, SendFlags::empty()).expect("send mismatch"),
            mismatched.len()
        );
        assert!(matches!(
            relay.poll_input(1_002),
            Err(Display1Error::Attachment(message)) if message.contains("mismatch")
        ));

        *relay.scanout.lock().expect("scanout") = Some((1920, 1080));
        let out_of_bounds = serde_json::to_vec(&serde_json::json!({
            "kind": "input",
            "lease_id": "lease-input",
            "workload_id": "browser-input",
            "generation": 13,
            "sequence": 3,
            "input": {
                "action": "pointer_motion",
                "x": 1920,
                "y": 1079,
                "dx": 0,
                "dy": 0
            }
        }))
        .expect("hostile pointer JSON");
        assert_eq!(
            send(&shell, &out_of_bounds, SendFlags::empty()).expect("send hostile pointer"),
            out_of_bounds.len()
        );
        assert!(matches!(
            relay.poll_input(1_003),
            Err(Display1Error::Attachment(message)) if message.contains("retained scanout")
        ));

        let descriptor = std::fs::File::open("/dev/null").expect("descriptor");
        let descriptors = [descriptor.as_fd()];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            sendmsg(
                &shell,
                &[IoSlice::new(&packet)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .expect("send input with fd"),
            packet.len()
        );
        assert!(matches!(
            relay.poll_input(1_004),
            Err(Display1Error::Attachment(message)) if message.contains("ancillary")
        ));
    }

    #[test]
    fn input_sequence_admission_rejects_replay_and_regression() {
        let mut state = Display1InputState::default();
        assert!(state.admit_sequence(1).is_ok());
        assert!(matches!(
            state.admit_sequence(1),
            Err(Display1Error::Attachment(message))
                if message.contains("replayed or non-monotonic")
        ));
        assert!(state.admit_sequence(3).is_ok());
        assert!(matches!(
            state.admit_sequence(2),
            Err(Display1Error::Attachment(message))
                if message.contains("replayed or non-monotonic")
        ));
    }

    #[test]
    fn relay_loss_reset_clears_stale_focus_edges_and_sequence() {
        let mut state = Display1InputState {
            focused: true,
            last_sequence: u64::MAX,
            held_keys: BTreeSet::from([30, 31]),
            held_buttons: BTreeSet::from([0, 2]),
        };

        state.reset_after_relay_loss();

        assert!(!state.focused);
        assert_eq!(state.last_sequence, 0);
        assert!(state.held_keys.is_empty());
        assert!(state.held_buttons.is_empty());
        assert!(state.admit_sequence(1).is_ok());
    }

    #[test]
    fn qemu_key_mapping_excludes_every_host_reserved_class() {
        assert_eq!(qemu_key_number(30), Some(30));
        assert_eq!(qemu_key_number(97), Some(0x9d));
        for host_only in [1, 113, 115, 125, 126, 224, 248] {
            assert_eq!(qemu_key_number(host_only), None);
        }
    }

    #[test]
    fn broker_rejects_nonce_replay_from_substituted_kernel_peer() {
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
        let (first, _) = seqpacket_pair();
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
        let (second, _) = seqpacket_pair();
        let substituted_peer = PeerCredentials {
            pid: expected_peer.pid.saturating_add(1),
            ..expected_peer
        };
        let error = broker
            .attach(
                second,
                substituted_peer,
                lease,
                "nonce-replay",
                "nonce-replay",
                1_000,
            )
            .expect_err("substituted peer replay must fail");
        assert!(
            matches!(error, Display1Error::Attachment(message) if message.contains("replayed"))
        );
    }

    #[test]
    fn dead_transport_reconnect_requires_same_owner_and_rejects_live_takeover() {
        let now_ms = display1_now_ms();
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-input-epoch".into(),
            nonce: "nonce-input-epoch".into(),
            workload_id: WorkloadId::new("browser-input-epoch").expect("workload id"),
            generation: 3,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: now_ms.saturating_add(60_000),
        };
        let broker = Display1ScmRightsBroker::new();
        let sink = Display1AttachmentSink::new();
        let (first_daemon, first_shell) = seqpacket_pair();
        let first_peer = peer_credentials(&first_daemon).expect("first peer");
        sink.install(
            broker
                .attach(
                    first_daemon,
                    first_peer,
                    lease.clone(),
                    &lease.nonce,
                    &lease.nonce,
                    now_ms,
                )
                .expect("first relay"),
        )
        .expect("install first");
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 0);

        let (live_takeover, _) = seqpacket_pair();
        let live_peer = peer_credentials(&live_takeover).expect("live takeover peer");
        let error = sink
            .install(
                broker
                    .attach(
                        live_takeover,
                        live_peer,
                        lease.clone(),
                        &lease.nonce,
                        &lease.nonce,
                        now_ms.saturating_add(1),
                    )
                    .expect("candidate relay"),
            )
            .expect_err("a live relay cannot be replaced");
        assert!(matches!(
            error,
            Display1Error::Attachment(message) if message.contains("already active")
        ));
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 0);

        drop(first_shell);
        let (second_daemon, second_shell) = seqpacket_pair();
        let second_peer = peer_credentials(&second_daemon).expect("second peer");
        sink.install(
            broker
                .attach(
                    second_daemon,
                    second_peer,
                    lease.clone(),
                    &lease.nonce,
                    &lease.nonce,
                    now_ms.saturating_add(2),
                )
                .expect("second relay"),
        )
        .expect("replace relay");
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 1);
        sink.first_frame.store(true, Ordering::Release);
        drop(second_shell);
        assert_eq!(
            sink.poll_input(),
            Ok(Display1InputPoll::Idle),
            "presentation liveness revokes the disconnected relay before input polling"
        );
        assert!(sink.relay.lock().expect("relay store").is_none());
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 2);
        sink.disable().expect("idempotent revoke");
        assert_eq!(
            sink.input_epoch.load(Ordering::Acquire),
            2,
            "an already disconnected relay is not released twice"
        );
    }

    #[test]
    fn pre_presentation_disconnect_revokes_dead_relay_and_frame_authority() {
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-pre-presentation-disconnect".into(),
            nonce: "nonce-pre-presentation-disconnect".into(),
            workload_id: WorkloadId::new("browser-pre-presentation").expect("workload id"),
            generation: 8,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let sink = Display1AttachmentSink::new();
        let (daemon, shell) = seqpacket_pair();
        let daemon_peer = peer_credentials(&daemon).expect("daemon peer");
        let shell_peer = peer_credentials(&shell).expect("shell peer");
        sink.install(
            Display1ScmRightsRelay::attach(
                daemon,
                daemon_peer,
                lease.clone(),
                &lease.nonce,
                &lease.nonce,
                display1_now_ms(),
            )
            .expect("relay"),
        )
        .expect("install relay");

        sink.accept(Display1DmaBufFrame {
            dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
            width: 64,
            height: 32,
            stride: 256,
            fourcc: 0x3432_5258,
            modifier: 0,
            y0_top: true,
        })
        .expect("send frame");
        receive_display1_frame(&shell, shell_peer, &lease, display1_now_ms())
            .expect("shell receives DMA-BUF before crashing");
        drop(shell);

        assert!(!sink.first_frame_seen());
        assert!(sink.relay.lock().expect("relay store").is_none());
        assert!(!sink.frame_delivered.load(Ordering::Acquire));
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 1);
        assert!(matches!(
            sink.accept(Display1DmaBufFrame {
                dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
                width: 64,
                height: 32,
                stride: 256,
                fourcc: 0x3432_5258,
                modifier: 0,
                y0_top: true,
            }),
            Err(Display1Error::Attachment(message))
                if message.contains("no authenticated shell relay")
        ));
    }

    #[test]
    fn post_presentation_disconnect_revokes_retained_frame_authority() {
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-post-presentation-disconnect".into(),
            nonce: "nonce-post-presentation-disconnect".into(),
            workload_id: WorkloadId::new("browser-post-presentation").expect("workload id"),
            generation: 9,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let sink = Display1AttachmentSink::new();
        let (daemon, shell) = seqpacket_pair();
        let daemon_peer = peer_credentials(&daemon).expect("daemon peer");
        let shell_peer = peer_credentials(&shell).expect("shell peer");
        sink.install(
            Display1ScmRightsRelay::attach(
                daemon,
                daemon_peer,
                lease.clone(),
                &lease.nonce,
                &lease.nonce,
                display1_now_ms(),
            )
            .expect("relay"),
        )
        .expect("install relay");

        sink.accept(Display1DmaBufFrame {
            dmabuf: std::fs::File::open("/dev/null").expect("dev null").into(),
            width: 64,
            height: 32,
            stride: 256,
            fourcc: 0x3432_5258,
            modifier: 0,
            y0_top: true,
        })
        .expect("send frame");
        receive_display1_frame(&shell, shell_peer, &lease, display1_now_ms())
            .expect("shell receives DMA-BUF");
        assert_eq!(
            send(&shell, &[DISPLAY1_PRESENT_ACK], SendFlags::empty()).expect("presentation ack"),
            1
        );
        assert!(sink.first_frame_seen());

        drop(shell);

        assert!(!sink.first_frame_seen());
        assert!(sink.relay.lock().expect("relay store").is_none());
        assert!(!sink.frame_delivered.load(Ordering::Acquire));
        assert_eq!(sink.input_epoch.load(Ordering::Acquire), 1);
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
        seqpacket_connect(original.socket_path()).expect("original broker remains reachable");
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
        let stale = seqpacket_listener(&socket_path).expect("stale listener");
        drop(stale);

        let replacement = Display1AttachmentServer::start_at(temp.path(), lease)
            .expect("replace connection-refused stale socket");
        seqpacket_connect(replacement.socket_path()).expect("replacement broker is reachable");
    }

    #[test]
    fn stale_generation_drop_cannot_unlink_newer_live_broker_socket() {
        let temp = tempfile::tempdir().expect("temp");
        let now = display1_now_ms();
        let stale_lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-restart-generation".into(),
            nonce: "nonce-restart-generation-1".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 1,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: now.saturating_add(200),
        };
        let stale = Display1AttachmentServer::start_at(temp.path(), stale_lease.clone())
            .expect("start stale-generation broker");
        let socket_path = stale.socket_path().to_path_buf();
        let deadline = Instant::now() + Duration::from_secs(2);
        while socket_path.exists() {
            assert!(
                Instant::now() < deadline,
                "stale-generation broker did not retire its endpoint"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let current_lease = WorkloadAttachmentLease {
            generation: 2,
            nonce: "nonce-restart-generation-2".into(),
            expires_at_ms: display1_now_ms().saturating_add(5_000),
            ..stale_lease
        };
        let current = Display1AttachmentServer::start_at(temp.path(), current_lease)
            .expect("start current-generation broker");
        seqpacket_connect(current.socket_path()).expect("current broker is initially reachable");

        // The stale owner still has a completed listener thread and reaches
        // its Drop cleanup only now, after the current generation reclaimed
        // the stable pathname.
        drop(stale);

        assert!(
            current.socket_path().exists(),
            "stale-generation cleanup removed the current broker endpoint"
        );
        seqpacket_connect(current.socket_path())
            .expect("current broker remains reachable after stale cleanup");
    }

    #[test]
    fn restart_cleanup_cannot_unlink_a_concurrent_newer_broker_socket() {
        let temp = tempfile::tempdir().expect("temp");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-restart-cleanup-race".into(),
            nonce: "nonce-restart-cleanup-race".into(),
            workload_id: WorkloadId::new("browser-restart-cleanup").expect("workload id"),
            generation: 17,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: display1_now_ms().saturating_add(5_000),
        };
        let socket_path = display1_socket_path_at(temp.path(), &lease.lease_id)
            .expect("restart broker socket path");
        let stale_listener = seqpacket_listener(&socket_path).expect("stale listener");
        let stale_identity =
            Display1SocketIdentity::from_path(&socket_path).expect("stale socket identity");

        drop(stale_listener);
        {
            let _current_lock = lock_display1_root(&socket_path).expect("current startup lock");
            fs::remove_file(&socket_path).expect("retire stale pathname");
        }
        let current =
            Display1AttachmentServer::start_at(temp.path(), lease).expect("current broker");

        {
            let _stale_cleanup_lock = lock_display1_root(&socket_path).expect("stale cleanup lock");
            assert!(
                !stale_identity.remove_if_owned(&socket_path),
                "restart cleanup accepted a substituted socket inode"
            );
        }
        seqpacket_connect(current.socket_path())
            .expect("concurrent newer broker remains reachable after stale cleanup");
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
        let stream = seqpacket_connect(server.socket_path()).expect("connect broker");
        let hello = serde_json::to_vec(&Display1AttachHello::from_lease(&lease)).expect("hello");
        assert_eq!(
            send(&stream, &hello, SendFlags::empty()).expect("hello packet"),
            hello.len()
        );

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
            assert!(
                Instant::now() < deadline,
                "broker did not accept the authenticated shell handshake"
            );
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
        assert_eq!(
            send(&stream, &[DISPLAY1_PRESENT_ACK], SendFlags::empty())
                .expect("presentation acknowledgement"),
            1
        );
        while !server.first_frame_seen() {
            assert!(
                Instant::now() < deadline,
                "broker did not observe the KMS presentation acknowledgement"
            );
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
        let stream = seqpacket_connect(&socket_path).expect("connect broker");
        let hello = serde_json::to_vec(&Display1AttachHello::from_lease(&lease)).expect("hello");
        assert_eq!(
            send(&stream, &hello, SendFlags::empty()).expect("hello packet"),
            hello.len()
        );

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
            assert!(
                Instant::now() < deadline,
                "broker did not install the authenticated relay"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let peer = peer_credentials(&stream).expect("peer credentials");
        receive_display1_frame(&stream, peer, &lease, display1_now_ms()).expect("first frame");
        assert_eq!(
            send(&stream, &[DISPLAY1_PRESENT_ACK], SendFlags::empty())
                .expect("presentation acknowledgement"),
            1
        );
        while !server.first_frame_seen() {
            assert!(
                Instant::now() < deadline,
                "broker did not observe readiness before expiry"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        while socket_path.exists() || server.first_frame_seen() {
            assert!(
                Instant::now() < deadline,
                "expired broker did not revoke its socket and readiness"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
