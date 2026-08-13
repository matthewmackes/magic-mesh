//! Clipboard mode — a space's cross-mesh clipboard lane (WL-FUNC-011).
//!
//! Renders the [`ClipboardLane`](mde_collab_types::ClipboardLane) projection: one
//! row per captured clip with its MIME kind (text vs. a shared URI), a preview
//! (shown where safe), the capturing node's attribution, and the SHA-256 content
//! address that de-duplicates the same clip across nodes. The worker folds the
//! existing cross-mesh clipboard captures (`event/clipboard/clip`) into
//! [`ClipboardPublished`](mde_collab_types::CollabEventKind::ClipboardPublished)
//! events; this mode also lets the seat **publish** a new clip and, per row,
//! **attach** it to the space, **pin/unpin** it, or **delete** it — each a typed
//! [`CollabCommand`].
//!
//! Text up to the shared canonical clipboard bound rides the clipboard lane;
//! anything larger is a Transfer, not a clip (the worker routes it there rather
//! than truncating).

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    ClipItemKind, ClipboardItem, ClipboardView, CollabCommand, EventId, SpaceId,
};

use crate::files::short_hash;
use crate::{icons, relative_age, CommunicationsSurface};

/// The lane row preview cap — a clip's preview is a recognisable head, never the
/// full (possibly large) content pasted into the row.
const PREVIEW_MAX: usize = 160;

/// The clipboard lane's content ceiling, shared with the canonical event body.
/// Larger content belongs on the Transfer lane and must not reach the publish
/// command's preview/hash materialization path.
const MAX_CLIP_BYTES: usize = mde_collab_types::MAX_CLIPBOARD_TEXT_BYTES;

/// Source identities cross the collaboration event boundary and are rendered
/// beside every clip. Keep them aligned with the platform's bounded identity
/// fields: trim transport whitespace, reject control characters, and never let
/// an unbounded projection become UI or signed-command attribution.
const MAX_CLIP_SOURCE_BYTES: usize = 128;

impl CommunicationsSurface {
    /// Render Clipboard mode for the selected space: the publish composer, then
    /// the newest-first lane with per-clip pin/attach/delete controls.
    pub(crate) fn clipboard_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to see its clipboard lane.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Clipboard")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("shared across the mesh")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
        ui.separator();

        let publishing_enabled = self.clipboard_publishing_enabled;
        self.clip_publish_composer(ui, sink, space, data.me().as_str(), publishing_enabled);
        ui.separator();

