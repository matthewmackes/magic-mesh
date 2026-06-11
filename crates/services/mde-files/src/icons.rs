//! Lucide-style icons — Rust port of the `I` object in the prototype.
//!
//! Every icon is a full SVG document; the prototype's per-icon JSX gets wrapped
//! in `<svg xmlns viewBox="0 0 24 24" fill="none" stroke="currentColor"
//! stroke-width="1.6" stroke-linecap="square" stroke-linejoin="miter"> … </svg>`.
//! `currentColor` is honored by the Iced `svg::Style` closure so the rendered
//! glyph picks up the surrounding text colour.

use cosmic::iced::widget::svg;

use crate::model::PinIcon;

macro_rules! lucide {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="square" stroke-linejoin="miter">"##,
            $body,
            r#"</svg>"#,
        ).as_bytes()
    };
}

macro_rules! lucide_thin {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="square">"##,
            $body,
            r#"</svg>"#,
        ).as_bytes()
    };
}

// ─── Window-chrome + toolbar ───────────────────────────────────────────────
pub const PANEL_RIGHT: &[u8] =
    lucide!(r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M15 3v18"/>"#);
pub const ARROW_LEFT: &[u8] = lucide!(r#"<path d="M19 12H5M12 19l-7-7 7-7"/>"#);
pub const REFRESH: &[u8] = lucide!(
    r#"<polyline points="20 4 20 10 14 10"/><polyline points="4 20 4 14 10 14"/><path d="M20 10A8 8 0 0 0 6 6"/><path d="M4 14a8 8 0 0 0 14 4"/>"#
);
pub const SEARCH: &[u8] =
    lucide!(r#"<circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.5" y2="16.5"/>"#);
pub const PLUS: &[u8] =
    lucide!(r#"<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>"#);
pub const MORE: &[u8] = lucide!(
    r#"<circle cx="5" cy="12" r="1.4"/><circle cx="12" cy="12" r="1.4"/><circle cx="19" cy="12" r="1.4"/>"#
);
pub const MINUS: &[u8] = lucide_thin!(r#"<line x1="6" y1="13" x2="18" y2="13"/>"#);
pub const MAXIMIZE: &[u8] = lucide_thin!(r#"<rect x="6" y="6" width="12" height="12"/>"#);
pub const CLOSE: &[u8] =
    lucide_thin!(r#"<line x1="6" y1="6" x2="18" y2="18"/><line x1="6" y1="18" x2="18" y2="6"/>"#);
pub const CHEVRON_RIGHT: &[u8] = lucide!(r#"<polyline points="9 6 15 12 9 18"/>"#);
pub const CHEVRON_DOWN: &[u8] = lucide!(r#"<polyline points="6 9 12 15 18 9"/>"#);

// ─── File-type icons ───────────────────────────────────────────────────────
pub const FOLDER: &[u8] = lucide!(
    r#"<path d="M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6z"/>"#
);
pub const DOC2: &[u8] = lucide!(
    r#"<path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z"/><polyline points="14 3 14 8 19 8"/><line x1="8" y1="13" x2="16" y2="13"/><line x1="8" y1="17" x2="13" y2="17"/>"#
);
pub const IMAGE_FILE: &[u8] = lucide!(
    r#"<rect x="3" y="4" width="18" height="16"/><circle cx="9" cy="10" r="1.5"/><polyline points="3 17 9 13 14 16 21 11"/>"#
);
pub const PDF: &[u8] = lucide!(
    r#"<path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z"/><polyline points="14 3 14 8 19 8"/><text x="7.5" y="18" font-size="6" font-family="monospace" fill="currentColor" stroke="none">PDF</text>"#
);
pub const ARCHIVE: &[u8] = lucide!(
    r#"<rect x="3" y="4" width="18" height="4"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8"/><line x1="10" y1="13" x2="14" y2="13"/>"#
);
pub const DISK_IMG: &[u8] =
    lucide!(r#"<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="3"/>"#);

// ─── Mesh / device icons ───────────────────────────────────────────────────
pub const MESH_HUB: &[u8] = lucide!(
    r#"<circle cx="12" cy="12" r="2.5"/><circle cx="5" cy="6" r="1.6"/><circle cx="19" cy="6" r="1.6"/><circle cx="5" cy="18" r="1.6"/><circle cx="19" cy="18" r="1.6"/><line x1="6.2" y1="7" x2="10.4" y2="11"/><line x1="17.8" y1="7" x2="13.6" y2="11"/><line x1="6.2" y1="17" x2="10.4" y2="13"/><line x1="17.8" y1="17" x2="13.6" y2="13"/>"#
);
pub const MONITOR: &[u8] = lucide!(
    r#"<rect x="3" y="4" width="18" height="12" rx="1"/><line x1="8" y1="20" x2="16" y2="20"/><line x1="12" y1="16" x2="12" y2="20"/>"#
);
pub const SERVER: &[u8] = lucide!(
    r#"<rect x="3" y="4" width="18" height="7" rx="1"/><rect x="3" y="13" width="18" height="7" rx="1"/><line x1="7" y1="7.5" x2="9" y2="7.5"/><line x1="7" y1="16.5" x2="9" y2="16.5"/><circle cx="17" cy="7.5" r="0.5" fill="currentColor"/><circle cx="17" cy="16.5" r="0.5" fill="currentColor"/>"#
);
pub const PHONE: &[u8] = lucide!(
    r#"<rect x="6" y="3" width="12" height="18" rx="1.5"/><line x1="11" y1="18" x2="13" y2="18"/>"#
);
pub const CPU: &[u8] = lucide!(
    r#"<rect x="6" y="6" width="12" height="12"/><rect x="9" y="9" width="6" height="6"/><line x1="9" y1="3" x2="9" y2="6"/><line x1="15" y1="3" x2="15" y2="6"/><line x1="9" y1="18" x2="9" y2="21"/><line x1="15" y1="18" x2="15" y2="21"/><line x1="3" y1="9" x2="6" y2="9"/><line x1="3" y1="15" x2="6" y2="15"/><line x1="18" y1="9" x2="21" y2="9"/><line x1="18" y1="15" x2="21" y2="15"/>"#
);

// ─── Verb icons ────────────────────────────────────────────────────────────
pub const INBOX: &[u8] = lucide!(
    r#"<polyline points="3 13 8 13 10 16 14 16 16 13 21 13"/><path d="M5 5h14l2 8v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-6z"/>"#
);
pub const SEND: &[u8] = lucide!(
    r#"<line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/>"#
);
pub const DOWNLOAD: &[u8] = lucide!(
    r#"<path d="M12 4v12"/><polyline points="6 12 12 18 18 12"/><line x1="4" y1="20" x2="20" y2="20"/>"#
);
pub const UPLOAD: &[u8] = lucide!(
    r#"<path d="M12 20V8"/><polyline points="6 12 12 6 18 12"/><line x1="4" y1="4" x2="20" y2="4"/>"#
);
pub const LIST_VIEW: &[u8] = lucide!(
    r#"<line x1="4" y1="7" x2="20" y2="7"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="17" x2="20" y2="17"/>"#
);
pub const GRID_VIEW: &[u8] = lucide!(
    r#"<rect x="4" y="4" width="7" height="7"/><rect x="13" y="4" width="7" height="7"/><rect x="4" y="13" width="7" height="7"/><rect x="13" y="13" width="7" height="7"/>"#
);

// ─── Local filesystem icons ────────────────────────────────────────────────
pub const HOME: &[u8] = lucide!(r#"<path d="M3 11l9-8 9 8"/><path d="M5 10v10h14V10"/>"#);
pub const HDD: &[u8] = lucide!(
    r#"<rect x="3" y="6" width="18" height="12" rx="1"/><line x1="6" y1="14" x2="8" y2="14"/><line x1="10" y1="14" x2="12" y2="14"/>"#
);
pub const TRASH2: &[u8] = lucide!(
    r#"<polyline points="4 6 20 6"/><path d="M8 6V4h8v2"/><path d="M6 6l1 14a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-14"/>"#
);
pub const PLAYER: &[u8] = lucide!(
    r#"<rect x="2" y="5" width="20" height="14" rx="2"/><polygon points="10 9 16 12 10 15 10 9" fill="currentColor" stroke="none"/>"#
);
pub const DOC: &[u8] = lucide!(
    r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><line x1="8" y1="12" x2="16" y2="12"/><line x1="8" y1="16" x2="14" y2="16"/>"#
);
pub const RUST: &[u8] = lucide!(
    r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.6 4.6l2.1 2.1M17.3 17.3l2.1 2.1M19.4 4.6l-2.1 2.1M6.7 17.3l-2.1 2.1"/>"#
);

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Build an Iced SVG handle from a static byte slice (one of the `pub const`s
/// above). The handle is cheap to clone and identity-compares by pointer.
#[must_use]
pub fn handle(svg_bytes: &'static [u8]) -> svg::Handle {
    svg::Handle::from_memory(svg_bytes)
}

/// Map a [`PinIcon`] to the SVG bytes used by the prototype's `FM_LOCAL_PINS`.
#[must_use]
pub fn svg_for_pin(pin: PinIcon) -> &'static [u8] {
    match pin {
        PinIcon::Home => HOME,
        PinIcon::Doc2 => DOC2,
        PinIcon::Image => IMAGE_FILE,
        PinIcon::Doc => DOC,
        PinIcon::Player => PLAYER,
        PinIcon::Rust => RUST,
        PinIcon::Hdd => HDD,
        PinIcon::Trash => TRASH2,
    }
}

/// Mime-icon mapping (`fm-row` left cell).
#[must_use]
pub fn svg_for_mime(mime: crate::model::Mime) -> &'static [u8] {
    use crate::model::Mime;
    match mime {
        Mime::Folder => FOLDER,
        Mime::Doc => DOC2,
        Mime::Image => IMAGE_FILE,
        Mime::Pdf => PDF,
        Mime::Archive => ARCHIVE,
        Mime::Disk => DISK_IMG,
    }
}

/// Peer-kind avatar icon mapping (`fm-peer-card .avatar`).
#[must_use]
pub fn svg_for_peer_kind(kind: crate::model::PeerKind) -> &'static [u8] {
    use crate::model::PeerKind;
    match kind {
        PeerKind::Desktop => MONITOR,
        PeerKind::Server => SERVER,
        PeerKind::Phone => PHONE,
        PeerKind::Ci => CPU,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_valid_svg_envelope() {
        let icons: &[&[u8]] = &[
            PANEL_RIGHT,
            ARROW_LEFT,
            REFRESH,
            SEARCH,
            PLUS,
            MORE,
            MINUS,
            MAXIMIZE,
            CLOSE,
            CHEVRON_RIGHT,
            CHEVRON_DOWN,
            FOLDER,
            DOC2,
            IMAGE_FILE,
            PDF,
            ARCHIVE,
            DISK_IMG,
            MESH_HUB,
            MONITOR,
            SERVER,
            PHONE,
            CPU,
            INBOX,
            SEND,
            DOWNLOAD,
            UPLOAD,
            LIST_VIEW,
            GRID_VIEW,
            HOME,
            HDD,
            TRASH2,
            PLAYER,
            DOC,
            RUST,
        ];
        for bytes in icons {
            let s = std::str::from_utf8(bytes).expect("icon bytes utf8");
            assert!(s.starts_with("<svg "), "icon must start with <svg : {s}");
            assert!(s.ends_with("</svg>"), "icon must close with </svg>: {s}");
        }
    }
}
