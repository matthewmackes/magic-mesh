//! Root-local, descriptor-backed VDI clipboard image materializer.
//!
//! The endpoint reuses the Transfers worker's one Files resolver. It validates
//! the current typed clipboard command and lease from the Bus, resolves the
//! opaque Files identity inside the daemon, and transfers only a verified
//! read-only descriptor over a peer-credential-checked local socket.

use std::collections::BTreeMap;
use std::io::IoSlice;
use std::os::fd::AsFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mackes_mesh_types::vdi_clipboard::{
    vdi_clipboard_session_topic, VdiClipboardFilesMaterializationErrorV1,
    VdiClipboardFilesMaterializationRequestV1, VdiClipboardFilesMaterializationResponseV1,
    VdiClipboardLeaseV2, VdiClipboardMessageV2, VdiClipboardReceiptV2,
    MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES,
    MAX_VDI_CLIPBOARD_TRANSPORT_V2_JSON_BYTES, VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET,
    VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX, VDI_CLIPBOARD_LEASE_TOPIC_PREFIX,
    VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX,
};
use mde_bus::persist::Persist;
use mde_collab_types::{FileRefId, TransferLocation};
use rustix::net::{
    accept_with, bind_unix, connect_unix, listen, recv, send, sendmsg, socket_with, AddressFamily,
    RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketFlags,
    SocketType,
};

use super::v2::open_bounded_files_source;
use super::FilesEndpointResolver;

const MAX_ONE_USE_RECORDS: usize = 1_024;
const MAX_PENDING_CLIENTS: usize = 8;
const PENDING_CLIENT_TTL_MS: u64 = 1_000;
const FILES_REFERENCE_PREFIX: &str = "files:v2:";

type CommandKey = (String, u64, String, u64);

struct PendingClient {
    stream: UnixStream,
    expires_at_ms: u64,
}

pub(super) struct ClipboardFilesMaterializer {
    listener: UnixStream,
    socket_path: PathBuf,
    bus_root: PathBuf,
    resolver: Arc<dyn FilesEndpointResolver>,
    expected_uid: u32,
    authorizations: BTreeMap<String, u64>,
    commands: BTreeMap<CommandKey, u64>,
    pending: Vec<PendingClient>,
}

