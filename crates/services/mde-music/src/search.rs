//! AIR-14 (v6.1) — global search over the Bus.
//!
//! The top-bar search field sends `action/music/search` (the daemon
//! proxies Airsonic `search3`) and renders the reply's three sections —
//! Artists / Albums / Songs — in a sheet over the current page. Per the
//! Q96 Bus-canonical lock the GUI never calls Airsonic directly.
//!
//! [`parse_search`] flattens the daemon's reply into the three display
//! lists (pure + unit-tested); [`fetch_search`] is the async Bus
//! round-trip the debounced Iced `Task` drives; [`enqueue`] adds a song
//! result to the playback queue with one Bus request.

use std::time::Duration;

use serde_json::Value;

use crate::library::LibraryItem;

/// The debounce window before a keystroke triggers a search (Q11).
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// The three independently-scrollable result sections of a `search3`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// Artist matches, each row linking to the artist's album list.
    pub artists: Vec<LibraryItem>,
    /// Album matches, each row linking to the album page.
    pub albums: Vec<LibraryItem>,
    /// Song matches; clicking one enqueues the song via [`enqueue`].
    pub songs: Vec<LibraryItem>,
}

impl SearchResults {
    /// Whether every section is empty (no matches / not yet searched).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty() && self.albums.is_empty() && self.songs.is_empty()
    }
}

/// Parse one `result.<key>[]` section into display rows, taking each row's
/// `id` + the `label_key` field (falling back to the id).
fn section(result: &Value, key: &str, label_key: &str) -> Vec<LibraryItem> {
    result
        .get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?;
                    let label = item.get(label_key).and_then(Value::as_str).unwrap_or(id);
                    Some(LibraryItem {
                        id: id.to_string(),
                        label: label.to_string(),
                        art_id: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the daemon's `{ok, result: {artists, albums, songs}}` search
/// reply into the three sections. Empty on `ok:false` / malformed /
/// missing (the sheet shows an honest "no results" state).
#[must_use]
pub fn parse_search(reply_json: &str) -> SearchResults {
    let Ok(v) = serde_json::from_str::<Value>(reply_json) else {
        return SearchResults::default();
    };
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return SearchResults::default();
    }
    let Some(result) = v.get("result") else {
        return SearchResults::default();
    };
    SearchResults {
        artists: section(result, "artists", "name"),
        albums: section(result, "albums", "name"),
        songs: section(result, "songs", "title"),
    }
}

/// Run a search over the Bus (`action/music/search`, the query in the body
/// — the daemon's `search3` proxy). Mirrors [`crate::library::fetch`]'s
/// spawn-blocking round-trip (the rusqlite `Persist` isn't `Send`).
///
/// # Errors
/// Bus-store open / request / timeout failures (daemon not running).
pub async fn fetch_search(query: String) -> Result<SearchResults, String> {
    tokio::task::spawn_blocking(move || -> Result<SearchResults, String> {
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
                "action/music/search",
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&query),
                Duration::from_secs(5),
            ))
            .map_err(|e| format!("daemon not responding ({e})"))?;
        Ok(parse_search(reply.body.as_deref().unwrap_or("")))
    })
    .await
    .map_err(|e| format!("search task join error: {e}"))?
}

/// Add a song to the playback queue over the Bus (`action/music/enqueue`).
/// The click action for a song search result.
///
/// # Errors
/// Bus-store open / request / timeout failures (daemon not running).
pub async fn enqueue(song_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let bus_root = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
        let persist =
            mde_bus::persist::Persist::open(bus_root).map_err(|e| format!("Bus store: {e}"))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(mde_bus::rpc::request(
            &persist,
            "action/music/enqueue",
            mde_bus::hooks::config::Priority::Default,
            None,
            Some(&song_id),
            Duration::from_secs(5),
        ))
        .map_err(|e| format!("daemon not responding ({e})"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("enqueue task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_three_sections() {
        let reply = r#"{"ok":true,"result":{
            "artists":[{"id":"ar1","name":"Miles Davis"}],
            "albums":[{"id":"al1","name":"Kind of Blue"}],
            "songs":[{"id":"s1","title":"So What"}]
        }}"#;
        let r = parse_search(reply);
        assert_eq!(
            r.artists,
            vec![LibraryItem {
                id: "ar1".into(),
                label: "Miles Davis".into(),
                art_id: None
            }]
        );
        assert_eq!(r.albums[0].label, "Kind of Blue");
        assert_eq!(r.songs[0].label, "So What");
        assert!(!r.is_empty());
    }

    #[test]
    fn parse_partial_and_failures() {
        // Only one section present.
        let r = parse_search(r#"{"ok":true,"result":{"albums":[{"id":"a","name":"X"}]}}"#);
        assert!(r.artists.is_empty() && r.songs.is_empty());
        assert_eq!(r.albums.len(), 1);
        // ok:false / malformed / empty result → all empty.
        assert!(parse_search(r#"{"ok":false,"error":"no server"}"#).is_empty());
        assert!(parse_search("not json").is_empty());
        assert!(parse_search(r#"{"ok":true}"#).is_empty());
        assert!(parse_search(r#"{"ok":true,"result":{}}"#).is_empty());
    }

    #[test]
    fn song_label_falls_back_to_id() {
        let r = parse_search(r#"{"ok":true,"result":{"songs":[{"id":"only-id"}]}}"#);
        assert_eq!(r.songs[0].label, "only-id");
    }
}
