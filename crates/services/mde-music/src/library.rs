//! AIR-10.b (v6.1) — library data over the Bus.
//!
//! The hub categories fetch their contents from the `mde-musicd` daemon
//! over the Bus (`action/music/{list-albums,list-artists,search}` →
//! `reply/<ulid>`) per the Q96 Bus-canonical lock — the GUI never talks
//! to Airsonic directly. [`parse_items`] flattens the daemon's reply
//! into display rows (pure + unit-tested); [`fetch`] is the async Bus
//! round-trip the Iced `Task` drives.

use std::time::Duration;

use crate::hub::HubCard;

/// One row in a library grid: a stable id + a display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryItem {
    pub id: String,
    pub label: String,
    /// AIR-11.c.3 — coverArt token for the per-card thumbnail (None when the
    /// section carries no cover art, e.g. genres / playlists).
    pub art_id: Option<String>,
}

/// The `action/music/<verb>` topic a hub card fetches from. `None` for
/// categories not yet backed by a daemon verb (only Radio —
/// AIR-4.b endpoint).
#[must_use]
pub fn verb_for(card: HubCard) -> Option<&'static str> {
    match card {
        HubCard::Albums => Some("list-albums"),
        HubCard::Artists => Some("list-artists"),
        HubCard::Genres => Some("list-genres"),
        HubCard::Podcasts => Some("list-podcasts"),
        HubCard::Recents => Some("list-recents"),
        HubCard::Playlists => Some("list-playlists"),
        _ => None,
    }
}

/// Parse the daemon's `{ok, result: {albums|artists|songs: [...]}}`
/// reply into display rows. Returns empty on `ok:false` / malformed /
/// missing sections (the view shows an honest empty state).
#[must_use]
pub fn parse_items(reply_json: &str) -> Vec<LibraryItem> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply_json) else {
        return Vec::new();
    };
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let result = match v.get("result") {
        Some(r) => r,
        None => return Vec::new(),
    };
    // Genres carry no `id` — the genre name (`value`) is both id + label.
    if let Some(arr) = result.get("genres").and_then(serde_json::Value::as_array) {
        return arr
            .iter()
            .filter_map(|g| {
                let value = g.get("value").and_then(serde_json::Value::as_str)?;
                Some(LibraryItem {
                    id: value.to_string(),
                    label: value.to_string(),
                    art_id: None,
                })
            })
            .collect();
    }
    // Try each id/label section; the first present one wins (a verb returns one).
    for (section, label_key) in [
        ("albums", "name"),
        ("artists", "name"),
        ("playlists", "name"),
        ("songs", "title"),
        ("podcasts", "title"),
        ("episodes", "title"),
    ] {
        if let Some(arr) = result.get(section).and_then(serde_json::Value::as_array) {
            return arr
                .iter()
                .filter_map(|item| {
                    let id = item.get("id").and_then(serde_json::Value::as_str)?;
                    let label = item
                        .get(label_key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(id);
                    let art_id = item
                        .get("coverArt")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    Some(LibraryItem {
                        id: id.to_string(),
                        label: label.to_string(),
                        art_id,
                    })
                })
                .collect();
        }
    }
    Vec::new()
}

/// Fetch a category's items from the daemon over the Bus. Returns an
/// error string (shown as an empty-state hint) when the Bus store is
/// unavailable or the daemon doesn't reply in time (not running).
///
/// # Errors
/// Bus-store open / request / timeout failures.
pub async fn fetch(verb: &'static str) -> Result<Vec<LibraryItem>, String> {
    // `Persist` (rusqlite) isn't `Send`, so the round-trip can't cross
    // Iced's multi-thread Task executor. Run it on a blocking thread with
    // a local current-thread runtime — only the `Send` `Vec` crosses back.
    tokio::task::spawn_blocking(move || -> Result<Vec<LibraryItem>, String> {
        let bus_root = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
        let persist =
            mde_bus::persist::Persist::open(bus_root).map_err(|e| format!("Bus store: {e}"))?;
        let topic = format!("action/music/{verb}");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let reply = rt
            .block_on(mde_bus::rpc::request(
                &persist,
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                None,
                Duration::from_secs(5),
            ))
            .map_err(|e| format!("daemon not responding ({e})"))?;
        Ok(parse_items(reply.body.as_deref().unwrap_or("")))
    })
    .await
    .map_err(|e| format!("fetch task join error: {e}"))?
}

/// Fetch the albums in a genre over the Bus (`action/music/albums-by-genre`,
/// the genre name in the body). The rows render like the album grid
/// (click → album page).
///
/// # Errors
/// Bus-store / request / timeout failures (daemon not running).
pub async fn fetch_albums_by_genre(genre: String) -> Result<Vec<LibraryItem>, String> {
    tokio::task::spawn_blocking(move || -> Result<Vec<LibraryItem>, String> {
        let bus_root = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
        let persist =
            mde_bus::persist::Persist::open(bus_root).map_err(|e| format!("Bus store: {e}"))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let reply = rt
            .block_on(mde_bus::rpc::request(
                &persist,
                "action/music/albums-by-genre",
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&genre),
                Duration::from_secs(5),
            ))
            .map_err(|e| format!("daemon not responding ({e})"))?;
        Ok(parse_items(reply.body.as_deref().unwrap_or("")))
    })
    .await
    .map_err(|e| format!("fetch task join error: {e}"))?
}