impl ClipboardFilesMaterializer {
    pub(super) fn bind(
        bus_root: PathBuf,
        resolver: Arc<dyn FilesEndpointResolver>,
    ) -> std::io::Result<Self> {
        let socket_path = bus_root.join(VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET);
        if socket_path.exists() {
            match seqpacket_connect(&socket_path) {
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        "VDI clipboard Files materializer is already active",
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(&socket_path)?
                }
                Err(error) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!(
                            "existing clipboard materializer endpoint is not replaceable: {error}"
                        ),
                    ))
                }
            }
        }
        let listener = seqpacket_listener(&socket_path)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            socket_path,
            bus_root,
            resolver,
            expected_uid: rustix::process::getuid().as_raw(),
            authorizations: BTreeMap::new(),
            commands: BTreeMap::new(),
            pending: Vec::new(),
        })
    }

    /// Poll at most eight nonblocking clients per worker tick so a same-uid
    /// peer that connects without sending cannot stall the transfer queue.
    pub(super) fn drain(&mut self, now_ms: u64) {
        self.authorizations.retain(|_, expiry| *expiry > now_ms);
        self.commands.retain(|_, expiry| *expiry > now_ms);
        self.pending.retain(|client| client.expires_at_ms > now_ms);
        while self.pending.len() < MAX_PENDING_CLIENTS {
            let stream = match accept_with(
                &self.listener,
                SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            ) {
                Ok(stream) => stream,
                Err(error) if error == rustix::io::Errno::AGAIN => break,
                Err(error) => {
                    tracing::warn!(target: "mackesd::transfers", %error, "clipboard materializer accept failed");
                    break;
                }
            };
            let stream: UnixStream = stream.into();
            self.pending.push(PendingClient {
                stream,
                expires_at_ms: now_ms.saturating_add(PENDING_CLIENT_TTL_MS),
            });
        }

        let clients = std::mem::take(&mut self.pending);
        for client in clients {
            if !self.serve_one(&client.stream, now_ms) && client.expires_at_ms > now_ms {
                self.pending.push(client);
            }
        }
    }

    /// `true` means the client is complete and may be dropped; `false` keeps a
    /// not-yet-readable client for a later tick, bounded by its one-second TTL.
    fn serve_one(&mut self, stream: &UnixStream, now_ms: u64) -> bool {
        let peer = match rustix::net::sockopt::get_socket_peercred(stream) {
            Ok(peer) if peer.uid.as_raw() == self.expected_uid => peer,
            _ => return true,
        };
        let _ = peer;
        let request = match receive_request(stream) {
            Ok(Some(request)) => request,
            Ok(None) => return false,
            Err(reason) => {
                let _ = send_refusal(stream, String::new(), reason);
                return true;
            }
        };
        let authorization_id = request.authorization_id.clone();
        match self.materialize(&request, now_ms) {
            Ok(file) => {
                let response = VdiClipboardFilesMaterializationResponseV1::Ready {
                    authorization_id,
                    selected_mime: request.selected_mime.clone(),
                    content_hash: request.content_hash.clone(),
                    byte_count: request.byte_count,
                };
                let _ = send_response(stream, &response, Some(file.as_fd()));
            }
            Err(reason) => {
                let _ = send_refusal(stream, authorization_id, reason);
            }
        }
        true
    }

    fn materialize(
        &mut self,
        request: &VdiClipboardFilesMaterializationRequestV1,
        now_ms: u64,
    ) -> Result<std::fs::File, VdiClipboardFilesMaterializationErrorV1> {
        request.validate()?;
        if now_ms >= request.lease_expires_at_ms || now_ms >= request.envelope_expires_at_ms {
            return Err(VdiClipboardFilesMaterializationErrorV1::Expired);
        }
        let command_key = (
            request.session_id.clone(),
            request.generation,
            request.lease_id.clone(),
            request.message_sequence,
        );
        if self.authorizations.contains_key(&request.authorization_id)
            || self.commands.contains_key(&command_key)
        {
            return Err(VdiClipboardFilesMaterializationErrorV1::Replayed);
        }
        if self.authorizations.len() >= MAX_ONE_USE_RECORDS
            || self.commands.len() >= MAX_ONE_USE_RECORDS
        {
            return Err(VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable);
        }

        let persist = Persist::open(self.bus_root.clone())
            .map_err(|_| VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)?;
        let lease: VdiClipboardLeaseV2 = read_typed_latest(
            &persist,
            &vdi_clipboard_session_topic(VDI_CLIPBOARD_LEASE_TOPIC_PREFIX, &request.session_id)
                .map_err(|_| VdiClipboardFilesMaterializationErrorV1::InvalidIdentity)?,
        )?;
        let command_topic = vdi_clipboard_session_topic(
            VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
            &request.session_id,
        )
        .map_err(|_| VdiClipboardFilesMaterializationErrorV1::InvalidIdentity)?;
        let command_body = read_latest_body(&persist, &command_topic)?;
        let command = VdiClipboardMessageV2::from_json_bytes(command_body.as_bytes())
            .map_err(|_| VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)?;
        let receipt_topic =
            vdi_clipboard_session_topic(VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX, &request.session_id)
                .map_err(|_| VdiClipboardFilesMaterializationErrorV1::InvalidIdentity)?;
        let receipt =
            read_optional_typed_latest::<VdiClipboardReceiptV2>(&persist, &receipt_topic)?;
        command
            .admit(&lease, receipt.as_ref(), now_ms)
            .map_err(|_| VdiClipboardFilesMaterializationErrorV1::LeaseMismatch)?;
        request.validate_against(&command, &lease, now_ms)?;

        // The trusted root shell sends this request only after its one-use
        // permission CAS. Consume both nonce and exact command before Files I/O;
        // failure cannot turn the same approval into another read attempt.
        self.authorizations.insert(
            request.authorization_id.clone(),
            request
                .lease_expires_at_ms
                .min(request.envelope_expires_at_ms),
        );
        self.commands.insert(
            command_key,
            request
                .lease_expires_at_ms
                .min(request.envelope_expires_at_ms),
        );

        let object = request
            .files_reference
            .strip_prefix(FILES_REFERENCE_PREFIX)
            .and_then(|value| value.parse::<FileRefId>().ok())
            .filter(|object| !object.is_nil())
            .ok_or(VdiClipboardFilesMaterializationErrorV1::InvalidFilesReference)?;
        open_bounded_files_source(
            self.resolver.as_ref(),
            &TransferLocation::Local { object },
            request.byte_count,
            &request.content_hash,
        )
        .map_err(|error| match error {
            super::TransferV2ResolutionError::AccessDenied(_) => {
                VdiClipboardFilesMaterializationErrorV1::FilesDenied
            }
            super::TransferV2ResolutionError::MetadataMismatch { .. }
            | super::TransferV2ResolutionError::StaleResolution(_)
            | super::TransferV2ResolutionError::IdentityMismatch(_) => {
                VdiClipboardFilesMaterializationErrorV1::MetadataMismatch
            }
            _ => VdiClipboardFilesMaterializationErrorV1::FilesUnavailable,
        })
    }
}

