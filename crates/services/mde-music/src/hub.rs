//! AIR-10 (v6.1) — the 7-card library hub.
//!
//! The hub is `mde-music`'s default landing (and every breadcrumb-root
//! click): seven category cards — Albums / Artists / Playlists / Recents
//! / Genres / Podcasts / Radio. This module is the pure card model
//! (labels + the canonical order); the Iced view renders them and the
//! live grids behind each card land with the daemon data path
//! (AIR-10.b).

/// One of the seven hub categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubCard {
    Albums,
    Artists,
    Playlists,
    Recents,
    Genres,
    Podcasts,
    Radio,
}

impl HubCard {
    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Playlists => "Playlists",
            Self::Recents => "Recents",
            Self::Genres => "Genres",
            Self::Podcasts => "Podcasts",
            Self::Radio => "Radio",
        }
    }

    /// The seven cards in canonical hub order.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Albums,
            Self::Artists,
            Self::Playlists,
            Self::Recents,
            Self::Genres,
            Self::Podcasts,
            Self::Radio,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_cards_in_canonical_order() {
        let all = HubCard::all();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], HubCard::Albums);
        assert_eq!(all[6], HubCard::Radio);
    }

    #[test]
    fn every_card_has_a_nonempty_label() {
        for c in HubCard::all() {
            assert!(!c.label().is_empty());
        }
        assert_eq!(HubCard::Playlists.label(), "Playlists");
    }
}
