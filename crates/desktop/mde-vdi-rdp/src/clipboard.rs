//! Bounded Unicode-text and CF_HTML CLIPRDR backend for the live RDP transport.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsFlags, FileContentsRequest, FileContentsResponse, FileDescriptor,
    FormatDataRequest, FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp_core::impl_as_any;
use mackes_mesh_types::vdi_clipboard::{
    VdiClipboardFileDescriptorV1, MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES,
    MAX_VDI_CLIPBOARD_FILE_DESCRIPTORS, MAX_VDI_CLIPBOARD_TEXT_BYTES,
    MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES,
};

/// The standard CLIPRDR text format supported by this backend.
pub const UNICODE_TEXT_FORMAT: ClipboardFormat =
    ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT);

/// A private registered-format ID paired with Windows' canonical CF_HTML name.
///
/// Registered IDs are scoped to the advertised format list. The peer requests
/// this exact ID after mapping the accompanying name into its local registry.
pub const HTML_FORMAT_ID: ClipboardFormatId = ClipboardFormatId(0xC000);
/// Standard Windows device-independent bitmap formats carried by CLIPRDR.
pub const DIB_FORMAT: ClipboardFormat = ClipboardFormat::new(ClipboardFormatId(8));
/// Standard Windows V5 device-independent bitmap format carried by CLIPRDR.
pub const DIBV5_FORMAT: ClipboardFormat = ClipboardFormat::new(ClipboardFormatId(17));

const MAX_REMOTE_FORMATS: usize = 256;
const REMOTE_FILE_CHUNK_BYTES: u32 = 256 * 1024;
const MAX_LOCAL_FILE_RESPONSES: usize = 32;
const LOCAL_FILE_SERVE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const CF_HTML_HEADER_SLACK_BYTES: usize = 1024;
const CF_HTML_PREFIX: &str = "<html><body><!--StartFragment-->";
const CF_HTML_SUFFIX: &str = "<!--EndFragment--></body></html>";

/// Build the named registered format used for CF_HTML negotiation.
#[must_use]
pub fn html_format() -> ClipboardFormat {
    ClipboardFormat::new(HTML_FORMAT_ID).with_name(ClipboardFormatName::HTML)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteFormat {
    UnicodeText,
    Html(ClipboardFormatId),
    Dib,
    DibV5,
    Files(ClipboardFormatId),
}

impl RemoteFormat {
    fn id(self) -> ClipboardFormatId {
        match self {
            Self::UnicodeText => ClipboardFormatId::CF_UNICODETEXT,
            Self::Html(id) => id,
            Self::Dib => DIB_FORMAT.id(),
            Self::DibV5 => DIBV5_FORMAT.id(),
            Self::Files(id) => id,
        }
    }
}

fn negotiated_file_list_format(available_formats: &[ClipboardFormat]) -> Option<RemoteFormat> {
    let is_file_list = |format: &ClipboardFormat| {
        format.id().is_registered()
            && format
                .name()
                .is_some_and(|name| name.value() == ClipboardFormatName::FILE_LIST.value())
    };
    let mut negotiated_id = None;
    for format in available_formats
        .iter()
        .filter(|format| is_file_list(format))
    {
        match negotiated_id {
            None => negotiated_id = Some(format.id()),
            Some(id) if id == format.id() => {}
            Some(_) => return None,
        }
    }
    let id = negotiated_id?;
    if available_formats
        .iter()
        .any(|format| format.id() == id && !is_file_list(format))
    {
        return None;
    }
    Some(RemoteFormat::Files(id))
}

fn negotiated_html_format(available_formats: &[ClipboardFormat]) -> Option<RemoteFormat> {
    let is_html = |format: &ClipboardFormat| {
        format.id().is_registered()
            && format
                .name()
                .is_some_and(|name| name.value() == ClipboardFormatName::HTML.value())
    };
    let mut negotiated_id = None;
    for format in available_formats.iter().filter(|format| is_html(format)) {
        match negotiated_id {
            None => negotiated_id = Some(format.id()),
            Some(id) if id == format.id() => {}
            Some(_) => return None,
        }
    }
    let id = negotiated_id?;

    // Registered IDs have meaning only within this advertised format list. If
    // the peer assigns the selected ID both to HTML and to an unnamed or
    // differently named entry, a later response cannot prove which MIME it
    // represents. Refuse the equivocation instead of making offer order an
    // authority decision.
    if available_formats
        .iter()
        .any(|format| format.id() == id && !is_html(format))
    {
        return None;
    }
    Some(RemoteFormat::Html(id))
}

/// One bounded guest image admitted from the exact negotiated CLIPRDR format.
///
/// Keeping the wire format in the type prevents a caller from treating a
/// CF_DIB response as CF_DIBV5 (or vice versa) while materializing the rich
/// clipboard payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteClipboardImageFormat {
    /// A validated CF_DIB payload.
    Dib,
    /// A validated CF_DIBV5 payload.
    DibV5,
}

/// Validated guest bitmap bytes paired with their exact negotiated format.
/// The fields are private so code outside this admission boundary cannot forge
/// a value around unvalidated bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteClipboardImage {
    format: RemoteClipboardImageFormat,
    data: Vec<u8>,
}

/// Bounded guest file metadata admitted from `FileGroupDescriptorW`.
///
/// This type deliberately carries metadata only. Raw guest paths and payloads
/// never become host paths at the CLIPRDR boundary; the Files authority decides
/// where a later, chunked transfer is materialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteClipboardFile {
    descriptor: VdiClipboardFileDescriptorV1,
}

impl RemoteClipboardFile {
    /// Sanitized basename supplied by IronRDP and re-attested by this boundary.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.descriptor.name
    }

    /// Sanitized relative directory, never an absolute or parent path.
    #[must_use]
    pub fn relative_path(&self) -> Option<&str> {
        self.descriptor.relative_path.as_deref()
    }

    /// Declared byte size admitted under the rich-envelope aggregate ceiling.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.descriptor.byte_count
    }
}

/// One negotiated, bounded guest file-list snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteClipboardFileList {
    files: Vec<RemoteClipboardFile>,
    clip_data_id: Option<u32>,
}

/// One sequential, bounded range ready for the Files authority to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteClipboardFileChunk {
    file_index: usize,
    offset: u64,
    data: Vec<u8>,
    complete: bool,
}

impl RemoteClipboardFileChunk {
    #[must_use]
    pub const fn file_index(&self) -> usize {
        self.file_index
    }

    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

#[derive(Debug, Clone)]
struct RemoteFileTransfer {
    file_index: usize,
    size: u64,
    next_offset: u64,
    stream_id: u32,
    clip_data_id: Option<u32>,
    request_outstanding: bool,
}

#[derive(Debug, Clone)]
struct LocalFileOffer {
    generation: u64,
    data: Arc<[u8]>,
    admitted_at: std::time::Instant,
}

impl RemoteClipboardFileList {
    /// Admitted file metadata in guest order.
    #[must_use]
    pub fn files(&self) -> &[RemoteClipboardFile] {
        &self.files
    }

    /// IronRDP lock identity binding later chunks to this exact clipboard snapshot.
    #[must_use]
    pub const fn clip_data_id(&self) -> Option<u32> {
        self.clip_data_id
    }
}

impl RemoteClipboardImage {
    fn new(format: RemoteClipboardImageFormat, data: &[u8]) -> Self {
        Self {
            format,
            data: data.to_vec(),
        }
    }

    /// Exact CLIPRDR format against which these bytes were validated.
    #[must_use]
    pub const fn format(&self) -> RemoteClipboardImageFormat {
        self.format
    }