        match data.clipboard_lane(space) {
            Some(lane) if !lane.items.is_empty() => {
                let now = data.now_unix_ms();
                let local_source = data.me().as_str();
                egui::ScrollArea::vertical()
                    .id_salt("collab-clipboard")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for item in &lane.items {
                            self.clip_row(ui, sink, space, item, now, local_source);
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(
                    egui::RichText::new("No clips shared in this space yet.")
                        .color(Style::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(
                        "Copy on any mesh node and it lands here — or publish one below.",
                    )
                    .small()
                    .color(Style::TEXT_DIM),
                );
            }
        }
    }

    /// The publish composer: type text, press <kbd>Enter</kbd> (or the publish
    /// glyph) to emit [`PublishClipboard`](CollabCommand::PublishClipboard) with a
    /// clip carrying the seat's attribution + the real SHA-256 content address.
    fn clip_publish_composer(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        me: &str,
        publishing_enabled: bool,
    ) {
        let mut buf = self.clip_drafts.get(&space).cloned().unwrap_or_default();
        let mut publish = false;
        ui.add_enabled_ui(publishing_enabled, |ui| {
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id(egui::Id::new(("mde-collab-clip-composer", space.as_uuid())))
                        .desired_width(f32::INFINITY)
                        .hint_text("Publish a clip  ·  Enter to share"),
                );
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (resp.lost_focus() || resp.has_focus()) && enter {
                    publish = true;
                }
                if icons::icon_button(
                    ui,
                    icons::CLIP_PUBLISH,
                    Style::SP_M,
                    Style::ACCENT,
                    "Publish clip",
                )
                .clicked()
                {
                    publish = true;
                }
            });
        });
        // Whitespace is meaningful clipboard content. Use trimming only to
        // identify an all-blank draft; the canonical clipboard contract hashes
        // and preserves the exact supplied bytes.
        let text = buf.as_str();
        if publishing_enabled && publish && !text.trim().is_empty() {
            // Keep the session-scoped draft when canonical admission refuses the
            // value; losing a user's bounded-text correction target would make a
            // rejected publish look like a successful action.
            if self.publish_clip_text(sink, space, text, me) {
                buf.clear();
            }
        }
        self.clip_drafts.insert(space, buf);
        if !publishing_enabled {
            ui.label(
                egui::RichText::new(
                    "Local clipboard publishing is off for this session. Remote history remains visible below.",
                )
                .small()
                .color(Style::TEXT_DIM),
            );
        }
    }

    /// One clipboard-lane row: the kind glyph, the preview, the source + content
    /// address facts, and the pin/attach/delete controls.
    fn clip_row(
        &self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        item: &ClipboardView,
        now_unix_ms: i64,
        local_source: &str,
    ) {
        let source = canonical_clip_source(&item.source).unwrap_or("");
        let origin = clipboard_origin(source, local_source);
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(
                    ui,
                    icons::clip_kind_icon(item.kind),
                    Style::SP_M,
                    Style::ACCENT,
                );
                ui.label(egui::RichText::new(clip_preview(&item.preview)).color(Style::TEXT));
                ui.label(
                    egui::RichText::new(origin.label(source))
                        .small()
                        .color(origin.color()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::CLIP_DELETE,
                        Style::SP_M,
                        Style::DANGER,
                        &origin.action_hint("Delete clip"),
                    )
                    .clicked()
                    {
                        self.delete_clip(sink, space, item.event_id);
                    }
                    if icons::icon_button(
                        ui,
                        icons::CLIP_ATTACH,
                        Style::SP_M,
                        Style::TEXT_DIM,
                        &origin.action_hint("Attach to a message in this space"),
                    )
                    .clicked()
                    {
                        self.attach_clip(sink, space, item.event_id);
                    }
                    // Pin toggle — a pinned clip survives the cap + clear.
                    let (tint, hint) = if item.pinned {
                        (Style::WARN, origin.action_hint("Unpin"))
                    } else {
                        (Style::TEXT_DIM, origin.action_hint("Pin"))
                    };
                    if icons::icon_button(ui, icons::CLIP_PIN, Style::SP_M, tint, &hint).clicked() {
                        self.toggle_clip_pin(sink, space, item.event_id, item.pinned);
                    }
                });
            });

            // Honest facts: who captured it, when, and its content address.
            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  {}  ·  content {}",
                    if source.is_empty() { "unknown" } else { source },
                    relative_age(now_unix_ms, item.at_unix_ms),
                    short_hash(&item.sha256_hex),
                ))
                .small()
                .color(Style::TEXT_DIM),
            );
        });
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// Build a [`ClipboardItem`] from `text` (detecting a URI, hashing the real
    /// content address, attributing it to `source`) and emit
    /// [`PublishClipboard`](CollabCommand::PublishClipboard) into `space`.
    ///
    /// This seam honors the session-scoped local-publish preference before
    /// constructing any command. The same boundary protects the widget and
    /// future callers from bypassing the opt-in, while the read-side lane keeps
    /// remote rows visible when local publishing is disabled.
    ///
    /// Returns `true` only when the command was admitted by the canonical text
    /// lane's local boundary. Callers use this to retain a rejected
    /// session-scoped draft for correction.
    pub(crate) fn publish_clip_text(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        text: &str,
        source: &str,
    ) -> bool {
        self.publish_clip_text_from_ingress(sink, space, text, source, ClipboardIngress::LocalSeat)
    }

    /// Admit a guest-originated clipboard value only when the VDI session has
    /// explicitly declared a real, bidirectional text channel.  This is kept
    /// separate from the local-seat composer so a missing, unsupported, or
    /// malformed capability can never be mistaken for local consent.
    ///
    /// The shell/VDI mount must translate its protocol-specific capability into
    /// [`VdiClipboardCapability`] before calling this seam.  In particular,
    /// unsupported RDP/SPICE reports and an unreadable capability record must
    /// use their corresponding fail-closed variants rather than a best guess.
    #[allow(dead_code)]
    pub(crate) fn publish_vdi_clip_text(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        text: &str,
        source: &str,
        capability: VdiClipboardCapability,
    ) -> bool {
        self.publish_clip_text_from_ingress(
            sink,
            space,
            text,
            source,
            ClipboardIngress::Vdi(capability),
        )
    }

    fn publish_clip_text_from_ingress(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        text: &str,
        source: &str,
        ingress: ClipboardIngress,
    ) -> bool {
        // Match the canonical event validator before constructing metadata:
        // blank text or attribution would be discarded downstream, while a
        // value over the shared UTF-8 byte ceiling must never be truncated here.
        // The local session privacy gate is checked at this seam as well as at
        // the widget boundary so test callers and future UI affordances cannot
        // bypass the opt-in by constructing a command directly.
        let Some(source) = canonical_clip_source(source) else {
            return false;
        };
        if !ingress.is_admitted()
            || !self.clipboard_publishing_enabled
            || text.trim().is_empty()
            || !clip_fits_lane(text.len())
        {
            return false;
        }

        let item = ClipboardItem {
            kind: detect_kind(text),
            preview: clip_preview(text),
            sha256_hex: mde_collab_types::value::sha256_hex(text.as_bytes()),
            len: text.len() as u64,
            source: source.to_owned(),
        };
        sink.emit(CollabCommand::PublishClipboard {
            space,
            text: text.to_owned(),
            item,
        });
        true
    }

    /// Emit [`AttachClipboard`](CollabCommand::AttachClipboard) — re-share `clip`
    /// as a message in `space`.
    pub(crate) fn attach_clip(&self, sink: &mut crate::CommandSink, space: SpaceId, clip: EventId) {
        sink.emit(CollabCommand::AttachClipboard { space, clip });
    }

    /// Emit [`PinClipboard`](CollabCommand::PinClipboard) or
    /// [`UnpinClipboard`](CollabCommand::UnpinClipboard) depending on the current
    /// pin state.
    pub(crate) fn toggle_clip_pin(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        clip: EventId,
        pinned: bool,
    ) {
        if pinned {
            sink.emit(CollabCommand::UnpinClipboard { space, clip });
        } else {
            sink.emit(CollabCommand::PinClipboard { space, clip });
        }
    }

    /// Emit [`DeleteClipboard`](CollabCommand::DeleteClipboard) — remove a single
    /// clip from the lane.
    pub(crate) fn delete_clip(&self, sink: &mut crate::CommandSink, space: SpaceId, clip: EventId) {
        sink.emit(CollabCommand::DeleteClipboard { space, clip });
    }
}

