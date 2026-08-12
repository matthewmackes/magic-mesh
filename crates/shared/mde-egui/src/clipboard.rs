//! Bounded clipboard authority shared by the bare-DRM runner and its clients.
//!
//! The direct seat owns one in-process offer/selection authority. Shell and VDI
//! adapters are clients: they may submit or observe offers, but never become a
//! second selection store. The authority uses the canonical Clipboard V2 MIME
//! and selection types while keeping transport work outside the render loop.

use mde_collab_types::{
    ClipboardClipId, ClipboardDenialReasonV2, ClipboardMimeKind, ClipboardMimeOfferV2,
    ClipboardPayloadV2, ClipboardSelectionDecisionV2, ClipboardSelectionV2, ClipboardSessionId,
    CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION, MAX_CLIPBOARD_OFFERS,
};

/// Maximum UTF-8 bytes in one local application/owner identity.
pub const MAX_CLIPBOARD_OWNER_BYTES: usize = 128;

/// Hard byte ceiling for one local text clipboard value.
///
/// This is the same 1 MiB ceiling used by the existing VDI clipboard relay. The
/// local seat truncates at a UTF-8 boundary before a provider sees the value, so
/// a provider cannot accidentally retain an unbounded platform output. The Bus
/// producer remains responsible for its existing content-id, source, timestamp,
/// and echo/dedup semantics.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
/// Aggregate inline bytes admitted in one local rich clipboard generation.
pub const MAX_CLIPBOARD_INLINE_TOTAL_BYTES: usize = 2 * 1024 * 1024;

/// One nonblocking update returned by a clipboard transport client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ClipboardClientPoll {
    /// No newer provider state is ready now.
    #[default]
    Unchanged,
    /// Replace the local offer with these richest-first canonical MIME offers.
    Offer(Vec<ClipboardMimeOfferV2>),
    /// The external provider explicitly lost ownership.
    Cleared,
}

/// Transport client used by the direct DRM clipboard authority.
///
/// Both methods must return immediately: implementations enqueue backend work
/// and use [`Self::poll_offer`] to deliver completed results on later frames.
/// They must not perform filesystem, network, guest, or Bus I/O inline.
pub trait RichClipboardClient {
    /// Poll one already-completed external provider update without waiting.
    fn poll_offer(&mut self) -> ClipboardClientPoll;

    /// Enqueue publication of the current local offer without waiting.
    fn publish_offer(&mut self, offer: &LocalClipboardOffer);

    /// Enqueue provider ownership release without waiting.
    fn clear_offer(&mut self);
}

/// Process-local nonblocking client used by DRM examples and focused tests.
#[derive(Debug, Default)]
pub struct MemoryRichClipboardClient {
    pending: ClipboardClientPoll,
    published: Option<LocalClipboardOffer>,
}

impl MemoryRichClipboardClient {
    /// Queue an external offer for the next nonblocking poll.
    pub fn queue_offer(&mut self, offers: Vec<ClipboardMimeOfferV2>) {
        self.pending = ClipboardClientPoll::Offer(offers);
    }

    /// Most recent offer published by the local DRM authority.
    #[must_use]
    pub const fn published(&self) -> Option<&LocalClipboardOffer> {
        self.published.as_ref()
    }
}

impl RichClipboardClient for MemoryRichClipboardClient {
    fn poll_offer(&mut self) -> ClipboardClientPoll {
        std::mem::take(&mut self.pending)
    }

    fn publish_offer(&mut self, offer: &LocalClipboardOffer) {
        self.published = Some(offer.clone());
    }

    fn clear_offer(&mut self) {
        self.published = None;
        self.pending = ClipboardClientPoll::Cleared;
    }
}

#[derive(Clone)]
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
struct DrmClipboardFocus(Option<String>);

fn drm_clipboard_focus_id() -> egui::Id {
    egui::Id::new("mde-egui.drm-clipboard-focus-v2")
}

/// Set the application/surface that owns clipboard output for this egui frame.
/// Passing `None` explicitly releases ownership. The DRM runner consumes this
/// marker after rendering, before admitting the frame's copy output.
pub fn set_drm_clipboard_owner(ctx: &egui::Context, owner: Option<&str>) {
    let owner = owner.map(str::to_owned);
    ctx.data_mut(|data| data.insert_temp(drm_clipboard_focus_id(), DrmClipboardFocus(owner)));
}

