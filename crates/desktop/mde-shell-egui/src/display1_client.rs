//! WL-ARCH-010 — shell-side authenticated Display1 DMA-BUF client.
//!
//! The daemon's Display1 broker transfers frame metadata and one owned FD over
//! a node-local Unix socket. This module validates the lease and kernel peer,
//! then hands the descriptor directly to mde-egui's PRIME/KMS importer. No
//! frame bytes enter the Bus, JSON, or a CPU staging buffer.

use std::io::{IoSliceMut, Write};
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
use mde_egui::drm::{self, Display1FramePoll, Display1FrameSource, ExternalDmaBufFrame};
use mde_bus::persist::Persist;
use rustix::net::{recvmsg, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use serde::{Deserialize, Serialize};

const MAX_ENVELOPE_BYTES: usize = 16 * 1024;
const DISPLAY1_HANDSHAKE_SCHEMA_VERSION: u16 = 1;
const MAX_HANDSHAKE_BYTES: usize = 4 * 1024;
/// Must match mackesd's node-local per-lease broker root. The socket path is
/// derived from the validated lease so a shell does not need an out-of-band
/// environment variable for the endpoint.
pub(crate) const DISPLAY1_SOCKET_ROOT: &str = "/run/mde/display1";

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
        let stream = UnixStream::connect(path)
            .map_err(|error| Display1ClientError::Peer(format!("connect broker: {error}")))?;
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
        })
    }

    fn receive_inner(
        &self,
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
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
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
        if received.bytes == 0 {
            return Ok(ReceivedFrame::Disconnected);
        }
        let envelope: FrameEnvelope = serde_json::from_slice(&bytes[..received.bytes])
            .map_err(|error| Display1ClientError::Protocol(format!("frame envelope: {error}")))?;
        if envelope.lease_id != self.lease.lease_id
            || envelope.workload_id != self.lease.workload_id.as_str()
            || envelope.generation != self.lease.generation
        {
            return Err(Display1ClientError::Lease(
                "frame lease or generation mismatch".into(),
            ));
        }
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
        let mut descriptor = None;
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(mut fds) = message {
                descriptor = fds.next();
                break;
            }
        }
        let descriptor =
            descriptor.ok_or_else(|| Display1ClientError::Protocol("missing DMA-BUF FD".into()))?;
        Ok(ReceivedFrame::Frame(descriptor, frame))
    }

    /// Receive and validate one metadata+SCM_RIGHTS frame. This compatibility
    /// helper is intentionally bounded to one frame; the live DRM loop uses
    /// [`Self::try_receive`] so it never blocks the render thread.
    pub(crate) fn receive(
        &self,
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
        }
    }

    /// Poll one frame without blocking the direct-DRM render loop.
    pub(crate) fn try_receive(
        &self,
        now_ms: u64,
    ) -> Result<Display1FramePoll, Display1ClientError> {
        Ok(match self.receive_inner(now_ms, RecvFlags::DONTWAIT)? {
            ReceivedFrame::Idle => Display1FramePoll::Idle,
            ReceivedFrame::Disconnected => Display1FramePoll::Disconnected,
            ReceivedFrame::Frame(fd, metadata) => Display1FramePoll::Frame { fd, metadata },
        })
    }
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
    pub(crate) fn dynamic(node: String, bus_root: Option<PathBuf>, workload_id: Option<String>) -> Self {
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
            Ok(frame @ Display1FramePoll::Frame { .. }) => Ok(frame),
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
            if let Some(lease) = discover_display1_lease(bus_root.as_deref(), &node, workload_id.as_deref()) {
                if let Some(socket) = socket_path_for_lease(&lease.lease_id) {
                    let now_ms = current_ms();
                    match Display1Client::connect_privileged(&socket, lease.clone(), now_ms) {
                        Ok(client) => pending = Some((lease.lease_id, client)),
                        Err(error) => tracing::debug!(target: "shell::display1", %error, "Display1 lease discovered but broker is not ready"),
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
        .filter(|status| {
            workload_id.is_none_or(|wanted| status.workload_id.as_str() == wanted)
        })
        .filter(|status| {
            !matches!(status.phase, WorkloadOperationPhase::Failed | WorkloadOperationPhase::Cancelled)
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

fn send_handshake(stream: &UnixStream, lease: &WorkloadAttachmentLease) -> Result<(), Display1ClientError> {
    let body = serde_json::to_vec(&Display1AttachHello {
        schema_version: DISPLAY1_HANDSHAKE_SCHEMA_VERSION,
        lease_id: lease.lease_id.as_str(),
        nonce: lease.nonce.as_str(),
        workload_id: lease.workload_id.as_str(),
        generation: lease.generation,
    })
    .map_err(|error| Display1ClientError::Protocol(format!("Display1 handshake encode: {error}")))?;
    if body.is_empty() || body.len() > MAX_HANDSHAKE_BYTES {
        return Err(Display1ClientError::Protocol(
            "Display1 handshake exceeds the bounded size".into(),
        ));
    }
    let length = u32::try_from(body.len())
        .map_err(|_| Display1ClientError::Protocol("Display1 handshake is too large".into()))?;
    let mut bytes = length.to_be_bytes().to_vec();
    bytes.extend_from_slice(&body);
    (&*stream)
        .write_all(&bytes)
        .map_err(|error| Display1ClientError::Peer(format!("Display1 handshake send: {error}")))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{
        WorkloadAttachmentProtocol, WorkloadId, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    };

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
        let (left, right) = UnixStream::pair().expect("socketpair");
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
        let client = Display1Client::attach(right, peer, lease.clone(), 1_000).expect("attach");
        assert!(matches!(
            client.receive(2_001),
            Err(Display1ClientError::Lease(_))
        ));
        drop(left);
    }

    #[test]
    fn nonblocking_poll_reports_idle_then_disconnects_without_waiting() {
        let (left, right) = UnixStream::pair().expect("socketpair");
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
        let client = Display1Client::attach(right, peer, lease, 1_000).expect("attach");
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
    fn dynamic_source_without_a_bus_stays_idle_and_is_bounded() {
        let mut source = DynamicDisplay1Client::spawn("seat15".into(), None, None);
        assert!(matches!(
            source.poll(Instant::now()),
            Ok(Display1FramePoll::Idle)
        ));
    }
}
