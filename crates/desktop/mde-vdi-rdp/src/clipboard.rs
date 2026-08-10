//! Bounded Unicode-text and CF_HTML CLIPRDR backend for the live RDP transport.

use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest,
    FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp_core::impl_as_any;
use mackes_mesh_types::vdi_clipboard::{
    MAX_VDI_CLIPBOARD_TEXT_BYTES, MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES,
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
}

impl RemoteFormat {
    const fn id(self) -> ClipboardFormatId {
        match self {
            Self::UnicodeText => ClipboardFormatId::CF_UNICODETEXT,
            Self::Html(id) => id,
        }
    }
}

#[derive(Debug, Default)]
struct ClipboardState {
    ready: bool,
    initial_format_list_requested: bool,
    local_generation: u64,
    local_text: Option<String>,
    local_html: Option<Vec<u8>>,
    local_dib: Option<Vec<u8>>,
    local_data_request: Option<(FormatDataRequest, u64)>,
    remote_unicode_offer: Option<RemoteFormat>,
    remote_html_offer: Option<RemoteFormat>,
    pending_remote_request: Option<RemoteFormat>,
    discard_replaced_response: bool,
    remote_text: Option<String>,
    remote_html: Option<String>,
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
            return Err(ClipboardBridgeError::TooLarge {
                bytes: html.len(),
                max_bytes: MAX_VDI_CLIPBOARD_TEXT_BYTES,
            });
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
        validate_dib(&dib, Some(DIBV5_FORMAT.id()))?;
        let mut state = self.lock();
        state.local_generation = state.local_generation.wrapping_add(1);
        state.local_text = None;
        state.local_html = None;
        state.local_dib = Some(dib);
        Ok(())
    }

    /// Return only formats backed by the current local offer.
    #[must_use]
    pub fn advertised_formats(&self) -> Vec<ClipboardFormat> {
        let state = self.lock();
        if state.local_text.is_some() {
            vec![UNICODE_TEXT_FORMAT]
        } else if state.local_html.is_some() {
            vec![html_format()]
        } else if state.local_dib.is_some() {
            vec![DIBV5_FORMAT, DIB_FORMAT]
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
        if requested_generation != state.local_generation {
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
        } else if request.format == DIBV5_FORMAT.id() || request.format == DIB_FORMAT.id() {
            match state.local_dib.as_ref() {
                Some(dib) => OwnedFormatDataResponse::new_data(dib.clone()),
                None => OwnedFormatDataResponse::new_error(),
            }
        } else {
            OwnedFormatDataResponse::new_error()
        })
    }

    /// Take the next truthfully negotiated remote format and bind the eventual
    /// callback to it. Unicode remains first so existing consumers retain their
    /// plain-text path when a peer advertises both formats.
    pub fn take_remote_format_request(&self) -> Option<ClipboardFormatId> {
        let mut state = self.lock();
        if state.pending_remote_request.is_some() || state.discard_replaced_response {
            return None;
        }
        let format = state
            .remote_unicode_offer
            .take()
            .or_else(|| state.remote_html_offer.take())?;
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
    /// UTF-8 text or HTML exceeded the canonical VDI clipboard limit.
    TooLarge {
        /// Rejected UTF-8 byte count.
        bytes: usize,
        /// Canonical maximum UTF-8 byte count.
        max_bytes: usize,
    },
    /// The image was not a bounded, self-consistent CF_DIB/CF_DIBV5 payload.
    InvalidImage,
}

impl core::fmt::Display for ClipboardBridgeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge { bytes, max_bytes } => write!(
                formatter,
                "RDP clipboard payload is {bytes} bytes; maximum is {max_bytes}"
            ),
            Self::InvalidImage => formatter.write_str("RDP clipboard DIB is malformed or unsafe"),
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
        self.with_state(|state| {
            if state.pending_remote_request.take().is_some() {
                // CLIPRDR responses carry no request ID. Consume and refuse the
                // old response before issuing a request against the new offer.
                state.discard_replaced_response = true;
            }
            state.remote_unicode_offer = None;
            state.remote_html_offer = None;
            state.remote_text = None;
            state.remote_html = None;

            if available_formats.len() > MAX_REMOTE_FORMATS {
                return;
            }
            state.remote_unicode_offer = available_formats
                .iter()
                .any(|format| format.id() == ClipboardFormatId::CF_UNICODETEXT)
                .then_some(RemoteFormat::UnicodeText);
            state.remote_html_offer = available_formats.iter().find_map(|format| {
                (format.id().is_registered()
                    && format
                        .name()
                        .is_some_and(|name| name.value() == ClipboardFormatName::HTML.value()))
                .then_some(RemoteFormat::Html(format.id()))
            });
        });
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.with_state(|state| {
            state.local_data_request = Some((request, state.local_generation));
        });
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        self.with_state(|state| {
            if state.discard_replaced_response {
                state.discard_replaced_response = false;
                return;
            }
            let Some(format) = state.pending_remote_request.take() else {
                state.remote_text = None;
                state.remote_html = None;
                return;
            };
            if response.is_error() {
                match format {
                    RemoteFormat::UnicodeText => state.remote_text = None,
                    RemoteFormat::Html(_) => state.remote_html = None,
                }
                return;
            }
            match format {
                RemoteFormat::UnicodeText => {
                    state.remote_text = decode_unicode_text(response.data())
                }
                RemoteFormat::Html(_) => state.remote_html = decode_cf_html(response.data()),
            }
        });
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
    if !matches!(compression, 0 | 3) {
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

#[cfg(test)]
mod tests {
    use super::{
        decode_cf_html, decode_unicode_text, encode_cf_html, html_format, ClipboardBridge,
        ClipboardBridgeError, DIBV5_FORMAT, DIB_FORMAT, HTML_FORMAT_ID, UNICODE_TEXT_FORMAT,
    };
    use ironrdp_cliprdr::pdu::{
        ClipboardFormat, ClipboardFormatId, ClipboardFormatName, FormatDataRequest,
        FormatDataResponse,
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

        // Guest-to-host image publication still needs a Files write authority;
        // do not request image bytes merely because the guest advertises them.
        backend.on_remote_copy(&[DIB_FORMAT, DIBV5_FORMAT]);
        assert_eq!(bridge.take_remote_format_request(), None);

        let mut hostile = one_pixel_dibv5();
        hostile[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert_eq!(
            bridge.offer_host_dibv5(hostile),
            Err(ClipboardBridgeError::InvalidImage)
        );
    }
}