#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub(crate) fn take_drm_clipboard_owner(ctx: &egui::Context) -> Option<Option<String>> {
    ctx.data_mut(|data| {
        let id = drm_clipboard_focus_id();
        let owner = data.get_temp::<DrmClipboardFocus>(id).map(|focus| focus.0);
        data.remove::<DrmClipboardFocus>(id);
        owner
    })
}

/// The single bounded offer currently owned by the direct DRM seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalClipboardOffer {
    owner: String,
    clip_id: ClipboardClipId,
    session: ClipboardSessionId,
    generation: u64,
    offers: Vec<ClipboardMimeOfferV2>,
}

impl LocalClipboardOffer {
    /// Owner application/surface identity.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Monotonic seat-local ownership generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Canonical V2 clip identity used by selections.
    #[must_use]
    pub const fn clip_id(&self) -> ClipboardClipId {
        self.clip_id
    }

    /// Canonical V2 login-session identity used by selections.
    #[must_use]
    pub const fn session(&self) -> ClipboardSessionId {
        self.session
    }

    /// Richest-first canonical V2 MIME offers. Selecting one does not discard
    /// the others.
    #[must_use]
    pub fn offers(&self) -> &[ClipboardMimeOfferV2] {
        &self.offers
    }
}

/// Admission failures for a local DRM clipboard offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalClipboardError {
    /// The owner token is empty or exceeds the fixed identity bound.
    InvalidOwner,
    /// The offer set is empty, too large, duplicated, or intrinsically invalid.
    Denied(ClipboardDenialReasonV2),
}

/// One seat-local, generation-checked rich clipboard authority.
#[derive(Debug)]
pub struct LocalClipboardAuthority {
    focused_owner: Option<String>,
    session: ClipboardSessionId,
    next_generation: u64,
    current: Option<LocalClipboardOffer>,
}