/// Capability supplied by a VDI protocol adapter at the guest-to-mesh ingress.
///
/// This deliberately has no permissive default.  A backend must make a positive
/// bidirectional assertion after validating its own protocol status; an absent
/// or malformed record is kept distinct from a protocol that explicitly reports
/// itself unsupported, and both are rejected before clipboard item hashing or
/// command emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum VdiClipboardCapability {
    /// Both host-to-guest and guest-to-host lanes have real protocol channels.
    Bidirectional,
    /// The backend explicitly reports that one or both lanes are unavailable.
    Unsupported,
    /// The backend record was absent, incomplete, or otherwise invalid.
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardIngress {
    LocalSeat,
    Vdi(VdiClipboardCapability),
}

impl ClipboardIngress {
    const fn is_admitted(self) -> bool {
        matches!(
            self,
            Self::LocalSeat | Self::Vdi(VdiClipboardCapability::Bidirectional)
        )
    }
}

/// Whether a clip of `len` bytes fits in the clipboard lane. Keep this a pure
/// boundary seam so the oversized-input policy is testable without making every
/// boundary test allocate a 100 MiB string.
const fn clip_fits_lane(len: usize) -> bool {
    len <= MAX_CLIP_BYTES
}

/// The provenance visible at the UI boundary. An empty source is retained as
/// an explicit unknown rather than being guessed as local or remote; older
/// projections can therefore stay visible without fabricating attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardOrigin {
    Local,
    Remote,
    Unattributed,
}

impl ClipboardOrigin {
    /// Renderable provenance label, including the source when the projection
    /// supplied one. This is intentionally text, not colour-only state.
    fn label(self, source: &str) -> String {
        match self {
            Self::Local => format!("Local · {source}"),
            Self::Remote => format!("Remote · {source}"),
            Self::Unattributed => "Source unavailable".to_owned(),
        }
    }

    /// Quazar token for the provenance label.
    const fn color(self) -> egui::Color32 {
        match self {
            Self::Local => Style::ACCENT,
            Self::Remote => Style::TEXT_DIM,
            Self::Unattributed => Style::DISABLED,
        }
    }