    /// Return the validated DIB bytes without discarding their typed format.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Debug, Default)]
struct ClipboardState {
    ready: bool,
    file_stream_ready: bool,
    initial_format_list_requested: bool,
    local_generation: u64,
    local_advertised_generation: Option<u64>,
    local_text: Option<String>,
    local_html: Option<Vec<u8>>,
    local_dib: Option<Vec<u8>>,
    local_file: Option<LocalFileOffer>,
    locked_local_files: BTreeMap<u32, LocalFileOffer>,
    local_file_responses: VecDeque<FileContentsResponse<'static>>,
    local_data_request: Option<(FormatDataRequest, Option<u64>)>,
    remote_unicode_offer: Option<RemoteFormat>,
    remote_html_offer: Option<RemoteFormat>,
    remote_image_offer: Option<RemoteFormat>,
    remote_file_offer: Option<RemoteFormat>,
    pending_remote_request: Option<RemoteFormat>,
    discard_replaced_response: bool,
    remote_text: Option<String>,
    remote_html: Option<String>,
    remote_image: Option<RemoteClipboardImage>,
    remote_files: Option<Result<RemoteClipboardFileList, ClipboardBridgeError>>,
    admitted_remote_files: Option<RemoteClipboardFileList>,
    remote_file_transfer: Option<RemoteFileTransfer>,
    remote_file_chunk: Option<Result<RemoteClipboardFileChunk, ClipboardBridgeError>>,
}

/// Thread-local connection handle used by the wire pump to service CLIPRDR
/// callbacks without exposing an OS clipboard directly to IronRDP.
#[derive(Debug, Clone)]
pub struct ClipboardBridge {
    state: Arc<Mutex<ClipboardState>>,
}

impl ClipboardBridge {
    /// Build the shared pump handle and the backend owned by IronRDP.
    #[must_use]
    pub fn pair() -> (Self, Box<dyn CliprdrBackend>) {
        let state = Arc::new(Mutex::new(ClipboardState::default()));
        (
            Self {
                state: Arc::clone(&state),
            },
            Box::new(TextCliprdrBackend { state }),
        )
    }

    /// Replace the host text offered to the guest. The caller must subsequently
    /// send a CLIPRDR format list through `Cliprdr::initiate_copy`.
    pub fn offer_host_text(&self, text: String) -> Result<(), ClipboardBridgeError> {
        if text.len() > MAX_VDI_CLIPBOARD_TEXT_BYTES {
            let error = ClipboardBridgeError::TooLarge {
                bytes: text.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            };
            self.revoke_local_offer();
            return Err(error);
        }
        let mut state = self.lock();
        state.local_generation = state.local_generation.wrapping_add(1);
        state.local_text = Some(text);
        state.local_html = None;
        state.local_dib = None;
        Ok(())
    }

    /// Replace the host offer with one bounded HTML fragment encoded as the
    /// Windows CF_HTML registered format.
    pub fn offer_host_html(&self, html: String) -> Result<(), ClipboardBridgeError> {
        if html.len() > MAX_VDI_CLIPBOARD_TEXT_BYTES {
            let error = ClipboardBridgeError::TooLarge {
                bytes: html.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            };
            self.revoke_local_offer();
            return Err(error);
        }
        let wire = encode_cf_html(&html);
        let mut state = self.lock();
        state.local_generation = state.local_generation.wrapping_add(1);
        state.local_text = None;
        state.local_html = Some(wire);
        state.local_dib = None;
        Ok(())
    }

    /// Replace the host offer with one already-encoded, bounded CF_DIBV5.
    /// Image decoding remains in the shell, which owns the Files descriptor;
    /// this protocol boundary accepts only a structurally valid DIB allocation.
    pub fn offer_host_dibv5(&self, dib: Vec<u8>) -> Result<(), ClipboardBridgeError> {
        if let Err(error) = validate_dib(&dib, Some(DIBV5_FORMAT.id())) {
            self.revoke_local_offer();
            return Err(error);
        }
        let mut state = self.lock();
        state.local_generation = state.local_generation.wrapping_add(1);
        state.local_text = None;
        state.local_html = None;
        state.local_dib = Some(dib);
        Ok(())
    }

    /// Replace the host offer with one bounded daemon-materialized file.
    ///
    /// The shell must obtain `data` from the governed Files descriptor only
    /// after its one-use permission decision. This boundary retains no path and
    /// serves only exact delayed-rendering ranges requested by CLIPRDR.
    pub fn offer_host_file(
        &self,
        name: String,
        data: Vec<u8>,
    ) -> Result<FileDescriptor, ClipboardBridgeError> {
        let Ok(descriptor) = VdiClipboardFileDescriptorV1::new(
            name.clone(),
            None,
            "application/octet-stream",
            data.len() as u64,
        ) else {
            self.revoke_local_offer();
            return Err(ClipboardBridgeError::InvalidLocalFile);
        };
        let mut state = self.lock();
        if !state.ready || !state.file_stream_ready {
            drop(state);
            self.revoke_local_offer();
            return Err(ClipboardBridgeError::InvalidLocalFile);
        }
        state.local_generation = state.local_generation.wrapping_add(1);
        let generation = state.local_generation;
        state.local_text = None;
        state.local_html = None;
        state.local_dib = None;
        state.local_file = Some(LocalFileOffer {
            generation,
            data: Arc::from(data),
            admitted_at: std::time::Instant::now(),
        });
        state.local_advertised_generation = Some(generation);
        Ok(FileDescriptor::new(descriptor.name)
            .with_file_size(state.local_file.as_ref().map_or(0, |file| file.data.len()) as u64))
    }

    /// Take one response to a guest's delayed file request.
    pub fn take_local_file_response(&self) -> Option<FileContentsResponse<'static>> {
        self.lock().local_file_responses.pop_front()
    }

    /// Return only formats backed by the current local offer.
    #[must_use]
    pub fn advertised_formats(&self) -> Vec<ClipboardFormat> {
        let mut state = self.lock();
        state.local_advertised_generation = Some(state.local_generation);
        if state.local_text.is_some() {
            vec![UNICODE_TEXT_FORMAT]
        } else if state.local_html.is_some() {
            vec![html_format()]
        } else if state.local_dib.is_some() {
            // The stored payload is specifically CF_DIBV5.  Advertising the
            // classic CF_DIB ID as an alias would let a peer request a
            // different wire format and receive bytes validated only for V5.
            vec![DIBV5_FORMAT]
        } else if state.local_file.is_some() {
            // File offers are announced by IronRDP's `initiate_file_copy`,
            // which owns the registered format IDs and snapshot bookkeeping.
            Vec::new()
        } else {
            Vec::new()
        }
    }

    /// Whether IronRDP requested the initial local format list.
    pub fn take_initial_format_list_request(&self) -> bool {
        std::mem::take(&mut self.lock().initial_format_list_requested)
    }

    /// Take a server request for the currently offered host data.
    pub fn take_local_data_response(&self) -> Option<OwnedFormatDataResponse> {
        let mut state = self.lock();
        let (request, requested_generation) = state.local_data_request.take()?;
        if requested_generation != Some(state.local_generation) {
            return Some(OwnedFormatDataResponse::new_error());
        }
        Some(if request.format == ClipboardFormatId::CF_UNICODETEXT {
            match state.local_text.as_deref() {
                Some(text) => OwnedFormatDataResponse::new_unicode_string(text),
                None => OwnedFormatDataResponse::new_error(),
            }
        } else if request.format == HTML_FORMAT_ID {
            match state.local_html.as_ref() {
                Some(html) => OwnedFormatDataResponse::new_data(html.clone()),
                None => OwnedFormatDataResponse::new_error(),
            }
        } else if request.format == DIBV5_FORMAT.id() {
            match state.local_dib.as_ref() {
                Some(dib) => OwnedFormatDataResponse::new_data(dib.clone()),
                None => OwnedFormatDataResponse::new_error(),
            }
        } else {
            OwnedFormatDataResponse::new_error()
        })
    }

