//! Target-resolution heuristic for the dialer display.
//!
//! Ported from the design bundle's `resolveTarget(target)` JavaScript
//! in `pjsip-interface/project/app.jsx`. Given the raw digit string
//! the operator has typed into the display field, classify it as one
//! of six states:
//!
//! - `Empty` — nothing typed yet, render the placeholder hint
//! - `Mesh { ok: true }` — exactly 4 digits starting with `1`, peer
//!   exists in the roster → Mesh chip + peer name
//! - `Mesh { ok: false }` — exactly 4 digits starting with `1`, peer
//!   NOT in the roster → 404 chip + "Not in mesh roster"
//! - `MeshPartial` — 1-3 digits starting with `1` → "N more digits"
//! - `Pstn` — `9` followed by 11 digits → PSTN chip + E.164 formatted
//! - `PstnPartial` — `9` followed by 1-10 digits → "N more digits via Vitelity"
//! - `Invalid` — any other prefix → error chip
//!
//! Per `docs/design/v6.0-pjsip-presence-and-hud.md` §1 Lock 4, the
//! mesh-peer roster used for membership checks is the live Bus
//! `mesh/roster` topic OR the fixture per VOIP-27.

use crate::roster::Peer;

/// The dialer's interpretation of the current display contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Field empty — show placeholder.
    Empty,
    /// 4-digit mesh extension, peer found.
    Mesh {
        name: String,
        hint: String,
        lan: bool,
    },
    /// 4-digit mesh extension, peer NOT in roster.
    MeshUnknown,
    /// 1-3 digits starting with `1`.
    MeshPartial { remaining: usize },
    /// `9` + 11 digits (PSTN E.164 with country prefix).
    Pstn { formatted: String },
    /// `9` + 1-10 digits.
    PstnPartial { remaining: usize },
    /// Doesn't match any prefix.
    Invalid,
}

impl Resolved {
    /// Whether the Call FAB should be enabled.
    #[must_use]
    pub const fn can_call(&self) -> bool {
        matches!(self, Resolved::Mesh { .. } | Resolved::Pstn { .. })
    }

    /// Whether this resolution will dial out over Vitelity (vs mesh).
    #[must_use]
    pub const fn is_pstn(&self) -> bool {
        matches!(self, Resolved::Pstn { .. } | Resolved::PstnPartial { .. })
    }
}

/// Resolve the raw digit string the operator typed.
///
/// `roster` is the current peer list (Bus or fixture).
#[must_use]
pub fn resolve_target(raw: &str, roster: &[Peer]) -> Resolved {
    if raw.is_empty() {
        return Resolved::Empty;
    }

    // Mesh extension: 4 digits starting with `1`.
    if raw.len() == 4 && raw.starts_with('1') && raw.chars().all(|c| c.is_ascii_digit()) {
        if let Some(p) = roster.iter().find(|p| p.ext == raw) {
            return Resolved::Mesh {
                name: p.name.clone(),
                hint: p.hint.clone(),
                lan: p.lan,
            };
        }
        return Resolved::MeshUnknown;
    }

    // PSTN: `9` + 11 digits (1 + 10-digit NANP E.164).
    if raw.len() == 12 && raw.starts_with('9') && raw.chars().all(|c| c.is_ascii_digit()) {
        return Resolved::Pstn {
            formatted: format_e164(&raw[1..]),
        };
    }

    // Mesh partial: 1-3 digits starting with `1`.
    if raw.len() <= 3 && raw.starts_with('1') && raw.chars().all(|c| c.is_ascii_digit()) {
        return Resolved::MeshPartial {
            remaining: 4 - raw.len(),
        };
    }

    // PSTN partial: `9` + 1-10 digits.
    if raw.len() <= 11 && raw.starts_with('9') && raw.chars().all(|c| c.is_ascii_digit()) {
        return Resolved::PstnPartial {
            remaining: 12 - raw.len(),
        };
    }

    Resolved::Invalid
}

/// Format an 11-digit NANP number (1 + 10 digits) as `+1 (NXX) NXX-XXXX`.
/// Falls back to `+<digits>` for non-NANP shapes.
fn format_e164(digits: &str) -> String {
    if digits.len() == 11 && digits.starts_with('1') {
        format!("+1 ({}) {}-{}", &digits[1..4], &digits[4..7], &digits[7..])
    } else {
        format!("+{digits}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Peer;

    fn fixture_roster() -> Vec<Peer> {
        vec![
            Peer {
                ext: "1001".into(),
                name: "alice-laptop".into(),
                role: "GUI".into(),
                presence: "available".into(),
                lan: true,
                hint: "Alice's ThinkPad".into(),
            },
            Peer {
                ext: "1004".into(),
                name: "nas-pi".into(),
                role: "Host".into(),
                presence: "available".into(),
                lan: true,
                hint: "Host peer".into(),
            },
        ]
    }

    #[test]
    fn empty_renders_empty() {
        assert_eq!(resolve_target("", &fixture_roster()), Resolved::Empty);
    }

    #[test]
    fn known_mesh_resolves_to_peer() {
        let r = resolve_target("1001", &fixture_roster());
        match r {
            Resolved::Mesh { name, lan, .. } => {
                assert_eq!(name, "alice-laptop");
                assert!(lan);
            }
            other => panic!("expected Mesh, got {other:?}"),
        }
    }

    #[test]
    fn unknown_mesh_extension_is_404() {
        assert_eq!(
            resolve_target("1999", &fixture_roster()),
            Resolved::MeshUnknown
        );
    }

    #[test]
    fn mesh_partial_counts_remaining() {
        assert_eq!(
            resolve_target("10", &fixture_roster()),
            Resolved::MeshPartial { remaining: 2 }
        );
    }

    #[test]
    fn pstn_full_formats_nanp() {
        match resolve_target("915558675309", &fixture_roster()) {
            Resolved::Pstn { formatted } => {
                assert_eq!(formatted, "+1 (555) 867-5309");
            }
            other => panic!("expected Pstn, got {other:?}"),
        }
    }

    #[test]
    fn pstn_partial_counts_remaining() {
        assert_eq!(
            resolve_target("9555", &fixture_roster()),
            Resolved::PstnPartial { remaining: 8 }
        );
    }

    #[test]
    fn other_prefixes_are_invalid() {
        assert_eq!(resolve_target("5", &fixture_roster()), Resolved::Invalid);
        assert_eq!(resolve_target("0123", &fixture_roster()), Resolved::Invalid);
    }

    #[test]
    fn can_call_only_for_full_targets() {
        assert!(!Resolved::Empty.can_call());
        assert!(!Resolved::MeshPartial { remaining: 2 }.can_call());
        assert!(!Resolved::MeshUnknown.can_call());
        assert!(Resolved::Mesh {
            name: "x".into(),
            hint: "y".into(),
            lan: true
        }
        .can_call());
        assert!(Resolved::Pstn {
            formatted: "+1".into()
        }
        .can_call());
    }
}