    /// Add provenance to command hints so keyboard/accessibility users can
    /// distinguish local and remote actions without changing command semantics.
    fn action_hint(self, action: &str) -> String {
        match self {
            Self::Local => format!("{action} local clipboard clip"),
            Self::Remote => format!("{action} remote clipboard clip"),
            Self::Unattributed => format!("{action} clipboard clip with unavailable source"),
        }
    }
}

/// Compare the projected source with the local actor identity. Remote rows are
/// never filtered here: visibility is a read-side property of the lane, while
/// this helper only makes attribution explicit in the presentation.
fn clipboard_origin(source: &str, local_source: &str) -> ClipboardOrigin {
    let Some(source) = canonical_clip_source(source) else {
        return ClipboardOrigin::Unattributed;
    };
    let Some(local_source) = canonical_clip_source(local_source) else {
        return ClipboardOrigin::Unattributed;
    };
    if source == local_source {
        ClipboardOrigin::Local
    } else {
        ClipboardOrigin::Remote
    }
}

/// Return the canonical collaboration attribution admitted for publishing and
/// presentation. This is deliberately stricter than a cosmetic `trim()`: a
/// control-bearing or oversized source is unavailable rather than rewritten
/// into a potentially different actor identity.
fn canonical_clip_source(source: &str) -> Option<&str> {
    let source = source.trim();
    (!source.is_empty()
        && source.len() <= MAX_CLIP_SOURCE_BYTES
        && !source.chars().any(char::is_control))
    .then_some(source)
}

/// Classify a clip's content. Only a complete HTTP(S) URI with a non-empty
/// authority receives URI attribution; prose that merely starts with a scheme,
/// malformed links, and control-bearing values remain honest plain text.
fn detect_kind(text: &str) -> ClipItemKind {
    if is_shared_http_uri(text) {
        ClipItemKind::Uri
    } else {
        ClipItemKind::Text
    }
}

/// Validate the small URI surface represented by [`ClipItemKind::Uri`] without
/// turning clipboard admission into URL resolution. The exact clipboard bytes
/// remain untouched; this seam only controls their typed presentation.
fn is_shared_http_uri(text: &str) -> bool {
    let candidate = text.trim();
    if candidate.is_empty()
        || candidate
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }

    let Some(remainder) = candidate
        .strip_prefix("https://")
        .or_else(|| candidate.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = remainder
        .split_once(['/', '?', '#'])
        .map_or(remainder, |(authority, _)| authority);

    !authority.is_empty()
        && authority != "."
        && authority != ".."
        && !authority.starts_with('.')
        && !authority.ends_with('.')
}