    /// Take the next truthfully negotiated remote format and bind the eventual
    /// callback to it. Unicode and HTML remain first so existing consumers
    /// retain their established paths when a peer also advertises images.
    pub fn take_remote_format_request(&self) -> Option<ClipboardFormatId> {
        let mut state = self.lock();
        if state.pending_remote_request.is_some() || state.discard_replaced_response {
            return None;
        }
        let format = state
            .remote_unicode_offer
            .take()
            .or_else(|| state.remote_html_offer.take())
            .or_else(|| state.remote_image_offer.take())
            .or_else(|| state.remote_file_offer.take())?;
        state.pending_remote_request = Some(format);
        Some(format.id())
    }

    /// Take the latest bounded guest text returned by CLIPRDR.
    pub fn take_remote_text(&self) -> Option<String> {
        self.lock().remote_text.take()
    }

    /// Take the latest bounded guest HTML fragment returned by CF_HTML.
    pub fn take_remote_html(&self) -> Option<String> {
        self.lock().remote_html.take()
    }

    /// Take the latest bounded guest image with its negotiated wire format.
    pub fn take_remote_image(&self) -> Option<RemoteClipboardImage> {
        self.lock().remote_image.take()
    }

    /// Take a guest file-list admission result exactly once.
    ///
    /// A rejected list is surfaced as a typed failure rather than silently
    /// disappearing or causing the transport to allocate from hostile sizes.
    pub fn take_remote_file_list(
        &self,
    ) -> Option<Result<RemoteClipboardFileList, ClipboardBridgeError>> {
        self.lock().remote_files.take()
    }

    /// Begin sequential range retrieval for one admitted guest file.
    ///
    /// Bytes are exposed only as bounded chunks for the Files authority to
    /// persist; this transport boundary never allocates from the declared full
    /// file size.
    pub fn begin_remote_file_retrieval(
        &self,
        file_index: usize,
    ) -> Result<(), ClipboardBridgeError> {
        let mut state = self.lock();
        if state.remote_file_transfer.is_some() || state.remote_file_chunk.is_some() {
            return Err(ClipboardBridgeError::InvalidFileTransfer);
        }
        let snapshot = state
            .admitted_remote_files
            .as_ref()
            .ok_or(ClipboardBridgeError::InvalidFileTransfer)?;
        let size = snapshot
            .files
            .get(file_index)
            .ok_or(ClipboardBridgeError::InvalidFileTransfer)?
            .size();
        let clip_data_id = snapshot.clip_data_id;
        if size == 0 {
            state.remote_file_chunk = Some(Ok(RemoteClipboardFileChunk {
                file_index,
                offset: 0,
                data: Vec::new(),
                complete: true,
            }));
            return Ok(());
        }
        let stream_id = state.local_generation.wrapping_add(1) as u32;
        state.local_generation = state.local_generation.wrapping_add(1);
        state.remote_file_transfer = Some(RemoteFileTransfer {
            file_index,
            size,
            next_offset: 0,
            stream_id,
            clip_data_id,
            request_outstanding: false,
        });
        Ok(())
    }

    /// Take the next exact CLIPRDR range request. Only one request may be in flight.
    pub fn take_remote_file_contents_request(&self) -> Option<FileContentsRequest> {
        let mut state = self.lock();
        if state.remote_file_chunk.is_some() {
            return None;
        }
        let transfer = state.remote_file_transfer.as_mut()?;
        if transfer.request_outstanding || transfer.next_offset >= transfer.size {
            return None;
        }
        let remaining = transfer.size - transfer.next_offset;
        let requested_size = remaining.min(u64::from(REMOTE_FILE_CHUNK_BYTES)) as u32;
        transfer.request_outstanding = true;
        Some(FileContentsRequest {
            stream_id: transfer.stream_id,
            index: i32::try_from(transfer.file_index).ok()?,
            flags: FileContentsFlags::RANGE,
            position: transfer.next_offset,
            requested_size,
            data_id: transfer.clip_data_id,
        })
    }

    /// Take one validated sequential chunk for Files materialization.
    pub fn take_remote_file_chunk(
        &self,
    ) -> Option<Result<RemoteClipboardFileChunk, ClipboardBridgeError>> {
        self.lock().remote_file_chunk.take()
    }

    /// Whether CLIPRDR completed its capability handshake.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.lock().ready
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClipboardState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn revoke_local_offer(&self) {
        let mut state = self.lock();
        state.local_generation = state.local_generation.wrapping_add(1);
        state.local_text = None;
        state.local_html = None;
        state.local_dib = None;
        state.local_file = None;
        state.local_advertised_generation = None;
        state.local_data_request = None;
        state.locked_local_files.clear();
        state.local_file_responses.clear();
    }
}

fn local_file_response(
    state: &ClipboardState,
    request: &FileContentsRequest,
) -> FileContentsResponse<'static> {
    let file = match request.data_id {
        Some(data_id) => state.locked_local_files.get(&data_id),
        None => None,
    };
    let Some(file) =
        file.filter(|file| request.index == 0 && file.admitted_at.elapsed() < LOCAL_FILE_SERVE_TTL)
    else {
        return FileContentsResponse::new_error(request.stream_id);
    };
    if request.flags == FileContentsFlags::SIZE {
        if request.position != 0 || request.requested_size != 8 {
            return FileContentsResponse::new_error(request.stream_id);
        }
        return FileContentsResponse::new_size_response(request.stream_id, file.data.len() as u64);
    }
    if request.flags != FileContentsFlags::RANGE
        || request.requested_size == 0
        || request.requested_size > REMOTE_FILE_CHUNK_BYTES
    {
        return FileContentsResponse::new_error(request.stream_id);
    }
    let Ok(start) = usize::try_from(request.position) else {
        return FileContentsResponse::new_error(request.stream_id);
    };
    let Some(end) = start.checked_add(request.requested_size as usize) else {
        return FileContentsResponse::new_error(request.stream_id);
    };
    if start >= file.data.len() {
        return FileContentsResponse::new_error(request.stream_id);
    }
    FileContentsResponse::new_data_response(
        request.stream_id,
        file.data[start..end.min(file.data.len())].to_vec(),
    )
}

/// A bounded CLIPRDR admission failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBridgeError {
    /// UTF-8 text or HTML exceeded the canonical VDI clipboard limit.
    TooLarge {
        /// Rejected UTF-8 byte count.
        bytes: usize,
        /// Canonical maximum UTF-8 byte count.
        max_bytes: usize,
    },
    /// The image was not a bounded, self-consistent CF_DIB/CF_DIBV5 payload.
    InvalidImage,
    /// Guest file-list metadata was unsafe, incomplete, or exceeded a bound.
    InvalidFileList,
    /// Guest file ranges were unsolicited, non-sequential, oversized, or stale.
    InvalidFileTransfer,
    /// Host file metadata or bytes exceeded the bounded serving contract.
    InvalidLocalFile,
}

impl core::fmt::Display for ClipboardBridgeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge { bytes, max_bytes } => write!(
                formatter,
                "RDP clipboard payload is {bytes} bytes; maximum is {max_bytes}"
            ),
            Self::InvalidImage => formatter.write_str("RDP clipboard DIB is malformed or unsafe"),
            Self::InvalidFileList => {
                formatter.write_str("RDP clipboard file list is malformed or unsafe")
            }
            Self::InvalidFileTransfer => {
                formatter.write_str("RDP clipboard file transfer is malformed or stale")
            }
            Self::InvalidLocalFile => {
                formatter.write_str("RDP host clipboard file is malformed or oversized")
            }
        }
    }
}

impl std::error::Error for ClipboardBridgeError {}

#[derive(Debug)]
struct TextCliprdrBackend {
    state: Arc<Mutex<ClipboardState>>,
}

impl_as_any!(TextCliprdrBackend);

impl TextCliprdrBackend {
    fn with_state(&self, update: impl FnOnce(&mut ClipboardState)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut state);
    }
}

impl CliprdrBackend for TextCliprdrBackend {
    fn temporary_directory(&self) -> &str {
        "/tmp"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
    }

    fn on_ready(&mut self) {
        self.with_state(|state| state.ready = true);
    }

    fn on_request_format_list(&mut self) {
        self.with_state(|state| state.initial_format_list_requested = true);
    }