impl Default for LocalClipboardAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalClipboardAuthority {
    /// Create an empty authority for one DRM login session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused_owner: None,
            session: ClipboardSessionId::new(),
            next_generation: 1,
            current: None,
        }
    }

    /// Current offer, if the focused owner still owns one.
    #[must_use]
    pub const fn current(&self) -> Option<&LocalClipboardOffer> {
        self.current.as_ref()
    }

    /// Move focus to one application/surface.
    ///
    /// Switching applications releases the previous owner's offer. Repeating
    /// the same owner is a no-op, so a normal render frame cannot churn the
    /// generation or invalidate its own selection.
    pub fn focus(&mut self, owner: &str) -> Result<bool, LocalClipboardError> {
        validate_owner(owner)?;
        if self.focused_owner.as_deref() == Some(owner) {
            return Ok(false);
        }
        self.focused_owner = Some(owner.to_owned());
        self.release_current();
        Ok(true)
    }

    /// Release clipboard ownership when the DRM seat/app loses focus.
    pub fn lose_focus(&mut self) -> bool {
        let changed = self.focused_owner.take().is_some() || self.current.is_some();
        if changed {
            self.release_current();
        }
        changed
    }

    /// Clear the current offer while retaining the focused application owner.
    pub fn clear(&mut self) -> bool {
        let changed = self.current.is_some();
        if changed {
            self.release_current();
        }
        changed
    }

    /// Replace the focused owner's offer after validating every canonical MIME
    /// representation and the fixed offer-count/duplicate bounds.
    pub fn replace(
        &mut self,
        offers: Vec<ClipboardMimeOfferV2>,
    ) -> Result<&LocalClipboardOffer, LocalClipboardError> {
        let owner = self
            .focused_owner
            .clone()
            .ok_or(LocalClipboardError::InvalidOwner)?;
        if let Err(error) = validate_offers(&offers) {
            // A provider replacement is an ownership event even when its rich
            // representations are malformed. Keeping the previous generation
            // selectable would let a restarted or compromised provider revoke
            // its bytes on one side of the boundary while the DRM seat keeps
            // publishing them on the other. Fail closed, and advance the local
            // generation so corrected-forward recovery cannot alias the stale
            // selection.
            self.release_current();
            return Err(error);
        }
        let generation = self.take_generation();
        self.current = Some(LocalClipboardOffer {
            owner,
            clip_id: ClipboardClipId::new(),
            session: self.session,
            generation,
            offers,
        });
        self.current.as_ref().ok_or(LocalClipboardError::Denied(
            ClipboardDenialReasonV2::InvalidPayload,
        ))
    }

    /// Apply one completed client update. This is bounded in-memory work and
    /// does not call back into the transport.
    pub fn apply_client_poll(
        &mut self,
        poll: ClipboardClientPoll,
    ) -> Result<bool, LocalClipboardError> {
        match poll {
            ClipboardClientPoll::Unchanged => Ok(false),
            ClipboardClientPoll::Cleared => Ok(self.clear()),
            ClipboardClientPoll::Offer(offers) => {
                self.replace(offers)?;
                Ok(true)
            }
        }
    }

    /// Build the canonical Clipboard V2 selection for this exact local offer
    /// generation and MIME representation.
    pub fn selection(
        &self,
        mime: ClipboardMimeKind,
    ) -> Result<ClipboardSelectionV2, ClipboardDenialReasonV2> {
        let current = self
            .current
            .as_ref()
            .ok_or(ClipboardDenialReasonV2::Stale)?;
        let offer = current
            .offers
            .iter()
            .find(|offer| offer.mime == mime)
            .ok_or(ClipboardDenialReasonV2::Unsupported)?;
        let selection = ClipboardSelectionV2 {
            schema_version: CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION,
            clip_id: current.clip_id,
            session: current.session,
            generation: current.generation,
            mime,
            content_sha256_hex: offer.content_sha256_hex.clone(),
            decision: ClipboardSelectionDecisionV2::Selected,
        };
        Ok(selection)
    }

    /// Materialize an exact canonical V2 selection from the current offer.
    /// Stale generations, clip/session mismatches, changed digests, denied
    /// decisions, and unsupported MIME kinds all fail closed.
    pub fn select(
        &self,
        selection: &ClipboardSelectionV2,
    ) -> Result<&ClipboardMimeOfferV2, ClipboardDenialReasonV2> {
        let current = self
            .current
            .as_ref()
            .ok_or(ClipboardDenialReasonV2::Stale)?;
        if selection.schema_version != CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION {
            return Err(ClipboardDenialReasonV2::UnknownVersion);
        }
        if !matches!(selection.decision, ClipboardSelectionDecisionV2::Selected) {
            return Err(ClipboardDenialReasonV2::Unsupported);
        }
        if selection.clip_id != current.clip_id
            || selection.session != current.session
            || selection.generation != current.generation
        {
            return Err(ClipboardDenialReasonV2::Stale);
        }
        let offer = current
            .offers
            .iter()
            .find(|offer| offer.mime == selection.mime)
            .ok_or(ClipboardDenialReasonV2::Unsupported)?;
        if selection.content_sha256_hex != offer.content_sha256_hex {
            return Err(ClipboardDenialReasonV2::InvalidPayload);
        }
        offer.validate().map_err(|error| error.denial_reason())?;
        Ok(offer)
    }

    /// Select exact plain text for egui's legacy Paste event.
    pub fn select_text(&self) -> Result<&str, ClipboardDenialReasonV2> {
        let selection = self.selection(ClipboardMimeKind::TextPlain)?;
        let offer = self.select(&selection)?;
        match &offer.payload {
            ClipboardPayloadV2::InlineText { text } => Ok(text),
            _ => Err(ClipboardDenialReasonV2::Unsupported),
        }
    }

    fn release_current(&mut self) {
        self.current = None;
        let _ = self.take_generation();
    }

    fn take_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        generation
    }
}

fn validate_owner(owner: &str) -> Result<(), LocalClipboardError> {
    if owner.is_empty()
        || owner.len() > MAX_CLIPBOARD_OWNER_BYTES
        || owner.trim() != owner
        || owner.chars().any(char::is_control)
    {
        Err(LocalClipboardError::InvalidOwner)
    } else {
        Ok(())
    }
}