/// A single-line, capped preview of clip content (the row shows a recognisable
/// head, never the full possibly-large payload).
fn clip_preview(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > PREVIEW_MAX {
        let head: String = one_line.chars().take(PREVIEW_MAX).collect();
        format!("{head}\u{2026}")
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_clip_source, clip_fits_lane, clip_preview, clipboard_origin, detect_kind,
        ClipboardOrigin, VdiClipboardCapability, MAX_CLIP_BYTES, MAX_CLIP_SOURCE_BYTES,
        PREVIEW_MAX,
    };
    use crate::{CommandSink, CommunicationsSurface};
    use mde_collab_types::SpaceId;

    #[test]
    fn oversized_clip_is_rejected_before_publish_command_materialization() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        let mut sink = CommandSink::new();
        let oversized = "x".repeat(MAX_CLIP_BYTES + 1);

        surface.publish_clip_text(&mut sink, SpaceId::new(), &oversized, "eagle");

        assert!(
            sink.queued().is_empty(),
            "oversized clipboard text must not become a PublishClipboard command"
        );
    }

    #[test]
    fn clip_lane_boundary_is_inclusive() {
        assert!(clip_fits_lane(MAX_CLIP_BYTES));
        assert!(!clip_fits_lane(MAX_CLIP_BYTES + 1));
    }

    #[test]
    fn publish_clip_text_accepts_the_shared_canonical_limit() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        let mut sink = CommandSink::new();
        let text = "x".repeat(mde_collab_types::MAX_CLIPBOARD_TEXT_BYTES);

        assert!(surface.publish_clip_text(&mut sink, SpaceId::new(), &text, "eagle"));

        let Some(mde_collab_types::CollabCommand::PublishClipboard {
            text: published,
            item,
            ..
        }) = sink.queued().first()
        else {
            panic!("canonical-bound clipboard text must become a publish command");
        };
        assert_eq!(published.len(), mde_collab_types::MAX_CLIPBOARD_TEXT_BYTES);
        assert_eq!(item.len, published.len() as u64);
        assert_eq!(item.source, "eagle");
    }

    #[test]
    fn publish_clip_text_preserves_whitespace_in_canonical_payload() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        let mut sink = CommandSink::new();
        let text = "  keep these bytes  ";

        assert!(surface.publish_clip_text(&mut sink, SpaceId::new(), text, "eagle"));

        let Some(mde_collab_types::CollabCommand::PublishClipboard {
            text: published,
            item,
            ..
        }) = sink.queued().first()
        else {
            panic!("clipboard payload must become a publish command");
        };
        assert_eq!(published, text);
        assert_eq!(item.len, text.len() as u64);
        assert_eq!(
            item.sha256_hex,
            mde_collab_types::value::sha256_hex(text.as_bytes())
        );
    }

    #[test]
    fn canonical_rejection_reports_failure_without_queueing_or_truncating() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        let mut sink = CommandSink::new();
        let oversized = format!(
            "{}é",
            "x".repeat(mde_collab_types::MAX_CLIPBOARD_TEXT_BYTES - 1)
        );

        assert!(!surface.publish_clip_text(&mut sink, SpaceId::new(), &oversized, "seat:eagle"));
        assert!(sink.queued().is_empty());
    }

    #[test]
    fn canonical_rejection_reports_failure_for_blank_text_or_attribution() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        for (text, source) in [("  \n", "seat:eagle"), ("real text", "  ")] {
            let mut sink = CommandSink::new();
            assert!(!surface.publish_clip_text(&mut sink, SpaceId::new(), text, source));
            assert!(sink.queued().is_empty());
        }
    }

    #[test]
    fn local_publish_is_opt_in_at_the_command_seam() {
        let surface = CommunicationsSurface::new();
        let mut sink = CommandSink::new();

        assert!(!surface.clipboard_publishing_enabled());
        assert!(!surface.publish_clip_text(
            &mut sink,
            SpaceId::new(),
            "remote history stays",
            "eagle"
        ));
        assert!(sink.is_empty());

        let mut enabled = CommunicationsSurface::new();
        enabled.set_clipboard_publishing_enabled(true);
        assert!(enabled.clipboard_publishing_enabled());
        assert!(enabled.publish_clip_text(
            &mut sink,
            SpaceId::new(),
            "explicit opt-in publish",
            "eagle"
        ));
        assert!(matches!(
            sink.queued().first(),
            Some(mde_collab_types::CollabCommand::PublishClipboard { .. })
        ));
    }

    #[test]
    fn unsupported_or_malformed_vdi_capability_never_materializes_a_publish_command() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);

        for capability in [
            VdiClipboardCapability::Unsupported,
            VdiClipboardCapability::Malformed,
        ] {
            let mut sink = CommandSink::new();
            assert!(
                !surface.publish_vdi_clip_text(
                    &mut sink,
                    SpaceId::new(),
                    "guest clipboard must not cross an unverified boundary",
                    "vdi:guest",
                    capability,
                ),
                "{capability:?} VDI capability must fail closed"
            );
            assert!(
                sink.is_empty(),
                "{capability:?} VDI capability must not materialize a PublishClipboard command"
            );
        }
    }

    #[test]
    fn explicitly_bidirectional_vdi_capability_uses_the_existing_bounded_publish_boundary() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);
        let mut sink = CommandSink::new();

        assert!(surface.publish_vdi_clip_text(
            &mut sink,
            SpaceId::new(),
            "guest clipboard",
            "vdi:guest",
            VdiClipboardCapability::Bidirectional,
        ));
        assert!(matches!(
            sink.queued().first(),
            Some(mde_collab_types::CollabCommand::PublishClipboard { text, item, .. })
                if text == "guest clipboard" && item.source == "vdi:guest"
        ));
    }

    #[test]
    fn clip_preview_keeps_normalization_and_truncation_behavior() {
        assert_eq!(clip_preview("  hello\nmesh  "), "hello mesh");

        let long = "a".repeat(PREVIEW_MAX + 1);
        assert_eq!(clip_preview(&long), format!("{}…", "a".repeat(PREVIEW_MAX)));
    }

    #[test]
    fn uri_kind_requires_one_complete_bounded_http_uri() {
        use mde_collab_types::ClipItemKind;

        for uri in [
            "https://mesh.example/path?q=1#status",
            " http://node.example:8080/health ",
        ] {
            assert_eq!(detect_kind(uri), ClipItemKind::Uri, "{uri:?}");
        }

        for ambiguous in [
            "https://",
            "https:///missing-authority",
            "https://mesh.example copied from another seat",
            "https://mesh.example\nspoofed text",
            "https://.example/path",
            "HTTPS://mesh.example/path",
        ] {
            assert_eq!(
                detect_kind(ambiguous),
                ClipItemKind::Text,
                "ambiguous clipboard content {ambiguous:?} must not receive URI attribution"
            );
        }
    }

    #[test]
    fn clipboard_origin_keeps_local_remote_and_unknown_attribution_explicit() {
        assert_eq!(clipboard_origin("eagle", "eagle"), ClipboardOrigin::Local);
        assert_eq!(clipboard_origin("falcon", "eagle"), ClipboardOrigin::Remote);
        assert_eq!(
            clipboard_origin("", "eagle"),
            ClipboardOrigin::Unattributed,
            "missing source must not be guessed as local or remote"
        );
    }

    #[test]
    fn clipboard_origin_labels_and_action_hints_are_textual() {
        assert_eq!(ClipboardOrigin::Local.label("eagle"), "Local · eagle");
        assert_eq!(
            ClipboardOrigin::Remote.action_hint("Attach to a message"),
            "Attach to a message remote clipboard clip"
        );
        assert_eq!(
            ClipboardOrigin::Unattributed.label(""),
            "Source unavailable"
        );
    }

    #[test]
    fn whitespace_only_source_is_unavailable_without_rewriting_attribution() {
        assert_eq!(
            clipboard_origin(" \t\n", "eagle"),
            ClipboardOrigin::Unattributed,
            "blank legacy attribution must not be presented as a remote source"
        );
    }

    #[test]
    fn source_attribution_is_canonical_and_control_safe_at_publish_boundary() {
        let mut surface = CommunicationsSurface::new();
        surface.set_clipboard_publishing_enabled(true);

        let mut canonical = CommandSink::new();
        assert!(surface.publish_clip_text(
            &mut canonical,
            SpaceId::new(),
            "mesh value",
            "  seat:eagle  ",
        ));
        assert!(matches!(
            canonical.queued().first(),
            Some(mde_collab_types::CollabCommand::PublishClipboard { item, .. })
                if item.source == "seat:eagle"
        ));

        for source in [
            "seat:eagle\nspoofed",
            "seat:eagle\u{0000}",
            &"x".repeat(MAX_CLIP_SOURCE_BYTES + 1),
        ] {
            let mut rejected = CommandSink::new();
            assert!(!surface.publish_clip_text(
                &mut rejected,
                SpaceId::new(),
                "mesh value",
                source,
            ));
            assert!(rejected.is_empty());
        }
    }

    #[test]
    fn projected_source_attribution_never_renders_control_or_spoofed_identity() {
        assert_eq!(canonical_clip_source("  eagle  "), Some("eagle"));
        assert_eq!(
            clipboard_origin("  eagle  ", "eagle"),
            ClipboardOrigin::Local
        );
        assert_eq!(canonical_clip_source("eagle\nfalcon"), None);
        assert_eq!(
            clipboard_origin("eagle\nfalcon", "eagle"),
            ClipboardOrigin::Unattributed
        );
    }

    #[test]
    fn explicit_publish_keeps_session_capture_opt_in_unchanged() {
        let mut surface = CommunicationsSurface::new();
        let mut sink = CommandSink::new();

        assert!(!surface.clipboard_publishing_enabled());
        assert!(!surface.publish_clip_text(
            &mut sink,
            SpaceId::new(),
            "intentional Mesh Teams publish",
            "eagle",
        ));
        assert!(sink.is_empty());
        surface.set_clipboard_publishing_enabled(true);
        assert!(surface.publish_clip_text(
            &mut sink,
            SpaceId::new(),
            "intentional Mesh Teams publish",
            "eagle",
        ));
        assert!(
            surface.clipboard_publishing_enabled(),
            "publishing must require and preserve the explicit session opt-in"
        );
        assert!(matches!(
            sink.queued().first(),
            Some(mde_collab_types::CollabCommand::PublishClipboard { .. })
        ));
    }
}