    fn on_format_list_response(&mut self, ok: bool) {
        if !ok {
            self.with_state(|state| {
                state.local_generation = state.local_generation.wrapping_add(1);
                state.local_advertised_generation = None;
                state.local_text = None;
                state.local_html = None;
                state.local_dib = None;
                state.local_file = None;
                state.local_file_responses.clear();
            });
        }
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        self.with_state(|state| {
            state.file_stream_ready = capabilities
                .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
                && capabilities.contains(ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA);
        });
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.with_state(|state| {
            if state.pending_remote_request.take().is_some() {
                // CLIPRDR responses carry no request ID. Consume and refuse the
                // old response before issuing a request against the new offer.
                state.discard_replaced_response = true;
            }
            state.remote_unicode_offer = None;
            state.remote_html_offer = None;
            state.remote_image_offer = None;
            state.remote_file_offer = None;
            state.remote_text = None;
            state.remote_html = None;
            state.remote_image = None;
            state.remote_files = None;
            state.admitted_remote_files = None;
            state.remote_file_transfer = None;
            state.remote_file_chunk = None;

            if available_formats.len() > MAX_REMOTE_FORMATS {
                return;
            }
            state.remote_unicode_offer = available_formats
                .iter()
                .any(|format| format.id() == ClipboardFormatId::CF_UNICODETEXT)
                .then_some(RemoteFormat::UnicodeText);
            state.remote_html_offer = negotiated_html_format(available_formats);
            state.remote_image_offer = if available_formats
                .iter()
                .any(|format| format.id() == DIBV5_FORMAT.id())
            {
                Some(RemoteFormat::DibV5)
            } else if available_formats
                .iter()
                .any(|format| format.id() == DIB_FORMAT.id())
            {
                Some(RemoteFormat::Dib)
            } else {
                None
            };
            state.remote_file_offer = negotiated_file_list_format(available_formats);
        });
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.with_state(|state| {
            state.local_data_request = Some((request, state.local_advertised_generation));
        });
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        self.with_state(|state| {
            if state.discard_replaced_response {
                state.discard_replaced_response = false;
                return;
            }
            let Some(format) = state.pending_remote_request.take() else {
                // CLIPRDR has no response nonce.  Once a response has been
                // admitted, an unsolicited duplicate is only a replay; it
                // must not erase the already-published value or turn a
                // successful transfer into an unexplained empty clipboard.
                return;
            };
            if response.is_error() {
                match format {
                    RemoteFormat::UnicodeText => state.remote_text = None,
                    RemoteFormat::Html(_) => state.remote_html = None,
                    RemoteFormat::Dib | RemoteFormat::DibV5 => state.remote_image = None,
                    RemoteFormat::Files(_) => state.remote_files = None,
                }
                return;
            }
            match format {
                RemoteFormat::UnicodeText => {
                    state.remote_text = decode_unicode_text(response.data())
                }
                RemoteFormat::Html(_) => {
                    state.remote_html = decode_cf_html(response.data())
                        .filter(|html| guest_html_fragment_is_safe(html));
                }
                RemoteFormat::Dib => {
                    state.remote_image = validate_dib(response.data(), Some(DIB_FORMAT.id()))
                        .is_ok()
                        .then(|| {
                            RemoteClipboardImage::new(
                                RemoteClipboardImageFormat::Dib,
                                response.data(),
                            )
                        });
                }
                RemoteFormat::DibV5 => {
                    state.remote_image = validate_dib(response.data(), Some(DIBV5_FORMAT.id()))
                        .is_ok()
                        .then(|| {
                            RemoteClipboardImage::new(
                                RemoteClipboardImageFormat::DibV5,
                                response.data(),
                            )
                        });
                }
                RemoteFormat::Files(_) => {
                    // IronRDP decodes file-list responses through
                    // `on_remote_file_list`; a generic response here is not
                    // authoritative file metadata.
                    state.remote_files = Some(Err(ClipboardBridgeError::InvalidFileList));
                }
            }
        });
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.with_state(|state| {
            let response = local_file_response(state, &request);
            if state.local_file_responses.len() >= MAX_LOCAL_FILE_RESPONSES {
                state.local_file_responses.clear();
                state
                    .local_file_responses
                    .push_back(FileContentsResponse::new_error(request.stream_id));
            } else {
                state.local_file_responses.push_back(response);
            }
        });
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        self.with_state(|state| {
            let Some(transfer) = state.remote_file_transfer.as_mut() else {
                return;
            };
            let expected = (transfer.size - transfer.next_offset)
                .min(u64::from(REMOTE_FILE_CHUNK_BYTES)) as usize;
            if !transfer.request_outstanding
                || response.stream_id() != transfer.stream_id
                || response.is_error()
                || response.data().is_empty()
                || response.data().len() > expected
            {
                state.remote_file_transfer = None;
                state.remote_file_chunk = Some(Err(ClipboardBridgeError::InvalidFileTransfer));
                return;
            }
            let offset = transfer.next_offset;
            transfer.next_offset += response.data().len() as u64;
            transfer.request_outstanding = false;
            let complete = transfer.next_offset == transfer.size;
            state.remote_file_chunk = Some(Ok(RemoteClipboardFileChunk {
                file_index: transfer.file_index,
                offset,
                data: response.data().to_vec(),
                complete,
            }));
            if complete {
                state.remote_file_transfer = None;
            }
        });
    }

    fn on_lock(&mut self, data_id: LockDataId) {
        self.with_state(|state| {
            if let Some(file) = state
                .local_file
                .as_ref()
                .filter(|file| state.local_advertised_generation == Some(file.generation))
            {
                state.locked_local_files.insert(data_id.0, file.clone());
            }
        });
    }

    fn on_unlock(&mut self, data_id: LockDataId) {
        self.with_state(|state| {
            state.locked_local_files.remove(&data_id.0);
        });
    }

    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        self.with_state(|state| {
            if state.discard_replaced_response {
                state.discard_replaced_response = false;
                return;
            }
            if !matches!(
                state.pending_remote_request.take(),
                Some(RemoteFormat::Files(_))
            ) {
                state.remote_files = Some(Err(ClipboardBridgeError::InvalidFileList));
                return;
            }
            let result = admit_remote_file_list(files, clip_data_id);
            state.admitted_remote_files = result.as_ref().ok().cloned();
            state.remote_files = Some(result);
        });
    }

    fn on_outgoing_locks_cleared(&mut self, clip_data_ids: &[LockDataId]) {
        self.with_state(|state| {
            let cleared = state
                .admitted_remote_files
                .as_ref()
                .and_then(RemoteClipboardFileList::clip_data_id)
                .is_some_and(|id| clip_data_ids.iter().any(|cleared| cleared.0 == id));
            if cleared {
                state.remote_files = None;
                state.admitted_remote_files = None;
                state.remote_file_transfer = None;
                state.remote_file_chunk = None;
            }
        });
    }
}

fn admit_remote_file_list(
    files: &[FileDescriptor],
    clip_data_id: Option<u32>,
) -> Result<RemoteClipboardFileList, ClipboardBridgeError> {
    if files.is_empty() || files.len() > MAX_VDI_CLIPBOARD_FILE_DESCRIPTORS {
        return Err(ClipboardBridgeError::InvalidFileList);
    }

    let mut total_bytes = 0_u64;
    let mut admitted = Vec::with_capacity(files.len());
    for file in files {
        let size = file
            .file_size
            .ok_or(ClipboardBridgeError::InvalidFileList)?;
        total_bytes = total_bytes
            .checked_add(size)
            .filter(|total| *total <= MAX_CLIPBOARD_ENVELOPE_V2_CONTENT_BYTES)
            .ok_or(ClipboardBridgeError::InvalidFileList)?;
        let descriptor = VdiClipboardFileDescriptorV1::new(
            file.name.clone(),
            file.relative_path.clone(),
            "application/octet-stream",
            size,
        )
        .map_err(|_| ClipboardBridgeError::InvalidFileList)?;
        admitted.push(RemoteClipboardFile { descriptor });
    }
    Ok(RemoteClipboardFileList {
        files: admitted,
        clip_data_id,
    })
}