impl Drop for ClipboardFilesMaterializer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn read_typed_latest<T: serde::de::DeserializeOwned>(
    persist: &Persist,
    topic: &str,
) -> Result<T, VdiClipboardFilesMaterializationErrorV1> {
    read_optional_typed_latest(persist, topic)?
        .ok_or(VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)
}

fn read_optional_typed_latest<T: serde::de::DeserializeOwned>(
    persist: &Persist,
    topic: &str,
) -> Result<Option<T>, VdiClipboardFilesMaterializationErrorV1> {
    let Some(body) = read_optional_latest_body(persist, topic)? else {
        return Ok(None);
    };
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|_| VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)
}

fn read_latest_body(
    persist: &Persist,
    topic: &str,
) -> Result<String, VdiClipboardFilesMaterializationErrorV1> {
    read_optional_latest_body(persist, topic)?
        .ok_or(VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)
}

fn read_optional_latest_body(
    persist: &Persist,
    topic: &str,
) -> Result<Option<String>, VdiClipboardFilesMaterializationErrorV1> {
    let Some(record) = persist
        .read_latest(topic)
        .map_err(|_| VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)?
    else {
        return Ok(None);
    };
    let body = record
        .body
        .as_ref()
        .ok_or(VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable)?;
    if body.len() > MAX_VDI_CLIPBOARD_TRANSPORT_V2_JSON_BYTES {
        return Err(VdiClipboardFilesMaterializationErrorV1::Oversized);
    }
    Ok(Some(body.clone()))
}

fn receive_request(
    stream: &UnixStream,
) -> Result<
    Option<VdiClipboardFilesMaterializationRequestV1>,
    VdiClipboardFilesMaterializationErrorV1,
> {
    let mut bytes = [0_u8; MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES + 1];
    let received = match recv(stream, &mut bytes, RecvFlags::empty()) {
        Ok(received) => received,
        Err(error) if error == rustix::io::Errno::AGAIN => return Ok(None),
        Err(_) => return Err(VdiClipboardFilesMaterializationErrorV1::AuthorityUnavailable),
    };
    if received == 0 || received > MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES {
        return Err(VdiClipboardFilesMaterializationErrorV1::Oversized);
    }
    let request: VdiClipboardFilesMaterializationRequestV1 =
        serde_json::from_slice(&bytes[..received])
            .map_err(|_| VdiClipboardFilesMaterializationErrorV1::UnsupportedSchema)?;
    request.validate()?;
    Ok(Some(request))
}

fn send_refusal(
    stream: &UnixStream,
    authorization_id: String,
    reason: VdiClipboardFilesMaterializationErrorV1,
) -> std::io::Result<()> {
    send_response(
        stream,
        &VdiClipboardFilesMaterializationResponseV1::Refused {
            authorization_id,
            reason,
        },
        None,
    )
}

