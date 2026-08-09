//! Bounded text-only CLIPRDR backend for the live RDP transport.

use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_core::impl_as_any;
use mackes_mesh_types::vdi_clipboard::MAX_VDI_CLIPBOARD_TEXT_BYTES;

/// The one CLIPRDR format this backend truthfully supports.
pub const UNICODE_TEXT_FORMAT: ClipboardFormat =
    ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT);

#[derive(Debug, Default)]
struct ClipboardState {
    ready: bool,
    initial_format_list_requested: bool,
    local_text: Option<String>,
    local_data_request: Option<FormatDataRequest>,
    remote_unicode_offer: bool,
    remote_text: Option<String>,
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
            return Err(ClipboardBridgeError::TooLarge {
                bytes: text.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            });
        }
        self.lock().local_text = Some(text);
        Ok(())
    }

    /// Whether IronRDP requested the initial local format list.
    pub fn take_initial_format_list_request(&self) -> bool {
        std::mem::take(&mut self.lock().initial_format_list_requested)
    }

    /// Take a server request for the currently offered host data.
    pub fn take_local_data_response(&self) -> Option<OwnedFormatDataResponse> {
        let mut state = self.lock();
        let request = state.local_data_request.take()?;
        if request.format != ClipboardFormatId::CF_UNICODETEXT {
            return Some(OwnedFormatDataResponse::new_error());
        }
        Some(match state.local_text.as_deref() {
            Some(text) => OwnedFormatDataResponse::new_unicode_string(text),
            None => OwnedFormatDataResponse::new_error(),
        })
    }

    /// Take the signal that the guest advertised Unicode text.
    pub fn take_remote_unicode_offer(&self) -> bool {
        std::mem::take(&mut self.lock().remote_unicode_offer)
    }

    /// Take the latest bounded guest text returned by CLIPRDR.
    pub fn take_remote_text(&self) -> Option<String> {
        self.lock().remote_text.take()
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
}

/// A bounded CLIPRDR admission failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBridgeError {
    /// UTF-8 text exceeded the canonical VDI clipboard limit.
    TooLarge {
        /// Rejected UTF-8 byte count.
        bytes: usize,
        /// Canonical maximum UTF-8 byte count.
        max_bytes: usize,
    },
}

impl core::fmt::Display for ClipboardBridgeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge { bytes, max_bytes } => write!(
                formatter,
                "RDP clipboard text is {bytes} bytes; maximum is {max_bytes}"
            ),
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
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {
        self.with_state(|state| state.ready = true);
    }

    fn on_request_format_list(&mut self) {
        self.with_state(|state| state.initial_format_list_requested = true);
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        _capabilities: ClipboardGeneralCapabilityFlags,
    ) {
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        let has_unicode = available_formats
            .iter()
            .any(|format| format.id() == ClipboardFormatId::CF_UNICODETEXT);
        self.with_state(|state| state.remote_unicode_offer = has_unicode);
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.with_state(|state| state.local_data_request = Some(request));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let decoded = (!response.is_error())
            .then(|| decode_unicode_text(response.data()))
            .flatten();
        self.with_state(|state| state.remote_text = decoded);
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}

    fn on_remote_file_list(&mut self, _files: &[FileDescriptor], _clip_data_id: Option<u32>) {}
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

#[cfg(test)]
mod tests {
    use super::{ClipboardBridge, ClipboardBridgeError, UNICODE_TEXT_FORMAT, decode_unicode_text};
    use ironrdp_cliprdr::pdu::{ClipboardFormatId, FormatDataRequest, FormatDataResponse};
    use mackes_mesh_types::vdi_clipboard::MAX_VDI_CLIPBOARD_TEXT_BYTES;

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
        assert!(bridge.take_remote_unicode_offer());
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
}