fn decode_unicode_text(data: &[u8]) -> Option<String> {
    if data.len()
        > MAX_VDI_CLIPBOARD_TEXT_BYTES
            .saturating_mul(2)
            .saturating_add(2)
        || !data.len().is_multiple_of(2)
    {
        return None;
    }
    let mut units = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    units.truncate(
        units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len()),
    );
    let text = String::from_utf16(&units).ok()?;
    (text.len() <= MAX_VDI_CLIPBOARD_TEXT_BYTES).then_some(text)
}

fn validate_dib(
    data: &[u8],
    format: Option<ClipboardFormatId>,
) -> Result<(), ClipboardBridgeError> {
    let max = usize::try_from(MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES).unwrap_or(usize::MAX);
    if data.len() < 40 || data.len() > max {
        return Err(if data.len() > max {
            ClipboardBridgeError::TooLarge {
                bytes: data.len(),
                max_bytes: max,
            }
        } else {
            ClipboardBridgeError::InvalidImage
        });
    }
    let u16_at = |offset: usize| {
        data.get(offset..offset + 2)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u16::from_le_bytes)
    };
    let u32_at = |offset: usize| {
        data.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let i32_at = |offset: usize| {
        data.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(i32::from_le_bytes)
    };
    let header = usize::try_from(u32_at(0).ok_or(ClipboardBridgeError::InvalidImage)?)
        .map_err(|_| ClipboardBridgeError::InvalidImage)?;
    if !matches!(header, 40 | 108 | 124)
        || format == Some(DIBV5_FORMAT.id()) && header != 124
        || header > data.len()
        || u16_at(12) != Some(1)
    {
        return Err(ClipboardBridgeError::InvalidImage);
    }
    let width = i32_at(4)
        .filter(|width| *width > 0)
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    let height = i32_at(8)
        .filter(|height| *height != 0 && *height != i32::MIN)
        .ok_or(ClipboardBridgeError::InvalidImage)?
        .unsigned_abs();
    let bits = u16_at(14)
        .filter(|bits| matches!(bits, 24 | 32))
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    let compression = u32_at(16).ok_or(ClipboardBridgeError::InvalidImage)?;
    if !matches!(compression, 0 | 3)
        // BI_BITFIELDS needs channel masks after the 40-byte
        // BITMAPINFOHEADER. This validator treats `header` as the complete
        // pixel-data offset, so accepting a 40-byte bitfield header would
        // interpret mask bytes (or absent bytes) as pixels.
        || compression == 3 && header < 52
    {
        return Err(ClipboardBridgeError::InvalidImage);
    }
    let row_bits = u64::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(u64::from(bits)))
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    let row_bytes = row_bits
        .checked_add(31)
        .map(|value| value / 32 * 4)
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    let pixels = row_bytes
        .checked_mul(u64::from(height))
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    let total = u64::try_from(header)
        .ok()
        .and_then(|header| header.checked_add(pixels))
        .ok_or(ClipboardBridgeError::InvalidImage)?;
    if total != data.len() as u64 || total > MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES {
        return Err(ClipboardBridgeError::InvalidImage);
    }
    let declared = u32_at(20).ok_or(ClipboardBridgeError::InvalidImage)?;
    if declared != 0 && u64::from(declared) != pixels {
        return Err(ClipboardBridgeError::InvalidImage);
    }
    Ok(())
}

fn encode_cf_html(fragment: &str) -> Vec<u8> {
    // Ten decimal digits cover the bounded payload and keep the header width
    // stable while calculating its byte offsets.
    let empty_header = cf_html_header(0, 0, 0, 0);
    let start_html = empty_header.len();
    let start_fragment = start_html + CF_HTML_PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + CF_HTML_SUFFIX.len();
    let header = cf_html_header(start_html, end_html, start_fragment, end_fragment);
    debug_assert_eq!(header.len(), start_html);

    let mut wire = Vec::with_capacity(end_html);
    wire.extend_from_slice(header.as_bytes());
    wire.extend_from_slice(CF_HTML_PREFIX.as_bytes());
    wire.extend_from_slice(fragment.as_bytes());
    wire.extend_from_slice(CF_HTML_SUFFIX.as_bytes());
    wire
}

fn cf_html_header(
    start_html: usize,
    end_html: usize,
    start_fragment: usize,
    end_fragment: usize,
) -> String {
    format!(
        "Version:1.0\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    )
}

fn decode_cf_html(data: &[u8]) -> Option<String> {
    if data.len() > MAX_VDI_CLIPBOARD_TEXT_BYTES.saturating_add(CF_HTML_HEADER_SLACK_BYTES) {
        return None;
    }
    let probe_len = data.len().min(CF_HTML_HEADER_SLACK_BYTES);
    let probe = std::str::from_utf8(&data[..probe_len]).ok()?;
    if !probe.starts_with("Version:") {
        return None;
    }
    let parse_offset = |header: &str, name: &str| {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name))?
            .trim()
            .parse::<usize>()
            .ok()
    };
    let start_html = parse_offset(probe, "StartHTML:")?;
    if start_html == 0 || start_html > probe_len {
        return None;
    }
    let header = std::str::from_utf8(&data[..start_html]).ok()?;
    let end_html = parse_offset(header, "EndHTML:")?;
    let start_fragment = parse_offset(header, "StartFragment:")?;
    let end_fragment = parse_offset(header, "EndFragment:")?;
    if start_html > end_html
        || start_html > start_fragment
        || start_fragment > end_fragment
        || end_fragment > end_html
        || end_html > data.len()
        || end_fragment - start_fragment > MAX_VDI_CLIPBOARD_TEXT_BYTES
    {
        return None;
    }
    std::str::from_utf8(&data[start_fragment..end_fragment])
        .ok()
        .map(str::to_owned)
}

