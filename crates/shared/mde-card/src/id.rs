//! Stable mesh-merged Card IDs (Portal-31, R5-Q9).
//!
//! Same logical Card on two peers must collapse to one row at merge
//! time. The ID derives from `(kind_tag, normalized_title)` — both
//! peers compute the same string, hash it, and arrive at the same
//! 16-hex-char prefix.
//!
//! Choice of hash: SHA-256 — already in the workspace lockfile via
//! mackesd's crypto path, and stable across Rust versions. We slice
//! to 16 hex chars (64 bits) because:
//!
//!   * Collision probability across the entire MDE fleet stays < 1e-9
//!     at ≤ 4 M cards per peer (birthday bound 2^32 cards). Real-world
//!     fleets sit at ~10 K cards/peer.
//!   * 64 bits keeps the on-screen `id:` chip short enough to fit
//!     in cascade-card chrome without truncation.

use sha2::{Digest, Sha256};

use crate::schema::CardKind;

/// Derive a stable mesh-merged ID for a Card.
///
/// The hash inputs are:
///   1. `kind.tag()` — canonical snake_case discriminator.
///   2. A single `\x1f` (US — unit separator) so kinds and titles
///      can never alias via concatenation collision.
///   3. The title, lower-cased and stripped of surrounding whitespace
///      (R5-Q9 calls for case-insensitive title matching across peers).
///
/// Returns the first 16 hex chars of the SHA-256 digest.
pub fn stable_id_for(kind: &CardKind, title: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.tag().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(normalize_title(title).as_bytes());
    let digest = hasher.finalize();
    hex16(&digest)
}

fn normalize_title(title: &str) -> String {
    title.trim().to_lowercase()
}

fn hex16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_16_lowercase_hex_chars() {
        let id = stable_id_for(&CardKind::App, "Firefox");
        assert_eq!(id.len(), 16, "got {id}");
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn id_is_deterministic() {
        let a = stable_id_for(&CardKind::App, "Firefox");
        let b = stable_id_for(&CardKind::App, "Firefox");
        assert_eq!(a, b);
    }

    #[test]
    fn id_normalizes_case() {
        let lower = stable_id_for(&CardKind::App, "firefox");
        let upper = stable_id_for(&CardKind::App, "FIREFOX");
        let mixed = stable_id_for(&CardKind::App, "FireFox");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[test]
    fn id_normalizes_surrounding_whitespace() {
        let bare = stable_id_for(&CardKind::App, "Firefox");
        let padded = stable_id_for(&CardKind::App, "   Firefox   ");
        assert_eq!(bare, padded);
    }

    #[test]
    fn id_differs_when_kind_differs() {
        let app = stable_id_for(&CardKind::App, "thing");
        let file = stable_id_for(&CardKind::File, "thing");
        assert_ne!(app, file);
    }

    #[test]
    fn id_differs_when_title_differs() {
        let a = stable_id_for(&CardKind::App, "alpha");
        let b = stable_id_for(&CardKind::App, "beta");
        assert_ne!(a, b);
    }

    #[test]
    fn unit_separator_prevents_kind_title_aliasing() {
        // Without the \x1f separator, ("app", "x") would collide with
        // ("ap", "px"). Confirm the separator does its job.
        let a = stable_id_for(&CardKind::App, "x");
        let b = stable_id_for(&CardKind::Other("ap".into()), "px");
        assert_ne!(a, b);
    }

    #[test]
    fn other_kind_carries_into_hash() {
        let custom_a = stable_id_for(&CardKind::Other("flavor".into()), "x");
        let custom_b = stable_id_for(&CardKind::Other("color".into()), "x");
        assert_ne!(custom_a, custom_b);
    }

    #[test]
    fn ids_are_pseudo_uniformly_distributed() {
        // Spot-check that consecutive titles don't share a 2-byte prefix
        // — that would indicate the hash isn't doing its job.
        let ids: Vec<_> = (0..32)
            .map(|i| stable_id_for(&CardKind::Note, &format!("title-{i}")))
            .collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[(i + 1)..] {
                assert_ne!(a, b);
                assert_ne!(&a[..4], &b[..4], "weak prefix uniqueness");
            }
        }
    }
}
