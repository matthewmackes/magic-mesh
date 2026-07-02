//! The bookmark tree value types (locks Q1, Q2, Q7).
//!
//! These are the *converged* read-model the CRDT ([`crate::crdt`]) produces —
//! plain owned structs the UI and worker consume. The live registers that the
//! merge folds ops into live in [`crate::crdt`]; these are the snapshot it hands
//! out.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::hlc::Author;

/// A content-addressed favicon reference (lock Q6): the SHA-256 of the icon
/// bytes.
///
/// Identical icons across bookmarks collapse to one blob in the
/// [`crate::favicon::FaviconStore`]. Serialized as a lowercase hex string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Render as a 64-char lowercase hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            // Two lowercase hex nibbles per byte (never raw-hex UI colour — this
            // is a content digest, the §4 no-raw-hex rule is about theme colour).
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
        }
        s
    }

    /// Parse a 64-char hex string back into a hash, or `None` if malformed.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        let bytes = hex.as_bytes();
        let mut i = 0;
        while i < 32 {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            // hi and lo are each < 16, so the packed byte is < 256.
            out[i] = u8::try_from((hi << 4) | lo).unwrap_or(0);
            i += 1;
        }
        Some(Self(out))
    }
}

impl Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        Self::from_hex(&hex).ok_or_else(|| serde::de::Error::custom("invalid ContentHash hex"))
    }
}

/// Where a bookmark came from (lock Q7). Imports tag their origin; a bookmark
/// added in-app is [`Source::Manual`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Created by the user inside the app.
    #[default]
    Manual,
    /// Imported from Firefox `places.sqlite`.
    Firefox,
    /// Imported from a Chromium `Bookmarks` JSON file.
    Chromium,
    /// Imported from a Safari HTML export.
    Safari,
    /// Imported from a generic Netscape bookmarks HTML file.
    NetscapeHtml,
    /// Any other origin, named freely (forward-compat).
    Other(String),
}

/// Whether a tree item is a bookmark or a folder. Fixed at creation — an id
/// never changes kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A leaf bookmark (has a URL).
    Bookmark,
    /// An interior folder (holds children).
    Folder,
}

/// A converged bookmark leaf (lock Q7). `parent`/`order_key` place it in the
/// tree; `modified` is the wall time of its most-recent winning field write and
/// `last_author` who made it (lock Q64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    /// The item id — the per-item CRDT key, minted at creation (lock Q1).
    pub id: Uuid,
    /// The parent folder id, or `None` for a top-level item.
    pub parent: Option<Uuid>,
    /// The fractional-index order key among its siblings (lock Q3).
    pub order_key: String,
    /// The target URL.
    pub url: String,
    /// The display title.
    pub title: String,
    /// The content-addressed favicon, if one is known (lock Q6).
    pub favicon_ref: Option<ContentHash>,
    /// Free-text tags kept from import (lock Q7/Q11).
    pub tags: Vec<String>,
    /// Free-text notes.
    pub notes: String,
    /// Wall time (ms) the bookmark was first added.
    pub added: u64,
    /// Wall time (ms) of the most-recent winning field write.
    pub modified: u64,
    /// Where the bookmark came from (lock Q7).
    pub source: Source,
    /// Who last wrote a winning field (lock Q64).
    pub last_author: Author,
}

/// A converged folder (locks Q2, Q7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// The item id — minted at creation (lock Q1).
    pub id: Uuid,
    /// The folder display name.
    pub name: String,
    /// The parent folder id, or `None` for a top-level folder.
    pub parent: Option<Uuid>,
    /// The fractional-index order key among its siblings (lock Q3).
    pub order_key: String,
    /// Who last wrote a winning field (lock Q64).
    pub last_author: Author,
}

/// A converged tree item — either a [`Bookmark`] or a [`Folder`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    /// A bookmark leaf.
    Bookmark(Bookmark),
    /// A folder.
    Folder(Folder),
}

impl Item {
    /// The item's id.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Bookmark(b) => b.id,
            Self::Folder(f) => f.id,
        }
    }

    /// The item's parent folder id (or `None` at the top level).
    #[must_use]
    pub const fn parent(&self) -> Option<Uuid> {
        match self {
            Self::Bookmark(b) => b.parent,
            Self::Folder(f) => f.parent,
        }
    }

    /// The item's sibling order key (lock Q3).
    #[must_use]
    pub fn order_key(&self) -> &str {
        match self {
            Self::Bookmark(b) => &b.order_key,
            Self::Folder(f) => &f.order_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_hex_round_trips() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap_or(0);
        }
        let h = ContentHash(bytes);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentHash::from_hex(&hex), Some(h));
        assert_eq!(ContentHash::from_hex("nothex"), None);
    }

    #[test]
    fn content_hash_serde_is_a_hex_string() {
        let h = ContentHash([0xab; 32]);
        let json = serde_json::to_string(&h).expect("serialize");
        assert!(json.starts_with('"') && json.contains("abab"));
        let back: ContentHash = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, h);
    }

    #[test]
    fn source_defaults_to_manual() {
        assert_eq!(Source::default(), Source::Manual);
    }
}