/// Refuse active guest HTML before it enters the host/mesh clipboard lane.
///
/// CF_HTML's offsets and byte cap protect the transport, but they do not make
/// the fragment inert. Keep ordinary rich formatting while refusing elements,
/// event-handler attributes, and URL schemes that can execute or navigate when
/// a downstream host clipboard consumer renders the guest-originated value.
fn guest_html_fragment_is_safe(fragment: &str) -> bool {
    let fragment = fragment.to_ascii_lowercase();
    let mut remainder = fragment.as_str();
    while let Some((_, after_open)) = remainder.split_once('<') {
        let Some((tag, after_tag)) = after_open.split_once('>') else {
            return false;
        };
        let tag = tag.trim_start().trim_start_matches('/').trim_start();
        let name_end = tag
            .find(|character: char| character.is_ascii_whitespace() || character == '/')
            .unwrap_or(tag.len());
        let name = &tag[..name_end];
        if matches!(
            name,
            "base"
                | "embed"
                | "form"
                | "iframe"
                | "link"
                | "meta"
                | "object"
                | "script"
                | "style"
                | "svg"
        ) {
            return false;
        }

        let attributes = &tag[name_end..];
        // Resource URLs are active content too: a downstream clipboard
        // renderer may dereference them even when the surrounding markup has
        // no script tag or event handler.  Keep the guest value inert at this
        // boundary; in particular, data: can smuggle HTML/SVG and file: can
        // expose host-local paths when pasted into a capable consumer.
        if attributes.contains("javascript:")
            || attributes.contains("vbscript:")
            || attributes.contains("data:")
            || attributes.contains("file:")
        {
            return false;
        }
        let mut attribute = attributes;
        while let Some(start) = attribute.find(|character: char| character.is_ascii_alphabetic()) {
            attribute = &attribute[start..];
            let end = attribute
                .find(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .unwrap_or(attribute.len());
            let name = &attribute[..end];
            let after_name = &attribute[end..];
            if name.starts_with("on") && name.len() > 2 && after_name.trim_start().starts_with('=')
            {
                return false;
            }
            attribute = after_name;
        }
        remainder = after_tag;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        decode_cf_html, decode_unicode_text, encode_cf_html, guest_html_fragment_is_safe,
        html_format, ClipboardBridge, ClipboardBridgeError, RemoteClipboardImageFormat,
        DIBV5_FORMAT, DIB_FORMAT, HTML_FORMAT_ID, UNICODE_TEXT_FORMAT,
    };
    use ironrdp_cliprdr::pdu::{
        ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
        FileContentsFlags, FileContentsRequest, FileContentsResponse, FileDescriptor,
        FormatDataRequest, FormatDataResponse, LockDataId,
    };
    use mackes_mesh_types::vdi_clipboard::MAX_VDI_CLIPBOARD_TEXT_BYTES;

    fn one_pixel_dibv5() -> Vec<u8> {
        let mut dib = vec![0_u8; 124 + 4];
        dib[0..4].copy_from_slice(&124_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1_i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3_u32.to_le_bytes());
        dib[20..24].copy_from_slice(&4_u32.to_le_bytes());
        dib[40..44].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
        dib[44..48].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
        dib[48..52].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
        dib[52..56].copy_from_slice(&0xff00_0000_u32.to_le_bytes());
        dib[124..128].copy_from_slice(&[0x33, 0x22, 0x11, 0xff]);
        dib
    }

    fn one_pixel_dib() -> Vec<u8> {
        let mut dib = vec![0_u8; 40 + 4];
        dib[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1_i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib[20..24].copy_from_slice(&4_u32.to_le_bytes());
        dib[40..44].copy_from_slice(&[0x33, 0x22, 0x11, 0xff]);
        dib
    }

    #[test]
    fn bridge_bounds_host_text_and_decodes_guest_unicode() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        bridge
            .offer_host_text("hello".into())
            .expect("bounded text");
        assert_eq!(
            decode_unicode_text(&[b'h', 0, b'i', 0, 0, 0]).as_deref(),
            Some("hi")
        );
        let oversized = "x".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES + 1);
        assert_eq!(
            bridge.offer_host_text(oversized),
            Err(ClipboardBridgeError::TooLarge {
                bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES + 1,
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            })
        );

        backend.on_remote_copy(&[UNICODE_TEXT_FORMAT]);
        assert_eq!(
            bridge.take_remote_format_request(),
            Some(ClipboardFormatId::CF_UNICODETEXT)
        );
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        assert_eq!(
            bridge.take_local_data_response().expect("response").data(),
            FormatDataResponse::new_unicode_string("hello").data()
        );
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("guest"));
        assert_eq!(bridge.take_remote_text().as_deref(), Some("guest"));
    }

    #[test]
    fn rich_html_negotiation_round_trips_registered_cf_html() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let remote_html_id = ClipboardFormatId::new(0xC123);
        let remote_html = ClipboardFormat::new(remote_html_id).with_name(ClipboardFormatName::HTML);
        backend.on_remote_copy(&[remote_html]);
        assert_eq!(bridge.take_remote_format_request(), Some(remote_html_id));

        let guest_wire = encode_cf_html("<b>guest</b>");
        backend.on_format_data_response(FormatDataResponse::new_data(guest_wire));
        assert_eq!(bridge.take_remote_html().as_deref(), Some("<b>guest</b>"));

        bridge
            .offer_host_html("<em>host</em>".into())
            .expect("bounded HTML");
        assert_eq!(bridge.advertised_formats(), vec![html_format()]);
        backend.on_format_data_request(FormatDataRequest {
            format: HTML_FORMAT_ID,
        });
        let response = bridge.take_local_data_response().expect("HTML response");
        assert_eq!(
            decode_cf_html(response.data()).as_deref(),
            Some("<em>host</em>")
        );
    }

    #[test]
    fn rich_html_admission_is_bounded_in_both_directions() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let oversized = "x".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES + 1);
        assert_eq!(
            bridge.offer_host_html(oversized),
            Err(ClipboardBridgeError::TooLarge {
                bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES + 1,
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            })
        );

        backend.on_remote_copy(&[html_format()]);
        assert_eq!(bridge.take_remote_format_request(), Some(HTML_FORMAT_ID));
        backend.on_format_data_response(FormatDataResponse::new_data(vec![
            b'x';
            MAX_VDI_CLIPBOARD_TEXT_BYTES + super::CF_HTML_HEADER_SLACK_BYTES
                + 1
        ]));
        assert_eq!(bridge.take_remote_html(), None);
    }

    #[test]
    fn guest_html_active_content_is_refused_before_host_publication() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_remote_copy(&[html_format()]);
        assert_eq!(bridge.take_remote_format_request(), Some(HTML_FORMAT_ID));

        let hostile =
            encode_cf_html(r#"<div onclick="javascript:alert(1)"><script>alert(1)</script></div>"#);
        backend.on_format_data_response(FormatDataResponse::new_data(hostile));

        assert_eq!(bridge.take_remote_html(), None);
        assert!(!guest_html_fragment_is_safe(
            r#"<div onclick="javascript:alert(1)"><script>alert(1)</script></div>"#
        ));
        assert!(guest_html_fragment_is_safe(
            "<p><strong>safe</strong> guest</p>"
        ));
    }

    #[test]
    fn guest_html_resource_urls_are_refused_before_host_publication() {
        for fragment in [
            r#"<img src="data:image/svg+xml,<svg onload=alert(1)>" />"#,
            r#"<a href="file:///etc/passwd">local file</a>"#,
        ] {
            let (bridge, mut backend) = ClipboardBridge::pair();
            backend.on_remote_copy(&[html_format()]);
            assert_eq!(bridge.take_remote_format_request(), Some(HTML_FORMAT_ID));

            backend.on_format_data_response(FormatDataResponse::new_data(encode_cf_html(fragment)));
            assert_eq!(
                bridge.take_remote_html(),
                None,
                "active resource URL must not cross the guest boundary"
            );
        }
    }

    #[test]
    fn guest_html_registered_format_identity_equivocation_is_refused() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let first_id = ClipboardFormatId(0xC101);
        let second_id = ClipboardFormatId(0xC102);
        let first = ClipboardFormat::new(first_id).with_name(ClipboardFormatName::HTML);
        let second = ClipboardFormat::new(second_id).with_name(ClipboardFormatName::HTML);

        backend.on_remote_copy(&[first.clone(), second]);
        assert_eq!(
            bridge.take_remote_format_request(),
            None,
            "one registered MIME name cannot authorize two wire identities"
        );

        backend.on_remote_copy(&[first.clone(), ClipboardFormat::new(first_id)]);
        assert_eq!(
            bridge.take_remote_format_request(),
            None,
            "one wire identity cannot simultaneously mean HTML and unnamed data"
        );

        backend.on_remote_copy(&[first.clone(), first]);
        assert_eq!(
            bridge.take_remote_format_request(),
            Some(first_id),
            "an identical duplicate is not an equivocation"
        );
        backend.on_format_data_response(FormatDataResponse::new_data(encode_cf_html(
            "<strong>exact</strong>",
        )));
        assert_eq!(
            bridge.take_remote_html().as_deref(),
            Some("<strong>exact</strong>")
        );
    }

    #[test]
    fn replacement_and_unsupported_formats_refuse_stale_callbacks() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_remote_copy(&[UNICODE_TEXT_FORMAT]);
        assert_eq!(
            bridge.take_remote_format_request(),
            Some(ClipboardFormatId::CF_UNICODETEXT)
        );

        backend.on_remote_copy(&[html_format()]);
        assert_eq!(bridge.take_remote_format_request(), None);
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("stale"));
        assert_eq!(bridge.take_remote_text(), None);
        assert_eq!(bridge.take_remote_format_request(), Some(HTML_FORMAT_ID));
        backend.on_format_data_response(FormatDataResponse::new_error());
        assert_eq!(bridge.take_remote_html(), None);

        let falsely_standard_html =
            ClipboardFormat::new(ClipboardFormatId::CF_TEXT).with_name(ClipboardFormatName::HTML);
        backend.on_remote_copy(&[falsely_standard_html]);
        assert_eq!(bridge.take_remote_format_request(), None);

        bridge.offer_host_text("old".into()).expect("text");
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        bridge
            .offer_host_html("new".into())
            .expect("replacement HTML");
        assert!(bridge
            .take_local_data_response()
            .expect("fail-closed response")
            .is_error());
    }

    #[test]
    fn duplicate_remote_response_cannot_erase_admitted_clipboard() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_remote_copy(&[UNICODE_TEXT_FORMAT]);
        assert_eq!(
            bridge.take_remote_format_request(),
            Some(ClipboardFormatId::CF_UNICODETEXT)
        );

        backend.on_format_data_response(FormatDataResponse::new_unicode_string("admitted"));
        // CLIPRDR supplies no response nonce, so a duplicate callback can
        // arrive before the accepted value has been consumed by the caller.
        // It is not authorized to clear that successful transfer.
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("replay"));
        assert_eq!(bridge.take_remote_text().as_deref(), Some("admitted"));
    }

    #[test]
    fn rejected_oversized_replacement_revokes_stale_host_clipboard_authority() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        bridge
            .offer_host_text("previous secret".into())
            .expect("bounded initial offer");
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });

        let oversized = "x".repeat(MAX_VDI_CLIPBOARD_TEXT_BYTES + 1);
        assert!(matches!(
            bridge.offer_host_html(oversized),
            Err(ClipboardBridgeError::TooLarge { .. })
        ));

        assert_eq!(bridge.advertised_formats(), Vec::<ClipboardFormat>::new());
        assert!(bridge
            .take_local_data_response()
            .expect("queued stale request must receive a response")
            .is_error());
    }

    #[test]
    fn stale_request_cannot_read_replacement_before_its_generation_is_advertised() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        bridge.offer_host_text("old".into()).expect("initial offer");
        assert_eq!(bridge.advertised_formats(), vec![UNICODE_TEXT_FORMAT]);

        bridge
            .offer_host_text("replacement secret".into())
            .expect("replacement offer");
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        assert!(bridge
            .take_local_data_response()
            .expect("stale request must receive a response")
            .is_error());

        assert_eq!(bridge.advertised_formats(), vec![UNICODE_TEXT_FORMAT]);
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        assert_eq!(
            bridge
                .take_local_data_response()
                .expect("current request")
                .data(),
            FormatDataResponse::new_unicode_string("replacement secret").data()
        );
    }

    #[test]
    fn bounded_dibv5_negotiation_round_trips_and_rejects_hostile_geometry() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let dib = one_pixel_dibv5();
        bridge.offer_host_dibv5(dib.clone()).expect("bounded DIBV5");
        assert_eq!(bridge.advertised_formats(), vec![DIBV5_FORMAT, DIB_FORMAT]);
        backend.on_format_data_request(FormatDataRequest {
            format: DIBV5_FORMAT.id(),
        });
        assert_eq!(
            bridge
                .take_local_data_response()
                .expect("DIB response")
                .data(),
            dib
        );

        let mut hostile = one_pixel_dibv5();
        hostile[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert_eq!(
            bridge.offer_host_dibv5(hostile),
            Err(ClipboardBridgeError::InvalidImage)
        );

        let mut missing_bitfield_masks = vec![0_u8; 40 + 4];
        missing_bitfield_masks[0..4].copy_from_slice(&40_u32.to_le_bytes());
        missing_bitfield_masks[4..8].copy_from_slice(&1_i32.to_le_bytes());
        missing_bitfield_masks[8..12].copy_from_slice(&(-1_i32).to_le_bytes());
        missing_bitfield_masks[12..14].copy_from_slice(&1_u16.to_le_bytes());
        missing_bitfield_masks[14..16].copy_from_slice(&32_u16.to_le_bytes());
        missing_bitfield_masks[16..20].copy_from_slice(&3_u32.to_le_bytes());
        missing_bitfield_masks[20..24].copy_from_slice(&4_u32.to_le_bytes());
        assert_eq!(
            super::validate_dib(&missing_bitfield_masks, Some(DIB_FORMAT.id())),
            Err(ClipboardBridgeError::InvalidImage)
        );
    }

    #[test]
    fn guest_dib_and_dibv5_are_admitted_as_typed_one_use_images() {
        for (format, wire, expected_format) in [
            (DIB_FORMAT, one_pixel_dib(), RemoteClipboardImageFormat::Dib),
            (
                DIBV5_FORMAT,
                one_pixel_dibv5(),
                RemoteClipboardImageFormat::DibV5,
            ),
        ] {
            let (bridge, mut backend) = ClipboardBridge::pair();
            let format_id = format.id();
            backend.on_remote_copy(&[format]);
            assert_eq!(bridge.take_remote_format_request(), Some(format_id));

            backend.on_format_data_response(FormatDataResponse::new_data(wire));
            let admitted = bridge.take_remote_image().expect("typed guest image");
            assert_eq!(admitted.format(), expected_format);
            assert_eq!(
                admitted.data(),
                match expected_format {
                    RemoteClipboardImageFormat::Dib => one_pixel_dib(),
                    RemoteClipboardImageFormat::DibV5 => one_pixel_dibv5(),
                }
            );
            assert_eq!(bridge.take_remote_image(), None);
            assert_eq!(bridge.take_remote_format_request(), None);
        }

        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_remote_copy(&[DIB_FORMAT, DIBV5_FORMAT]);
        assert_eq!(
            bridge.take_remote_format_request(),
            Some(DIBV5_FORMAT.id()),
            "prefer the stronger V5 representation independent of offer order"
        );
    }

    #[test]
    fn host_dibv5_offer_cannot_be_requested_as_classic_dib() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        bridge
            .offer_host_dibv5(one_pixel_dibv5())
            .expect("valid V5 offer");
        assert_eq!(bridge.advertised_formats(), vec![DIBV5_FORMAT]);

        backend.on_format_data_request(FormatDataRequest {
            format: DIB_FORMAT.id(),
        });
        assert!(
            bridge
                .take_local_data_response()
                .expect("refusal response")
                .is_error(),
            "a V5 payload must never be served under the classic DIB format"
        );
    }

    #[test]
    fn guest_image_format_confusion_replacement_and_replay_fail_closed() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_remote_copy(&[DIBV5_FORMAT]);
        assert_eq!(bridge.take_remote_format_request(), Some(DIBV5_FORMAT.id()));

        // A structurally valid classic DIB is not valid for the negotiated V5
        // request, even though both formats carry bitmap bytes.
        backend.on_format_data_response(FormatDataResponse::new_data(one_pixel_dib()));
        assert_eq!(bridge.take_remote_image(), None);

        backend.on_remote_copy(&[DIB_FORMAT]);
        assert_eq!(bridge.take_remote_format_request(), Some(DIB_FORMAT.id()));
        backend.on_remote_copy(&[DIBV5_FORMAT]);
        assert_eq!(bridge.take_remote_format_request(), None);
        backend.on_format_data_response(FormatDataResponse::new_data(one_pixel_dib()));
        assert_eq!(bridge.take_remote_image(), None, "stale response refused");
        assert_eq!(bridge.take_remote_format_request(), Some(DIBV5_FORMAT.id()));

        let admitted = one_pixel_dibv5();
        backend.on_format_data_response(FormatDataResponse::new_data(admitted.clone()));
        backend.on_format_data_response(FormatDataResponse::new_data(one_pixel_dibv5()));
        assert_eq!(
            bridge
                .take_remote_image()
                .map(|image| (image.format(), image.data().to_vec())),
            Some((RemoteClipboardImageFormat::DibV5, admitted)),
            "unsolicited replay cannot replace the admitted image"
        );
    }

    #[test]
    fn guest_file_list_is_format_bound_bounded_and_lock_scoped() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let file_format_id = ClipboardFormatId(0xC321);
        let file_format =
            ClipboardFormat::new(file_format_id).with_name(ClipboardFormatName::FILE_LIST);

        backend.on_remote_copy(&[file_format.clone()]);
        assert_eq!(bridge.take_remote_format_request(), Some(file_format_id));
        backend.on_remote_file_list(
            &[
                FileDescriptor::new("report.pdf").with_file_size(12_345),
                FileDescriptor::new("chart.png")
                    .with_relative_path("quarter-1\\figures")
                    .with_file_size(54_321),
            ],
            Some(77),
        );
        let admitted = bridge
            .take_remote_file_list()
            .expect("file-list callback")
            .expect("bounded metadata");
        assert_eq!(admitted.clip_data_id(), Some(77));
        assert_eq!(admitted.files()[0].name(), "report.pdf");
        assert_eq!(
            admitted.files()[1].relative_path(),
            Some("quarter-1\\figures")
        );
        assert_eq!(admitted.files()[1].size(), 54_321);

        backend.on_remote_copy(&[file_format.clone()]);
        assert_eq!(bridge.take_remote_format_request(), Some(file_format_id));
        backend.on_remote_file_list(
            &[FileDescriptor::new("escape.txt")
                .with_relative_path("..\\host")
                .with_file_size(1)],
            Some(78),
        );
        assert_eq!(
            bridge.take_remote_file_list(),
            Some(Err(ClipboardBridgeError::InvalidFileList))
        );

        backend.on_remote_copy(&[file_format.clone()]);
        assert_eq!(bridge.take_remote_format_request(), Some(file_format_id));
        backend.on_remote_file_list(
            &[
                FileDescriptor::new("first.bin").with_file_size(3 * 1024 * 1024 * 1024),
                FileDescriptor::new("second.bin").with_file_size(2 * 1024 * 1024 * 1024),
            ],
            Some(79),
        );
        assert_eq!(
            bridge.take_remote_file_list(),
            Some(Err(ClipboardBridgeError::InvalidFileList))
        );

        backend.on_remote_copy(&[file_format.clone()]);
        assert_eq!(bridge.take_remote_format_request(), Some(file_format_id));
        backend.on_remote_file_list(
            &[FileDescriptor::new("locked.txt").with_file_size(9)],
            Some(80),
        );
        backend.on_outgoing_locks_cleared(&[LockDataId(80)]);
        assert_eq!(
            bridge.take_remote_file_list(),
            None,
            "expired CLIPRDR lock must revoke its file-list snapshot"
        );

        let equivocated = ClipboardFormat::new(file_format_id);
        backend.on_remote_copy(&[file_format, equivocated]);
        assert_eq!(
            bridge.take_remote_format_request(),
            None,
            "one registered ID cannot mean both files and unnamed bytes"
        );
    }

    #[test]
    fn guest_file_retrieval_is_sequential_chunked_and_snapshot_bound() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        let file_format_id = ClipboardFormatId(0xC322);
        backend.on_remote_copy(&[
            ClipboardFormat::new(file_format_id).with_name(ClipboardFormatName::FILE_LIST)
        ]);
        assert_eq!(bridge.take_remote_format_request(), Some(file_format_id));
        backend.on_remote_file_list(
            &[FileDescriptor::new("recording.bin").with_file_size(300_000)],
            Some(91),
        );
        bridge
            .take_remote_file_list()
            .expect("metadata callback")
            .expect("admitted metadata");

        bridge.begin_remote_file_retrieval(0).expect("start file");
        let first = bridge
            .take_remote_file_contents_request()
            .expect("first range");
        assert_eq!(first.index, 0);
        assert_eq!(first.flags, FileContentsFlags::RANGE);
        assert_eq!(first.position, 0);
        assert_eq!(first.requested_size, 256 * 1024);
        assert_eq!(first.data_id, Some(91));
        assert_eq!(bridge.take_remote_file_contents_request(), None);

        backend.on_file_contents_response(FileContentsResponse::new_data_response(
            first.stream_id,
            vec![0xA5; 100_000],
        ));
        let first_chunk = bridge
            .take_remote_file_chunk()
            .expect("first callback")
            .expect("valid first range");
        assert_eq!(first_chunk.file_index(), 0);
        assert_eq!(first_chunk.offset(), 0);
        assert_eq!(first_chunk.data().len(), 100_000);
        assert!(!first_chunk.is_complete());

        let second = bridge
            .take_remote_file_contents_request()
            .expect("tail range");
        assert_eq!(second.position, 100_000);
        assert_eq!(second.requested_size, 200_000);
        backend.on_file_contents_response(FileContentsResponse::new_data_response(
            second.stream_id,
            vec![0x5A; second.requested_size as usize],
        ));
        let tail = bridge
            .take_remote_file_chunk()
            .expect("tail callback")
            .expect("valid tail");
        assert_eq!(tail.offset(), 100_000);
        assert!(tail.is_complete());
        assert_eq!(bridge.take_remote_file_contents_request(), None);

        bridge.begin_remote_file_retrieval(0).expect("restart file");
        let replay = bridge
            .take_remote_file_contents_request()
            .expect("fresh range");
        backend.on_file_contents_response(FileContentsResponse::new_data_response(
            replay.stream_id.wrapping_add(1),
            vec![0; replay.requested_size as usize],
        ));
        assert_eq!(
            bridge.take_remote_file_chunk(),
            Some(Err(ClipboardBridgeError::InvalidFileTransfer))
        );
        assert_eq!(bridge.take_remote_file_contents_request(), None);
    }

    #[test]
    fn host_file_serving_is_permission_bounded_range_bound_and_cancelled() {
        let (bridge, mut backend) = ClipboardBridge::pair();
        backend.on_ready();
        backend.on_process_negotiated_capabilities(
            ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
                | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA,
        );
        let descriptor = bridge
            .offer_host_file("quarterly-report.pdf".into(), b"governed-bytes".to_vec())
            .expect("permission-approved Files descriptor");
        assert_eq!(descriptor.file_size, Some(14));
        assert_eq!(descriptor.name, "quarterly-report.pdf");

        backend.on_lock(LockDataId(41));
        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 7,
            index: 0,
            flags: FileContentsFlags::SIZE,
            position: 0,
            requested_size: 8,
            data_id: Some(41),
        });
        let size = bridge.take_local_file_response().expect("size response");
        assert_eq!(size.data_as_size().expect("u64 size"), 14);

        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 71,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 4,
            data_id: None,
        });
        assert!(
            bridge
                .take_local_file_response()
                .expect("unbound response")
                .is_error(),
            "a negotiated file stream must never bypass its lock identity"
        );

        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 8,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 9,
            requested_size: 5,
            data_id: Some(41),
        });
        assert_eq!(
            bridge
                .take_local_file_response()
                .expect("range response")
                .data(),
            b"bytes"
        );

        bridge
            .offer_host_file("replacement.png".into(), b"replacement".to_vec())
            .expect("replacement offer");
        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 9,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 8,
            data_id: Some(41),
        });
        assert_eq!(
            bridge
                .take_local_file_response()
                .expect("locked snapshot response")
                .data(),
            b"governed"
        );

        assert_eq!(
            bridge.offer_host_file("../outside.txt".into(), b"must-not-serve".to_vec()),
            Err(ClipboardBridgeError::InvalidLocalFile),
            "an invalid replacement must revoke every prior file authority"
        );
        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 10,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 8,
            data_id: Some(41),
        });
        assert!(
            bridge
                .take_local_file_response()
                .expect("revocation response")
                .is_error(),
            "a rejected replacement must destroy the locked prior snapshot"
        );

        backend.on_unlock(LockDataId(41));
        backend.on_file_contents_request(FileContentsRequest {
            stream_id: 11,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 8,
            data_id: Some(41),
        });
        assert!(
            bridge
                .take_local_file_response()
                .expect("cancel response")
                .is_error(),
            "unlock must destroy the prior delayed-rendering authority"
        );
    }
}