fn send_response(
    stream: &UnixStream,
    response: &VdiClipboardFilesMaterializationResponseV1,
    descriptor: Option<std::os::fd::BorrowedFd<'_>>,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body.is_empty() || body.len() > MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "clipboard materialization response exceeded its packet cap",
        ));
    }
    if let Some(descriptor) = descriptor {
        let descriptors = [descriptor];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut control);
        if !ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)) {
            return Err(std::io::Error::other(
                "SCM_RIGHTS response buffer too small",
            ));
        }
        let sent = sendmsg(
            stream,
            &[IoSlice::new(&body)],
            &mut ancillary,
            SendFlags::empty(),
        )?;
        if sent != body.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "short clipboard materialization response",
            ));
        }
    } else {
        let sent = send(stream, &body, SendFlags::empty())?;
        if sent != body.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "short clipboard materialization refusal",
            ));
        }
    }
    Ok(())
}

fn seqpacket_connect(path: &Path) -> std::io::Result<UnixStream> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )?;
    connect_unix(&socket, &SocketAddrUnix::new(path)?)?;
    Ok(socket.into())
}

fn seqpacket_listener(path: &Path) -> std::io::Result<UnixStream> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    bind_unix(&socket, &SocketAddrUnix::new(path)?)?;
    listen(&socket, 8)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{IoSliceMut, Read as _};

    use mackes_mesh_types::vdi_clipboard::{
        ClipboardEnvelopeV2, VdiClipboardDisclosureV2, VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
    };
    use mde_bus::hooks::config::Priority;
    use mde_collab_types::FileRefId;
    use rustix::net::{recvmsg, RecvAncillaryBuffer, RecvAncillaryMessage};

    #[derive(Clone)]
    struct ResolverFixture(super::super::ResolvedFilesEndpoint);

    impl FilesEndpointResolver for ResolverFixture {
        fn resolve(
            &self,
            identity: &TransferLocation,
            _role: super::super::FilesEndpointRole,
        ) -> Result<super::super::ResolvedFilesEndpoint, super::super::FilesResolveFailure>
        {
            if identity == &self.0.identity {
                Ok(self.0.clone())
            } else {
                Err(super::super::FilesResolveFailure::Unavailable)
            }
        }
    }

    struct RejectingResolver;

    impl FilesEndpointResolver for RejectingResolver {
        fn resolve(
            &self,
            _identity: &TransferLocation,
            _role: super::super::FilesEndpointRole,
        ) -> Result<super::super::ResolvedFilesEndpoint, super::super::FilesResolveFailure>
        {
            Err(super::super::FilesResolveFailure::Unavailable)
        }
    }

    #[test]
    fn silent_clients_never_block_the_worker_and_expire() {
        let root = tempfile::tempdir().expect("authority root");
        let mut endpoint = ClipboardFilesMaterializer::bind(
            root.path().to_path_buf(),
            Arc::new(RejectingResolver),
        )
        .expect("materializer");
        let socket = root.path().join(VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET);
        let clients: Vec<_> = (0..MAX_PENDING_CLIENTS)
            .map(|_| seqpacket_connect(&socket).expect("silent client"))
            .collect();

        endpoint.drain(10_000);
        assert_eq!(endpoint.pending.len(), MAX_PENDING_CLIENTS);
        endpoint.drain(10_000 + PENDING_CLIENT_TTL_MS + 1);
        assert!(endpoint.pending.is_empty());

        drop(clients);
    }

    #[test]
    fn exact_files_command_releases_one_verified_descriptor_once() {
        let root = tempfile::tempdir().expect("authority root");
        let content_root = root.path().join("content");
        let bytes = b"small image source";
        let hash = ClipboardEnvelopeV2::content_hash_for(bytes);
        let relative = PathBuf::from(&hash[..2]).join(&hash);
        let path = content_root.join(&relative);
        std::fs::create_dir_all(path.parent().expect("content parent")).expect("content dirs");
        std::fs::write(&path, bytes).expect("content bytes");
        let canonical_root = std::fs::canonicalize(&content_root).expect("canonical content root");
        let object = FileRefId::new();
        let identity = TransferLocation::Local { object };
        let resolver = Arc::new(ResolverFixture(super::super::ResolvedFilesEndpoint {
            identity: identity.clone(),
            canonical_root,
            relative_path: relative,
            generation: 4,
            sha256_hex: hash.clone(),
            size_bytes: bytes.len() as u64,
            object_type: super::super::FilesObjectType::RegularFile,
            available: true,
            readable: true,
            writable: false,
        }));
        let now_ms = 1_700_000_000_000;
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "rdp:oak:image".into(),
            generation: 3,
            lease_id: "rdp-image-3".into(),
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + 60_000,
            permitted_mime_offers: vec!["image/png".into()],
        };
        let envelope = ClipboardEnvelopeV2::new_files(
            "node-a",
            "seat-a",
            "source-session",
            1,
            now_ms,
            vec!["image/png".into()],
            "image",
            hash,
            bytes.len() as u64,
            format!("{FILES_REFERENCE_PREFIX}{object}"),
            now_ms + 30_000,
        )
        .expect("image envelope");
        let command = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: 1,
            selected_mime: "image/png".into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        };
        let persist = Persist::open(root.path().to_path_buf()).expect("Bus");
        for (prefix, body) in [
            (
                VDI_CLIPBOARD_LEASE_TOPIC_PREFIX,
                serde_json::to_string(&lease).expect("lease JSON"),
            ),
            (
                VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
                serde_json::to_string(&command).expect("command JSON"),
            ),
        ] {
            let topic = vdi_clipboard_session_topic(prefix, &lease.session_id).expect("topic");
            persist
                .write(&topic, Priority::Default, None, Some(&body))
                .expect("Bus write");
        }
        let mut endpoint = ClipboardFilesMaterializer::bind(root.path().to_path_buf(), resolver)
            .expect("materializer");
        let request = VdiClipboardFilesMaterializationRequestV1::from_message(
            &command,
            "86ad680c-2ae2-4ac8-8b31-74de41450ee3",
        )
        .expect("request");
        let client =
            seqpacket_connect(&root.path().join(VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET))
                .expect("root client");
        let request_body = serde_json::to_vec(&request).expect("request JSON");
        assert_eq!(
            send(&client, &request_body, SendFlags::empty()).expect("send request"),
            request_body.len()
        );
        endpoint.drain(now_ms + 1);
        assert_eq!(
            endpoint.authorizations.get(&request.authorization_id),
            Some(&(now_ms + 30_000)),
            "one-use authority follows the earlier envelope expiry"
        );
        assert_eq!(endpoint.commands.len(), 1);
        let mut response_bytes = [0_u8; MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES];
        let mut iov = [IoSliceMut::new(&mut response_bytes)];
        let mut control = [0_u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut control);
        let received = recvmsg(&client, &mut iov, &mut ancillary, RecvFlags::empty())
            .expect("descriptor response");
        let response: VdiClipboardFilesMaterializationResponseV1 =
            serde_json::from_slice(&response_bytes[..received.bytes]).expect("response JSON");
        assert!(matches!(
            response,
            VdiClipboardFilesMaterializationResponseV1::Ready { .. }
        ));
        let mut descriptor = None;
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(mut descriptors) = message {
                descriptor = descriptors.next();
                assert!(descriptors.next().is_none());
            }
        }
        let mut file = std::fs::File::from(descriptor.expect("one descriptor"));
        let mut observed = Vec::new();
        file.read_to_end(&mut observed).expect("descriptor bytes");
        assert_eq!(observed, bytes);

        let replay_client =
            seqpacket_connect(&root.path().join(VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET))
                .expect("replay client");
        assert_eq!(
            send(&replay_client, &request_body, SendFlags::empty()).expect("send replay"),
            request_body.len()
        );
        endpoint.drain(now_ms + 2);
        let mut refusal = [0_u8; MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES];
        let received = recv(&replay_client, &mut refusal, RecvFlags::empty()).expect("refusal");
        let response: VdiClipboardFilesMaterializationResponseV1 =
            serde_json::from_slice(&refusal[..received]).expect("refusal JSON");
        assert_eq!(
            response,
            VdiClipboardFilesMaterializationResponseV1::Refused {
                authorization_id: request.authorization_id,
                reason: VdiClipboardFilesMaterializationErrorV1::Replayed,
            }
        );
        endpoint.drain(now_ms + 30_000);
        assert!(endpoint.authorizations.is_empty());
        assert!(endpoint.commands.is_empty());
    }
}
