//! WL-ARCH-010 — shell-side authenticated Display1 DMA-BUF client.
//!
//! The daemon's Display1 broker transfers frame metadata and one owned FD over
//! a node-local Unix socket. This module validates the lease and kernel peer,
//! then hands the descriptor directly to mde-egui's PRIME/KMS importer. No
//! frame bytes enter the Bus, JSON, or a CPU staging buffer.

use std::io::IoSliceMut;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::workloads::{
    WorkloadAttachmentLease, WorkloadAttachmentProtocol, WorkloadOperationPhase,
    WorkloadStateSnapshot,
};
use mde_bus::persist::Persist;
use mde_egui::drm::{self, Display1FramePoll, Display1FrameSource, ExternalDmaBufFrame};
use rustix::net::{
    connect_unix, recv, recvmsg, send, socket_with, AddressFamily, RecvAncillaryBuffer,
    RecvAncillaryMessage, RecvFlags, SendFlags, SocketAddrUnix, SocketFlags, SocketType,
};
use serde::{Deserialize, Serialize};

const MAX_ENVELOPE_BYTES: usize = 16 * 1024;
const DISPLAY1_HANDSHAKE_SCHEMA_VERSION: u16 = 1;
const MAX_HANDSHAKE_BYTES: usize = 4 * 1024;
const DISPLAY1_PRESENT_ACK: u8 = 0xA5;
// Linux's stable MSG_CTRUNC ABI bit. rustix 0.38 retains this result flag but
// does not publish a named RecvFlags constant for it.
const DISPLAY1_MSG_CTRUNC: u32 = 0x08;
/// Must match mackesd's node-local per-lease broker root. The socket path is
/// derived from the validated lease so a shell does not need an out-of-band
/// environment variable for the endpoint.
pub(crate) const DISPLAY1_SOCKET_ROOT: &str = "/run/mde/display1";

fn require_seqpacket(stream: &UnixStream) -> Result<(), Display1ClientError> {
    let socket_type = rustix::net::sockopt::get_socket_type(stream)
        .map_err(|error| Display1ClientError::Peer(format!("SO_TYPE: {error}")))?;
    if socket_type != SocketType::SEQPACKET {
        return Err(Display1ClientError::Protocol(
            "Display1 relay requires Unix SOCK_SEQPACKET".into(),
        ));
    }
    Ok(())
}

fn connect_seqpacket(path: &Path) -> Result<UnixStream, Display1ClientError> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|error| Display1ClientError::Peer(format!("create broker socket: {error}")))?;
    let address = SocketAddrUnix::new(path)
        .map_err(|error| Display1ClientError::Peer(format!("broker socket path: {error}")))?;
    connect_unix(&socket, &address)
        .map_err(|error| Display1ClientError::Peer(format!("connect broker: {error}")))?;
    Ok(socket.into())
}

fn reject_truncated_packet(flags: RecvFlags, context: &str) -> Result<(), Display1ClientError> {
    if flags.contains(RecvFlags::TRUNC) {
        return Err(Display1ClientError::Protocol(format!(
            "truncated Display1 {context} packet"
        )));
    }
    if flags.bits() & DISPLAY1_MSG_CTRUNC != 0 {
        return Err(Display1ClientError::Protocol(format!(
            "truncated Display1 {context} ancillary data"
        )));
    }
    Ok(())
}

/// Derive the broker endpoint for one validated attachment lease.
#[must_use]
pub(crate) fn socket_path_for_lease(lease_id: &str) -> Option<std::path::PathBuf> {
    if lease_id.is_empty()
        || lease_id.len() > 128
        || !lease_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    Some(std::path::PathBuf::from(DISPLAY1_SOCKET_ROOT).join(format!("{lease_id}.sock")))
}

/// Kernel-derived credentials for the daemon peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Display1PeerCredentials {
    pub(crate) pid: i32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