fn validate_offers(offers: &[ClipboardMimeOfferV2]) -> Result<(), LocalClipboardError> {
    if offers.is_empty() || offers.len() > MAX_CLIPBOARD_OFFERS {
        return Err(LocalClipboardError::Denied(
            ClipboardDenialReasonV2::Oversized,
        ));
    }
    let mut seen = Vec::with_capacity(offers.len());
    let mut inline_bytes = 0usize;
    for offer in offers {
        offer
            .validate()
            .map_err(|error| LocalClipboardError::Denied(error.denial_reason()))?;
        if seen.contains(&offer.mime) {
            return Err(LocalClipboardError::Denied(
                ClipboardDenialReasonV2::InvalidPayload,
            ));
        }
        if let ClipboardPayloadV2::InlineText { text } = &offer.payload {
            inline_bytes = inline_bytes.saturating_add(text.len());
            if inline_bytes > MAX_CLIPBOARD_INLINE_TOTAL_BYTES {
                return Err(LocalClipboardError::Denied(
                    ClipboardDenialReasonV2::Oversized,
                ));
            }
        }
        seen.push(offer.mime);
    }
    Ok(())
}

/// Build one exact, bounded plain-text V2 offer for egui compatibility.
pub fn text_offer(text: &str) -> Option<ClipboardMimeOfferV2> {
    let text = normalize_and_bound_text(text, MAX_CLIPBOARD_TEXT_BYTES);
    (!text.is_empty())
        .then(|| ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, text).ok())
        .flatten()
}

/// Normalize line endings and cap text without allocating an unbounded result.
///
/// A CRLF pair is one LF, a lone CR is also an LF, and the result is cut only at
/// a UTF-8 character boundary. Keeping this helper in the shared seam ensures
/// reads from a remote provider and writes from egui have the same contract.
#[must_use]
pub(crate) fn normalize_and_bound_text(text: &str, max_bytes: usize) -> String {
    if max_bytes == 0 || text.is_empty() {
        return String::new();
    }

    let mut bounded = String::with_capacity(text.len().min(max_bytes));
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let normalized = if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            '\n'
        } else {
            ch
        };
        if bounded.len() + normalized.len_utf8() > max_bytes {
            break;
        }
        bounded.push(normalized);
    }
    bounded
}

/// A text-only clipboard provider for the bare-DRM runner.
///
/// Implementations may refresh from an external source in [`Self::read_text`] and
/// publish a local copy in [`Self::write_text`]. Failures stay implementation-owned
/// so a temporarily unavailable mesh transport never breaks local input handling.
/// An implementation must return `None` when no text is available; callers use that
/// result to clear any cached paste value. Passing an empty string to
/// [`Self::write_text`] clears the provider.
pub trait TextClipboard {
    /// Return the text that should be pasted now, if one is available.
    fn read_text(&mut self) -> Option<String>;

    /// Record text copied by egui.
    fn write_text(&mut self, text: &str);
}

/// Process-local text clipboard used by compatibility callers and DRM examples.
#[derive(Debug, Default)]
pub struct MemoryTextClipboard {
    text: Option<String>,
}

impl MemoryTextClipboard {
    /// Create an empty process-local clipboard.
    #[must_use]
    pub const fn new() -> Self {
        Self { text: None }
    }
}

impl TextClipboard for MemoryTextClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn write_text(&mut self, text: &str) {
        let text = normalize_and_bound_text(text, MAX_CLIPBOARD_TEXT_BYTES);
        if text.is_empty() {
            self.text = None;
            return;
        }