/// Fetch a podcast channel's episodes over the Bus
/// (`action/music/podcast-episodes`, the channel id in the body). The rows
/// render like any grid (each episode's id is its playable `streamId`).
///
/// # Errors
/// Bus-store / request / timeout failures (daemon not running).
pub async fn fetch_podcast_episodes(channel_id: String) -> Result<Vec<LibraryItem>, String> {
    tokio::task::spawn_blocking(move || -> Result<Vec<LibraryItem>, String> {
        let bus_root = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
        let persist =
            mde_bus::persist::Persist::open(bus_root).map_err(|e| format!("Bus store: {e}"))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let reply = rt
            .block_on(mde_bus::rpc::request(
                &persist,
                "action/music/podcast-episodes",
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&channel_id),
                Duration::from_secs(5),
            ))
            .map_err(|e| format!("daemon not responding ({e})"))?;
        Ok(parse_items(reply.body.as_deref().unwrap_or("")))
    })
    .await
    .map_err(|e| format!("fetch task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_mapping() {
        assert_eq!(verb_for(HubCard::Albums), Some("list-albums"));
        assert_eq!(verb_for(HubCard::Artists), Some("list-artists"));
        assert_eq!(verb_for(HubCard::Radio), None);
    }

    #[test]
    fn parse_albums_section() {
        let reply = r#"{"ok":true,"result":{"albums":[
            {"id":"a1","name":"Moon Safari"},
            {"id":"a2","name":"Discovery"}
        ]}}"#;
        let items = parse_items(reply);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "a1");
        assert_eq!(items[0].label, "Moon Safari");
    }

    #[test]
    fn parse_artists_section() {
        let reply = r#"{"ok":true,"result":{"artists":[{"id":"2","name":"Air"}]}}"#;
        assert_eq!(
            parse_items(reply),
            vec![LibraryItem {
                id: "2".into(),
                label: "Air".into(),
                art_id: None
            }]
        );
    }

    #[test]
    fn parse_songs_uses_title() {
        let reply = r#"{"ok":true,"result":{"songs":[{"id":"s1","title":"La Femme d'Argent"}]}}"#;
        assert_eq!(parse_items(reply)[0].label, "La Femme d'Argent");
    }

    #[test]
    fn parse_failures_are_empty() {
        assert!(parse_items(r#"{"ok":false,"error":"no server"}"#).is_empty());
        assert!(parse_items("not json").is_empty());
        assert!(parse_items(r#"{"ok":true}"#).is_empty());
        assert!(parse_items(r#"{"ok":true,"result":{}}"#).is_empty());
    }

    #[test]
    fn label_falls_back_to_id_when_missing() {
        let reply = r#"{"ok":true,"result":{"albums":[{"id":"only-id"}]}}"#;
        assert_eq!(parse_items(reply)[0].label, "only-id");
    }

    #[test]
    fn parse_podcasts_and_episodes_sections() {
        let pods =
            parse_items(r#"{"ok":true,"result":{"podcasts":[{"id":"c1","title":"Rust Talks"}]}}"#);
        assert_eq!(
            pods,
            vec![LibraryItem {
                id: "c1".into(),
                label: "Rust Talks".into(),
                art_id: None
            }]
        );
        let eps = parse_items(r#"{"ok":true,"result":{"episodes":[{"id":"s1","title":"Ep 1"}]}}"#);
        assert_eq!(eps[0].label, "Ep 1");
    }

    #[test]
    fn parse_genres_section_uses_value_as_id_and_label() {
        // The daemon's list-genres reply flattens to result.genres = [...].
        let reply = r#"{"ok":true,"result":{"genres":[
            {"value":"Jazz","albumCount":12},{"value":"Rock"}
        ]}}"#;
        let items = parse_items(reply);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            LibraryItem {
                id: "Jazz".into(),
                label: "Jazz".into(),
                art_id: None
            }
        );
        assert_eq!(items[1].label, "Rock");
    }

    #[test]
    fn verb_for_recents_and_playlists() {
        assert_eq!(verb_for(HubCard::Recents), Some("list-recents"));
        assert_eq!(verb_for(HubCard::Playlists), Some("list-playlists"));
        assert_eq!(verb_for(HubCard::Radio), None);
    }

    #[test]
    fn parse_playlists_section() {
        let reply = r#"{"ok":true,"result":{"playlists":[
            {"id":"pl1","name":"Roadtrip"},{"id":"pl2","name":"Focus"}
        ]}}"#;
        let items = parse_items(reply);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            LibraryItem {
                id: "pl1".into(),
                label: "Roadtrip".into(),
                art_id: None
            }
        );
        assert_eq!(items[1].label, "Focus");
    }

    #[test]
    fn parse_items_captures_cover_art() {
        let reply = r#"{"ok":true,"result":{"albums":[
            {"id":"a1","name":"Moon Safari","coverArt":"al-a1"}
        ]}}"#;
        let items = parse_items(reply);
        assert_eq!(items[0].art_id.as_deref(), Some("al-a1"));
    }
}