fn peer_credentials(stream: &UnixStream) -> Result<Display1PeerCredentials, Display1ClientError> {
    let credentials = rustix::net::sockopt::get_socket_peercred(stream)
        .map_err(|error| Display1ClientError::Peer(error.to_string()))?;
    Ok(Display1PeerCredentials {
        pid: rustix::process::Pid::as_raw(Some(credentials.pid)),
        uid: credentials.uid.as_raw(),
        gid: credentials.gid.as_raw(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameEnvelope {
    kind: String,
    lease_id: String,
    workload_id: String,
    generation: u64,
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    modifier: u64,
    y0_top: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DamageEnvelope {
    kind: String,
    lease_id: String,
    workload_id: String,
    generation: u64,
    damage: Display1Damage,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Display1Damage {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RelayEnvelope {
    Frame(FrameEnvelope),
    Damage(DamageEnvelope),
}

#[derive(Debug, Serialize)]
struct Display1AttachHello<'a> {
    schema_version: u16,
    lease_id: &'a str,
    nonce: &'a str,
    workload_id: &'a str,
    generation: u64,
}

enum ReceivedFrame {
    Idle,
    Disconnected,
    Frame(OwnedFd, ExternalDmaBufFrame),
    Damage(Display1Damage),
}

/// Errors before a frame reaches KMS.
#[derive(Debug)]
pub(crate) enum Display1ClientError {
    Peer(String),
    Lease(String),
    Protocol(String),
    Import(drm::DrmError),
}

impl std::fmt::Display for Display1ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer(error) => write!(f, "Display1 peer rejected: {error}"),
            Self::Lease(error) => write!(f, "Display1 lease rejected: {error}"),
            Self::Protocol(error) => write!(f, "Display1 protocol rejected: {error}"),
            Self::Import(error) => write!(f, "Display1 KMS import failed: {error}"),
        }
    }
}

impl std::error::Error for Display1ClientError {}

/// A lease-bound shell attachment. The expected peer is captured from the
/// authenticated broker handoff, never accepted from frame metadata.
#[derive(Debug)]
pub(crate) struct Display1Client {
    stream: UnixStream,
    peer: Display1PeerCredentials,
    lease: WorkloadAttachmentLease,
    frame_received: bool,
    first_present_acknowledged: bool,
    last_frame_size: Option<(u32, u32)>,
}

impl Display1Client {
    /// Connect to the node-local broker's per-attachment socket.  The broker
    /// runs as the privileged system daemon; accepting only a root peer keeps
    /// the lease/SCM_RIGHTS channel bound to that authority before any frame is
    /// read.  The caller still supplies the expiring, generation-bound lease.
    pub(crate) fn connect_privileged(
        path: &Path,
        lease: WorkloadAttachmentLease,
        now_ms: u64,
    ) -> Result<Self, Display1ClientError> {
        lease
            .validate(now_ms)
            .map_err(|error| Display1ClientError::Lease(error.to_string()))?;
        let stream = connect_seqpacket(path)?;
        let peer = peer_credentials(&stream)?;
        if peer.uid != 0 || peer.gid != 0 {
            return Err(Display1ClientError::Peer(
                "Display1 broker is not the privileged mackesd peer".into(),
            ));
        }
        send_handshake(&stream, &lease)?;
        Self::attach(stream, peer, lease, now_ms)
    }

    /// Bind one already-authorized stream to its kernel peer and lease.
    pub(crate) fn attach(
        stream: UnixStream,
        expected_peer: Display1PeerCredentials,
        lease: WorkloadAttachmentLease,
        now_ms: u64,
    ) -> Result<Self, Display1ClientError> {
        require_seqpacket(&stream)?;
        lease
            .validate(now_ms)
            .map_err(|error| Display1ClientError::Lease(error.to_string()))?;
        let peer = peer_credentials(&stream)?;
        if peer != expected_peer {
            return Err(Display1ClientError::Peer(
                "kernel credentials do not match the authorized broker".into(),
            ));
        }
        Ok(Self {
            stream,
            peer,
            lease,
            frame_received: false,
            first_present_acknowledged: false,
            last_frame_size: None,
        })
    }

    /// Notify the broker exactly once that the first imported frame completed
    /// its KMS modeset/page-flip. Socket delivery alone is not readiness.
    fn acknowledge_first_present(&mut self) -> Result<(), Display1ClientError> {
        if self.first_present_acknowledged {
            return Ok(());
        }
        if !self.frame_received {
            return Err(Display1ClientError::Protocol(
                "cannot acknowledge presentation before receiving a Display1 frame".into(),
            ));
        }
        let sent =
            send(&self.stream, &[DISPLAY1_PRESENT_ACK], SendFlags::DONTWAIT).map_err(|error| {
                Display1ClientError::Protocol(format!(
                    "send Display1 presentation acknowledgement: {error}"
                ))
            })?;
        if sent != 1 {
            return Err(Display1ClientError::Protocol(
                "short Display1 presentation acknowledgement".into(),
            ));
        }
        self.first_present_acknowledged = true;
        Ok(())
    }

    fn receive_inner(
        &mut self,
        now_ms: u64,
        flags: RecvFlags,
    ) -> Result<ReceivedFrame, Display1ClientError> {
        if peer_credentials(&self.stream)? != self.peer {
            return Err(Display1ClientError::Peer(
                "kernel credentials changed".into(),
            ));
        }
        self.lease
            .validate(now_ms)
            .map_err(|error| Display1ClientError::Lease(error.to_string()))?;
        let mut bytes = [0_u8; MAX_ENVELOPE_BYTES];
        let mut iov = [IoSliceMut::new(&mut bytes)];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(2))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        let received = match recvmsg(&self.stream, &mut iov, &mut ancillary, flags) {
            Ok(received) => received,
            Err(error)
                if flags.contains(RecvFlags::DONTWAIT) && error == rustix::io::Errno::AGAIN =>
            {
                return Ok(ReceivedFrame::Idle);
            }
            Err(error) => {
                return Err(Display1ClientError::Protocol(format!(
                    "SCM_RIGHTS receive: {error}"
                )))
            }
        };
        reject_truncated_packet(received.flags, "relay")?;
        let mut descriptor = None;
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(mut fds) = message {
                let first = fds.next();
                if descriptor.is_some() || fds.next().is_some() {
                    return Err(Display1ClientError::Protocol(
                        "Display1 message carried multiple descriptors".into(),
                    ));
                }
                descriptor = first;
            }
        }
        if received.bytes == 0 {
            if descriptor.is_some() {
                return Err(Display1ClientError::Protocol(
                    "empty Display1 packet carried a descriptor".into(),
                ));
            }
            let mut next = [0_u8; 1];
            return match recv(
                &self.stream,
                &mut next,
                RecvFlags::PEEK | RecvFlags::DONTWAIT,
            ) {
                Err(rustix::io::Errno::AGAIN) => Err(Display1ClientError::Protocol(
                    "empty Display1 relay packet".into(),
                )),
                Ok(0) => Ok(ReceivedFrame::Disconnected),
                Ok(_) => Err(Display1ClientError::Protocol(
                    "empty Display1 relay packet preceded another packet".into(),
                )),
                Err(error) => Err(Display1ClientError::Protocol(format!(
                    "classify empty Display1 packet: {error}"
                ))),
            };
        }
        let envelope: RelayEnvelope = serde_json::from_slice(&bytes[..received.bytes])
            .map_err(|error| Display1ClientError::Protocol(format!("relay envelope: {error}")))?;
        match envelope {
            RelayEnvelope::Frame(envelope) => {
                validate_envelope_binding(
                    &envelope.kind,
                    "frame",
                    &envelope.lease_id,
                    &envelope.workload_id,
                    envelope.generation,
                    &self.lease,
                )?;
                if !envelope.y0_top {
                    return Err(Display1ClientError::Protocol(
                        "bottom-up scanout is unsupported by the native KMS path".into(),
                    ));
                }
                let frame = ExternalDmaBufFrame {
                    width: envelope.width,
                    height: envelope.height,
                    stride: envelope.stride,
                    fourcc: envelope.fourcc,
                    modifier: envelope.modifier,
                };
                frame.validate().map_err(Display1ClientError::Import)?;
                let descriptor = descriptor
                    .ok_or_else(|| Display1ClientError::Protocol("missing DMA-BUF FD".into()))?;
                self.frame_received = true;
                self.last_frame_size = Some((frame.width, frame.height));
                Ok(ReceivedFrame::Frame(descriptor, frame))
            }
            RelayEnvelope::Damage(envelope) => {
                validate_envelope_binding(
                    &envelope.kind,
                    "damage",
                    &envelope.lease_id,
                    &envelope.workload_id,
                    envelope.generation,
                    &self.lease,
                )?;
                if descriptor.is_some() {
                    return Err(Display1ClientError::Protocol(
                        "Display1 damage must not carry a descriptor".into(),
                    ));
                }
                let (width, height) = self.last_frame_size.ok_or_else(|| {
                    Display1ClientError::Protocol(
                        "Display1 damage arrived before an accepted frame".into(),
                    )
                })?;
                validate_damage(envelope.damage, width, height)?;
                Ok(ReceivedFrame::Damage(envelope.damage))
            }
        }
    }

    /// Receive and validate one metadata+SCM_RIGHTS frame. This compatibility
    /// helper is intentionally bounded to one frame; the live DRM loop uses
    /// [`Self::try_receive`] so it never blocks the render thread.
    pub(crate) fn receive(
        &mut self,
        now_ms: u64,
    ) -> Result<(OwnedFd, ExternalDmaBufFrame), Display1ClientError> {
        match self.receive_inner(now_ms, RecvFlags::empty())? {
            ReceivedFrame::Frame(fd, frame) => Ok((fd, frame)),
            ReceivedFrame::Idle => Err(Display1ClientError::Protocol(
                "unexpected non-blocking idle result".into(),
            )),
            ReceivedFrame::Disconnected => Err(Display1ClientError::Protocol(
                "Display1 peer disconnected".into(),
            )),
            ReceivedFrame::Damage(_) => Err(Display1ClientError::Protocol(
                "unexpected damage while waiting for a complete frame".into(),
            )),
        }
    }

    /// Poll one frame without blocking the direct-DRM render loop.
    pub(crate) fn try_receive(
        &mut self,
        now_ms: u64,
    ) -> Result<Display1FramePoll, Display1ClientError> {
        Ok(match self.receive_inner(now_ms, RecvFlags::DONTWAIT)? {
            ReceivedFrame::Idle => Display1FramePoll::Idle,
            ReceivedFrame::Disconnected => Display1FramePoll::Disconnected,
            ReceivedFrame::Frame(fd, metadata) => Display1FramePoll::Frame { fd, metadata },
            ReceivedFrame::Damage(damage) => Display1FramePoll::Damage {
                x: damage.x,
                y: damage.y,
                width: damage.width,
                height: damage.height,
            },
        })
    }
}

fn validate_envelope_binding(
    actual_kind: &str,
    expected_kind: &str,
    lease_id: &str,
    workload_id: &str,
    generation: u64,
    lease: &WorkloadAttachmentLease,
) -> Result<(), Display1ClientError> {
    if actual_kind != expected_kind {
        return Err(Display1ClientError::Protocol(
            "unknown Display1 relay message kind".into(),
        ));
    }
    if lease_id != lease.lease_id
        || workload_id != lease.workload_id.as_str()
        || generation != lease.generation
    {
        return Err(Display1ClientError::Lease(
            "relay lease or generation mismatch".into(),
        ));
    }
    Ok(())
}

fn validate_damage(
    damage: Display1Damage,
    scanout_width: u32,
    scanout_height: u32,
) -> Result<(), Display1ClientError> {
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
        return Err(Display1ClientError::Protocol(
            "Display1 damage is outside the retained frame".into(),
        ));
    }
    Ok(())
}