        self.text = Some(text);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        text_offer, LocalClipboardAuthority, LocalClipboardError, MemoryTextClipboard,
        TextClipboard, MAX_CLIPBOARD_TEXT_BYTES,
    };
    use mde_collab_types::{
        ClipboardDenialReasonV2, ClipboardMimeKind, ClipboardMimeOfferV2, ClipboardPayloadV2,
        ClipboardSelectionDecisionV2, ClipboardUnsupportedReason, MAX_CLIPBOARD_INLINE_TEXT_BYTES,
        MAX_CLIPBOARD_OFFERS,
    };

    #[test]
    fn memory_provider_round_trips_text() {
        let mut clipboard = MemoryTextClipboard::new();
        assert!(clipboard.read_text().is_none());
        clipboard.write_text("seat\r\ntext\rline");
        assert_eq!(clipboard.read_text().as_deref(), Some("seat\ntext\nline"));
    }

    #[test]
    fn empty_write_clears_the_available_text() {
        let mut clipboard = MemoryTextClipboard::new();
        clipboard.write_text("stale text");
        clipboard.write_text("");
        assert!(clipboard.read_text().is_none());
    }

    #[test]
    fn memory_provider_bounds_without_splitting_utf8() {
        let mut clipboard = MemoryTextClipboard::new();
        let prefix = "a".repeat(MAX_CLIPBOARD_TEXT_BYTES - 1);
        clipboard.write_text(&format!("{prefix}é"));

        let stored = clipboard.read_text().expect("bounded text is retained");
        assert_eq!(stored.len(), MAX_CLIPBOARD_TEXT_BYTES - 1);
        assert_eq!(stored, prefix);
        assert!(stored.is_char_boundary(stored.len()));
    }

    #[test]
    fn normalization_and_bound_are_shared_by_local_writes() {
        let mut clipboard = MemoryTextClipboard::new();
        clipboard.write_text("one\r\ntwo\rthree");
        assert_eq!(clipboard.read_text().as_deref(), Some("one\ntwo\nthree"));
    }

    #[test]
    fn ownership_generation_invalidates_stale_selection_on_app_switch() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("files").expect("focus files");
        authority
            .replace(vec![text_offer("files text").expect("text offer")])
            .expect("files offer");
        let stale = authority
            .selection(ClipboardMimeKind::TextPlain)
            .expect("selection");
        let first_generation = stale.generation;

        assert!(authority.focus("browser").expect("switch app"));
        assert_eq!(
            authority.select(&stale),
            Err(ClipboardDenialReasonV2::Stale)
        );
        authority
            .replace(vec![text_offer("browser text").expect("text offer")])
            .expect("browser offer");
        assert!(authority.current().expect("current").generation() > first_generation);
    }

    #[test]
    fn hostile_owner_identity_is_rejected_before_clipboard_admission() {
        let mut authority = LocalClipboardAuthority::new();
        for owner in [" files", "files ", "files\nstatus", "files\u{7f}status"] {
            assert_eq!(
                authority.focus(owner),
                Err(LocalClipboardError::InvalidOwner),
                "malformed owner must not enter the native clipboard authority"
            );
        }
        assert!(authority.current().is_none());
    }

    #[test]
    fn restarted_native_provider_invalid_replacement_cannot_retain_prior_offer_authority() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("files").expect("focus files");
        authority
            .replace(vec![text_offer("prior rich clipboard").expect("text offer")])
            .expect("initial provider offer");
        let stale_selection = authority
            .selection(ClipboardMimeKind::TextPlain)
            .expect("initial selection");

        let mut substituted = text_offer("replacement").expect("replacement offer");
        substituted.content_sha256_hex = Some("0".repeat(64));
        assert_eq!(
            authority.replace(vec![substituted]),
            Err(LocalClipboardError::Denied(
                ClipboardDenialReasonV2::InvalidPayload
            )),
            "a restarted provider's digest substitution must fail admission"
        );
        assert!(
            authority.current().is_none(),
            "rejected replacement must revoke the prior offer generation"
        );
        assert_eq!(
            authority.select(&stale_selection),
            Err(ClipboardDenialReasonV2::Stale),
            "the prior exact selection must not survive provider replacement"
        );

        let recovered = authority
            .replace(vec![text_offer("corrected forward").expect("corrected offer")])
            .expect("corrected provider offer");
        assert!(recovered.generation() > stale_selection.generation);
        assert_eq!(
            authority.select_text(),
            Ok("corrected forward"),
            "a valid newer provider generation must restore clipboard authority"
        );
    }

    #[test]
    fn focus_loss_revokes_offer_and_exact_generation_selection() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("editor").expect("focus editor");
        authority
            .replace(vec![text_offer("draft").expect("text offer")])
            .expect("offer");
        let selection = authority
            .selection(ClipboardMimeKind::TextPlain)
            .expect("selection");
        assert!(authority.lose_focus());
        assert!(authority.current().is_none());
        assert_eq!(
            authority.select(&selection),
            Err(ClipboardDenialReasonV2::Stale)
        );
    }

    #[test]
    fn hostile_offer_bounds_and_duplicate_mime_fail_closed() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("files").expect("focus files");

        let too_many = (0..=MAX_CLIPBOARD_OFFERS)
            .map(|_| {
                ClipboardMimeOfferV2::unsupported(
                    ClipboardMimeKind::ImagePng,
                    ClipboardUnsupportedReason::TransportUnsupported,
                )
            })
            .collect();
        assert_eq!(
            authority.replace(too_many),
            Err(LocalClipboardError::Denied(
                ClipboardDenialReasonV2::Oversized
            ))
        );

        let duplicate = vec![
            text_offer("one").expect("text offer"),
            text_offer("two").expect("text offer"),
        ];
        assert_eq!(
            authority.replace(duplicate),
            Err(LocalClipboardError::Denied(
                ClipboardDenialReasonV2::InvalidPayload
            ))
        );

        let oversized = "x".repeat(MAX_CLIPBOARD_INLINE_TEXT_BYTES + 1);
        assert!(ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextHtml, oversized).is_err());
        assert!(authority.current().is_none());
    }

    #[test]
    fn aggregate_inline_payload_bound_rejects_valid_individual_offers() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("editor").expect("focus editor");
        let chunk = "x".repeat(MAX_CLIPBOARD_INLINE_TEXT_BYTES);
        let first = ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, chunk.clone())
            .expect("first bounded offer");
        let second = ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextHtml, chunk)
            .expect("second bounded offer");
        let third = ClipboardMimeOfferV2::inline_text(
            ClipboardMimeKind::TextRtf,
            "x".repeat(MAX_CLIPBOARD_INLINE_TEXT_BYTES),
        )
        .expect("third bounded offer");
        assert_eq!(
            authority.replace(vec![first, second, third]),
            Err(LocalClipboardError::Denied(
                ClipboardDenialReasonV2::Oversized
            ))
        );
        assert!(authority.current().is_none());
    }

    #[test]
    fn selecting_plain_text_preserves_rich_mime_offer_order_and_bytes() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("editor").expect("focus editor");
        let html =
            ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextHtml, "<strong>same</strong>")
                .expect("html offer");
        let plain = text_offer("same\r\ntext").expect("plain offer");
        let rtf = ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextRtf, r"{\rtf1 same}")
            .expect("rtf offer");
        authority
            .replace(vec![html.clone(), plain.clone(), rtf.clone()])
            .expect("rich offer");

        let selection = authority
            .selection(ClipboardMimeKind::TextPlain)
            .expect("plain selection");
        let selected = authority.select(&selection).expect("selected plain");
        assert_eq!(selected, &plain);
        assert_eq!(
            authority.current().expect("current").offers(),
            &[html, plain, rtf]
        );
        assert_eq!(authority.select_text().expect("plain text"), "same\ntext");
    }

    #[test]
    fn tampered_or_denied_canonical_selection_fails_closed() {
        let mut authority = LocalClipboardAuthority::new();
        authority.focus("editor").expect("focus editor");
        authority
            .replace(vec![text_offer("exact").expect("text offer")])
            .expect("offer");
        let mut selection = authority
            .selection(ClipboardMimeKind::TextPlain)
            .expect("selection");
        selection.content_sha256_hex = Some("0".repeat(64));
        assert_eq!(
            authority.select(&selection),
            Err(ClipboardDenialReasonV2::InvalidPayload)
        );
        selection.content_sha256_hex = authority.current().expect("current").offers()[0]
            .content_sha256_hex
            .clone();
        selection.decision = ClipboardSelectionDecisionV2::Denied {
            reason: ClipboardDenialReasonV2::SecretBearing,
        };
        assert_eq!(
            authority.select(&selection),
            Err(ClipboardDenialReasonV2::Unsupported)
        );
        assert!(matches!(
            &authority.current().expect("current").offers()[0].payload,
            ClipboardPayloadV2::InlineText { text } if text == "exact"
        ));
    }
}