/// A display source that can receive a lease after the shell has already
/// entered its direct-DRM loop. The discovery and blocking Unix connect happen
/// on a worker thread; the render thread only performs bounded channel and
/// non-blocking socket operations.
pub(crate) enum Display1Source {
    Static(Display1Client),
    Dynamic(DynamicDisplay1Client),
}

impl Display1Source {
    pub(crate) fn dynamic(
        node: String,
        bus_root: Option<PathBuf>,
        workload_id: Option<String>,
    ) -> Self {
        Self::Dynamic(DynamicDisplay1Client::spawn(node, bus_root, workload_id))
    }
}

impl Display1FrameSource for Display1Source {
    fn poll(&mut self, now: Instant) -> Result<Display1FramePoll, drm::DrmError> {
        match self {
            Self::Static(client) => client.poll(now),
            Self::Dynamic(client) => client.poll(now),
        }
    }

    fn frame_presented(&mut self) -> Result<(), drm::DrmError> {
        match self {
            Self::Static(client) => client.frame_presented(),
            Self::Dynamic(client) => client.frame_presented(),
        }
    }
}

/// Background Workload projection watcher and attachment connector.
pub(crate) struct DynamicDisplay1Client {
    incoming: Receiver<Display1Client>,
    reconnect: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    client: Option<Display1Client>,
    thread: Option<JoinHandle<()>>,
}

impl DynamicDisplay1Client {
    fn spawn(node: String, bus_root: Option<PathBuf>, workload_id: Option<String>) -> Self {
        let (sender, incoming) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let reconnect = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_reconnect = Arc::clone(&reconnect);
        let thread = thread::Builder::new()
            .name("display1-discovery".into())
            .spawn(move || {
                discover_and_connect(
                    node,
                    bus_root,
                    workload_id,
                    sender,
                    thread_stop,
                    thread_reconnect,
                );
            })
            .ok();
        Self {
            incoming,
            reconnect,
            stop,
            client: None,
            thread,
        }
    }

    fn poll(&mut self, now: Instant) -> Result<Display1FramePoll, drm::DrmError> {
        if self.client.is_none() {
            match self.incoming.try_recv() {
                Ok(client) => self.client = Some(client),
                Err(TryRecvError::Empty) => return Ok(Display1FramePoll::Idle),
                Err(TryRecvError::Disconnected) => return Ok(Display1FramePoll::Disconnected),
            }
        }
        let Some(client) = self.client.as_mut() else {
            return Ok(Display1FramePoll::Idle);
        };
        match client.poll(now) {
            Ok(frame @ (Display1FramePoll::Frame { .. } | Display1FramePoll::Damage { .. })) => {
                Ok(frame)
            }
            Ok(Display1FramePoll::Idle) => Ok(Display1FramePoll::Idle),
            Ok(Display1FramePoll::Disconnected) => {
                self.client = None;
                self.reconnect.store(true, Ordering::Release);
                Ok(Display1FramePoll::Disconnected)
            }
            Err(error) => {
                self.client = None;
                self.reconnect.store(true, Ordering::Release);
                Err(error)
            }
        }
    }

    fn frame_presented(&mut self) -> Result<(), drm::DrmError> {
        self.client
            .as_mut()
            .ok_or_else(|| {
                drm::DrmError::Present(
                    "Display1 client disconnected before presentation acknowledgement".into(),
                )
            })?
            .frame_presented()
    }
}

impl Drop for DynamicDisplay1Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn discover_and_connect(
    node: String,
    bus_root: Option<PathBuf>,
    workload_id: Option<String>,
    sender: SyncSender<Display1Client>,
    stop: Arc<AtomicBool>,
    reconnect: Arc<AtomicBool>,
) {
    let mut offered_lease: Option<String> = None;
    let mut pending: Option<(String, Display1Client)> = None;
    while !stop.load(Ordering::Acquire) {
        if reconnect.swap(false, Ordering::AcqRel) {
            offered_lease = None;
        }
        if pending.is_none() && offered_lease.is_none() {
            if let Some(lease) =
                discover_display1_lease(bus_root.as_deref(), &node, workload_id.as_deref())
            {
                if let Some(socket) = socket_path_for_lease(&lease.lease_id) {
                    let now_ms = current_ms();
                    match Display1Client::connect_privileged(&socket, lease.clone(), now_ms) {
                        Ok(client) => pending = Some((lease.lease_id, client)),
                        Err(error) => {
                            tracing::debug!(target: "shell::display1", %error, "Display1 lease discovered but broker is not ready")
                        }
                    }
                }
            }
        }
        if let Some((lease_id, client)) = pending.take() {
            match sender.try_send(client) {
                Ok(()) => offered_lease = Some(lease_id),
                Err(TrySendError::Full(client)) => pending = Some((lease_id, client)),
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn discover_display1_lease(
    bus_root: Option<&Path>,
    node: &str,
    workload_id: Option<&str>,
) -> Option<WorkloadAttachmentLease> {
    let root = bus_root?;
    let persist = Persist::open(root.to_path_buf()).ok()?;
    let snapshot: WorkloadStateSnapshot = crate::workload_api::read_state(&persist, node)?;
    snapshot
        .workloads
        .into_iter()
        .filter(|status| workload_id.is_none_or(|wanted| status.workload_id.as_str() == wanted))
        .filter(|status| {
            !matches!(
                status.phase,
                WorkloadOperationPhase::Failed | WorkloadOperationPhase::Cancelled
            )
        })
        .filter_map(|status| status.attachment)
        .filter(|lease| lease.protocol == WorkloadAttachmentProtocol::QemuDisplay1Dmabuf)
        .max_by_key(|lease| lease.generation)
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn send_handshake(
    stream: &UnixStream,
    lease: &WorkloadAttachmentLease,
) -> Result<(), Display1ClientError> {
    require_seqpacket(stream)?;
    let body = serde_json::to_vec(&Display1AttachHello {
        schema_version: DISPLAY1_HANDSHAKE_SCHEMA_VERSION,
        lease_id: lease.lease_id.as_str(),
        nonce: lease.nonce.as_str(),
        workload_id: lease.workload_id.as_str(),
        generation: lease.generation,
    })
    .map_err(|error| {
        Display1ClientError::Protocol(format!("Display1 handshake encode: {error}"))
    })?;
    if body.is_empty() || body.len() > MAX_HANDSHAKE_BYTES {
        return Err(Display1ClientError::Protocol(
            "Display1 handshake exceeds the bounded size".into(),
        ));
    }
    let sent = send(stream, &body, SendFlags::empty())
        .map_err(|error| Display1ClientError::Peer(format!("Display1 handshake send: {error}")))?;
    if sent != body.len() {
        return Err(Display1ClientError::Protocol(
            "short Display1 handshake packet".into(),
        ));
    }
    Ok(())
}

impl Display1FrameSource for Display1Client {
    fn poll(&mut self, _now: Instant) -> Result<Display1FramePoll, drm::DrmError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        self.try_receive(now_ms)
            .map_err(|error| drm::DrmError::Present(error.to_string()))
    }

    fn frame_presented(&mut self) -> Result<(), drm::DrmError> {
        self.acknowledge_first_present()
            .map_err(|error| drm::DrmError::Present(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{
        WorkloadAttachmentProtocol, WorkloadId, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    };
    use rustix::net::{sendmsg, SendAncillaryBuffer, SendAncillaryMessage};
    use std::io::{IoSlice, Read};
    use std::os::fd::AsFd;

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

    #[test]
    fn lease_id_derives_the_default_node_local_socket() {
        assert_eq!(
            socket_path_for_lease("lease-client"),
            Some(std::path::PathBuf::from(
                "/run/mde/display1/lease-client.sock",
            ))
        );
        assert!(socket_path_for_lease("../escape").is_none());
    }

    #[test]
    fn lease_and_peer_are_required_before_frame_import() {
        let (left, right) = seqpacket_pair();
        let peer = peer_credentials(&left).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-client".into(),
            nonce: "nonce-client".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("workload id"),
            generation: 1,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 2_000,
        };
        let mut client = Display1Client::attach(right, peer, lease.clone(), 1_000).expect("attach");
        assert!(matches!(
            client.receive(2_001),
            Err(Display1ClientError::Lease(_))
        ));
        drop(left);
    }

    #[test]
    fn nonblocking_poll_reports_idle_then_disconnects_without_waiting() {
        let (left, right) = seqpacket_pair();
        let peer = peer_credentials(&left).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-poll".into(),
            nonce: "nonce-poll".into(),
            workload_id: WorkloadId::new("browser-poll").expect("workload id"),
            generation: 1,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 20_000,
        };
        let mut client = Display1Client::attach(right, peer, lease, 1_000).expect("attach");
        assert!(matches!(
            client.try_receive(1_001),
            Ok(Display1FramePoll::Idle)
        ));
        drop(left);
        assert!(matches!(
            client.try_receive(1_002),
            Ok(Display1FramePoll::Disconnected)
        ));
    }

    #[test]
    fn zero_length_packet_is_rejected_while_orderly_disconnect_is_distinct() {
        let (daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-empty-packet".into(),
            nonce: "nonce-empty-packet".into(),
            workload_id: WorkloadId::new("browser-empty-packet").expect("workload id"),
            generation: 1,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 20_000,
        };
        let mut client = Display1Client::attach(shell, peer, lease, 1_000).expect("attach");
        assert_eq!(
            send(&daemon, &[], SendFlags::empty()).expect("send empty packet"),
            0
        );
        assert!(matches!(
            client.try_receive(1_001),
            Err(Display1ClientError::Protocol(message)) if message.contains("empty")
        ));
        drop(daemon);
        assert!(matches!(
            client.try_receive(1_002),
            Ok(Display1FramePoll::Disconnected)
        ));
    }

    #[test]
    fn rapid_frame_and_damage_packets_preserve_boundaries_and_descriptor_binding() {
        let (daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-packet-boundaries".into(),
            nonce: "nonce-packet-boundaries".into(),
            workload_id: WorkloadId::new("browser-packet-boundaries").expect("workload id"),
            generation: 9,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 20_000,
        };
        let mut client = Display1Client::attach(shell, peer, lease, 1_000).expect("attach");
        let frame = serde_json::to_vec(&serde_json::json!({
            "kind": "frame",
            "lease_id": "lease-packet-boundaries",
            "workload_id": "browser-packet-boundaries",
            "generation": 9,
            "width": 64,
            "height": 32,
            "stride": 256,
            "fourcc": 0x3432_5258_u32,
            "modifier": 0,
            "y0_top": true
        }))
        .expect("frame envelope");
        let dmabuf = std::fs::File::open("/dev/null").expect("DMA-BUF fixture");
        let descriptors = [dmabuf.as_fd()];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            sendmsg(
                &daemon,
                &[IoSlice::new(&frame)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .expect("send frame packet"),
            frame.len()
        );
        for (x, y) in [(1_u32, 2_u32), (3, 4)] {
            let damage = serde_json::to_vec(&serde_json::json!({
                "kind": "damage",
                "lease_id": "lease-packet-boundaries",
                "workload_id": "browser-packet-boundaries",
                "generation": 9,
                "damage": {"x": x, "y": y, "width": 8, "height": 6}
            }))
            .expect("damage envelope");
            assert_eq!(
                send(&daemon, &damage, SendFlags::empty()).expect("send damage packet"),
                damage.len()
            );
        }

        assert!(matches!(
            client.try_receive(1_001),
            Ok(Display1FramePoll::Frame { .. })
        ));
        assert!(matches!(
            client.try_receive(1_002),
            Ok(Display1FramePoll::Damage { x: 1, y: 2, .. })
        ));
        assert!(matches!(
            client.try_receive(1_003),
            Ok(Display1FramePoll::Damage { x: 3, y: 4, .. })
        ));
        assert!(matches!(
            client.try_receive(1_004),
            Ok(Display1FramePoll::Idle)
        ));
    }

    #[test]
    fn descriptor_cannot_cross_packet_boundaries_or_attach_to_damage() {
        let (daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-descriptor-boundary".into(),
            nonce: "nonce-descriptor-boundary".into(),
            workload_id: WorkloadId::new("browser-descriptor-boundary").expect("workload id"),
            generation: 5,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 20_000,
        };
        let mut client = Display1Client::attach(shell, peer, lease, 1_000).expect("attach");
        let frame_without_fd = serde_json::to_vec(&serde_json::json!({
            "kind": "frame",
            "lease_id": "lease-descriptor-boundary",
            "workload_id": "browser-descriptor-boundary",
            "generation": 5,
            "width": 64,
            "height": 32,
            "stride": 256,
            "fourcc": 0x3432_5258_u32,
            "modifier": 0,
            "y0_top": true
        }))
        .expect("frame envelope");
        assert_eq!(
            send(&daemon, &frame_without_fd, SendFlags::empty()).expect("send frame without fd"),
            frame_without_fd.len()
        );
        let damage_with_fd = serde_json::to_vec(&serde_json::json!({
            "kind": "damage",
            "lease_id": "lease-descriptor-boundary",
            "workload_id": "browser-descriptor-boundary",
            "generation": 5,
            "damage": {"x": 1, "y": 2, "width": 8, "height": 6}
        }))
        .expect("damage envelope");
        let descriptor = std::fs::File::open("/dev/null").expect("descriptor fixture");
        let descriptors = [descriptor.as_fd()];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            sendmsg(
                &daemon,
                &[IoSlice::new(&damage_with_fd)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .expect("send damage with fd"),
            damage_with_fd.len()
        );

        assert!(matches!(
            client.try_receive(1_001),
            Err(Display1ClientError::Protocol(message)) if message.contains("missing DMA-BUF")
        ));
        assert!(matches!(
            client.try_receive(1_002),
            Err(Display1ClientError::Protocol(message)) if message.contains("must not carry")
        ));

        let descriptors = [descriptor.as_fd(); 16];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(16))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            sendmsg(
                &daemon,
                &[IoSlice::new(&damage_with_fd)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .expect("send ancillary-overflow packet"),
            damage_with_fd.len()
        );
        assert!(matches!(
            client.try_receive(1_003),
            Err(Display1ClientError::Protocol(message)) if message.contains("ancillary")
        ));
    }

    #[test]
    fn oversized_packet_is_rejected_instead_of_partially_parsed() {
        let (daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-truncated-packet".into(),
            nonce: "nonce-truncated-packet".into(),
            workload_id: WorkloadId::new("browser-truncated-packet").expect("workload id"),
            generation: 2,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: 20_000,
        };
        let mut client = Display1Client::attach(shell, peer, lease, 1_000).expect("attach");
        let oversized = vec![b'X'; MAX_ENVELOPE_BYTES + 1];
        assert_eq!(
            send(&daemon, &oversized, SendFlags::empty()).expect("send oversized packet"),
            oversized.len()
        );
        assert!(matches!(
            client.try_receive(1_001),
            Err(Display1ClientError::Protocol(message)) if message.contains("truncated")
        ));
    }

    #[test]
    fn presentation_acknowledgement_is_emitted_once_after_kms_success() {
        let (mut daemon, shell) = seqpacket_pair();
        let peer = peer_credentials(&daemon).expect("peer");
        let lease = WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: "lease-presented".into(),
            nonce: "nonce-presented".into(),
            workload_id: WorkloadId::new("browser-presented").expect("workload id"),
            generation: 3,
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: u64::MAX,
        };
        let mut client = Display1Client::attach(shell, peer, lease, 1_000).expect("attach");

        assert!(matches!(
            client.frame_presented(),
            Err(drm::DrmError::Present(message)) if message.contains("before receiving")
        ));

        let envelope = serde_json::to_vec(&serde_json::json!({
            "kind": "frame",
            "lease_id": "lease-presented",
            "workload_id": "browser-presented",
            "generation": 3,
            "width": 64,
            "height": 32,
            "stride": 256,
            "fourcc": 0x3432_5258_u32,
            "modifier": 0,
            "y0_top": true
        }))
        .expect("frame envelope");
        let dmabuf = std::fs::File::open("/dev/null").expect("DMA-BUF fixture");
        let descriptors = [dmabuf.as_fd()];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            sendmsg(
                &daemon,
                &[IoSlice::new(&envelope)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .expect("send frame"),
            envelope.len()
        );
        assert!(matches!(
            client.try_receive(1_001),
            Ok(Display1FramePoll::Frame { .. })
        ));

        client.frame_presented().expect("first KMS acknowledgement");
        let mut ack = [0_u8; 1];
        daemon.read_exact(&mut ack).expect("daemon receives ack");
        assert_eq!(ack, [DISPLAY1_PRESENT_ACK]);

        let damage = serde_json::to_vec(&serde_json::json!({
            "kind": "damage",
            "lease_id": "lease-presented",
            "workload_id": "browser-presented",
            "generation": 3,
            "damage": {"x": 4, "y": 5, "width": 16, "height": 8}
        }))
        .expect("damage envelope");
        assert_eq!(
            send(&daemon, &damage, SendFlags::empty()).expect("send damage"),
            damage.len()
        );
        assert!(matches!(
            client.try_receive(1_002),
            Ok(Display1FramePoll::Damage {
                x: 4,
                y: 5,
                width: 16,
                height: 8
            })
        ));
        let hostile_damage = serde_json::to_vec(&serde_json::json!({
            "kind": "damage",
            "lease_id": "lease-presented",
            "workload_id": "browser-presented",
            "generation": 3,
            "damage": {"x": 60, "y": 0, "width": 8, "height": 1}
        }))
        .expect("hostile damage envelope");
        assert_eq!(
            send(&daemon, &hostile_damage, SendFlags::empty()).expect("send hostile damage"),
            hostile_damage.len()
        );
        assert!(matches!(
            client.try_receive(1_003),
            Err(Display1ClientError::Protocol(message))
                if message.contains("outside the retained frame")
        ));

        client
            .frame_presented()
            .expect("repeated frame is idempotent");
        daemon.set_nonblocking(true).expect("nonblocking daemon");
        assert!(matches!(
            daemon.read(&mut ack),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn dynamic_source_without_a_bus_stays_idle_and_is_bounded() {
        let mut source = DynamicDisplay1Client::spawn("seat15".into(), None, None);
        assert!(matches!(
            source.poll(Instant::now()),
            Ok(Display1FramePoll::Idle)
        ));
    }
}
