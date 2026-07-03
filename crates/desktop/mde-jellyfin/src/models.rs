//! The typed serde projection of the Jellyfin REST responses.
//!
//! Jellyfin serves `PascalCase` JSON; each struct maps it to idiomatic
//! `snake_case` Rust via `#[serde(rename_all = "PascalCase")]`. Every field is
//! `#[serde(default)]`
//! or an `Option` so a trimmed / evolving server payload deserializes softly
//! rather than failing the whole browse.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The universal Jellyfin item — one row of any `/Items`-shaped response.
///
/// A single struct models every kind (`Series`, `Season`, `Episode`, `Movie`,
/// `BoxSet`, `MusicArtist`, `MusicAlbum`, `Audio`, …); [`item_type`](Self::item_type)
/// discriminates. The client only projects the fields the browse surface needs;
/// unknown fields are ignored.
///
/// (`Eq` is intentionally not derived — [`UserData::played_percentage`] is an
/// `f64`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    /// The item's server GUID (the id every child/image/season query keys on).
    pub id: String,
    /// The display name / title, if present.
    #[serde(default)]
    pub name: Option<String>,
    /// The item kind (`"Series"`, `"Season"`, `"Episode"`, `"Movie"`,
    /// `"BoxSet"`, `"MusicAlbum"`, `"MusicArtist"`, `"Audio"`, …).
    #[serde(rename = "Type", default)]
    pub item_type: Option<String>,
    /// The long description / synopsis, if present.
    #[serde(default)]
    pub overview: Option<String>,
    /// The production / release year, if reported.
    #[serde(default)]
    pub production_year: Option<i32>,
    /// The item's ordinal within its parent — the episode number, or the track
    /// number for audio.
    #[serde(default)]
    pub index_number: Option<i32>,
    /// The parent's ordinal — the season number for an episode.
    #[serde(default)]
    pub parent_index_number: Option<i32>,
    /// The owning series' GUID (set on seasons + episodes).
    #[serde(default)]
    pub series_id: Option<String>,
    /// The owning series' name (set on seasons + episodes).
    #[serde(default)]
    pub series_name: Option<String>,
    /// The owning season's GUID (set on episodes; the key
    /// [`build_show_tree`](crate::build_show_tree) folds on).
    #[serde(default)]
    pub season_id: Option<String>,
    /// The immediate parent's GUID (a series for a season, a library for a
    /// top-level view) — the `ParentId` a child query passes back.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// For a library view / collection: its kind (`"movies"`, `"tvshows"`,
    /// `"music"`, `"boxsets"`, `"playlists"`).
    #[serde(default)]
    pub collection_type: Option<String>,
    /// The runtime in 100-ns ticks (10 000 000 per second), if reported.
    #[serde(default)]
    pub run_time_ticks: Option<i64>,
    /// The item's genre labels.
    #[serde(default)]
    pub genres: Vec<String>,
    /// The count of direct children (seasons under a series, episodes under a
    /// season), if reported.
    #[serde(default)]
    pub child_count: Option<i32>,
    /// Image kind → tag map (`{"Primary": "<tag>", "Logo": "<tag>", …}`); the
    /// tag is fed to [`image_url`](crate::image_url) to build a cache-stable
    /// artwork URL.
    #[serde(default)]
    pub image_tags: BTreeMap<String, String>,
    /// The tags of the item's backdrop images (positional, not keyed).
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    /// The per-user playback state (resume position, played, favorite), if the
    /// query requested it.
    #[serde(default)]
    pub user_data: Option<UserData>,
}

/// The per-user playback state attached to a [`BaseItemDto`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    /// Where playback was last left, in 100-ns ticks — the resume point
    /// Continue-Watching restores.
    #[serde(default)]
    pub playback_position_ticks: i64,
    /// Whether the item is marked fully played.
    #[serde(default)]
    pub played: bool,
    /// How many times the item has been played.
    #[serde(default)]
    pub play_count: i32,
    /// Whether the user favorited the item.
    #[serde(default)]
    pub is_favorite: bool,
    /// For a folder (series/season): the count of unplayed children, if
    /// reported.
    #[serde(default)]
    pub unplayed_item_count: Option<i32>,
    /// The played fraction `0.0..=100.0` the server computed, if reported.
    #[serde(default)]
    pub played_percentage: Option<f64>,
}

/// The `QueryResult` envelope every `/Items`-shaped endpoint returns
/// (`/Users/{id}/Items`, `/Shows/Seasons`, `/Shows/Episodes`, `/Shows/NextUp`,
/// `/Items/Resume`, `/Genres`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsResponse {
    /// The page of items.
    #[serde(default)]
    pub items: Vec<BaseItemDto>,
    /// The total match count across all pages (for paging).
    #[serde(default)]
    pub total_record_count: i64,
    /// The start offset this page represents.
    #[serde(default)]
    pub start_index: i64,
}

/// The public user identity carried by an [`AuthenticationResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PublicUser {
    /// The user's GUID — the `UserId` every browse query is scoped to.
    pub id: String,
    /// The user's display name.
    #[serde(default)]
    pub name: String,
    /// The server GUID the user belongs to, if reported.
    #[serde(default)]
    pub server_id: Option<String>,
}

/// The result of a successful login (username/password or Quick Connect
/// exchange) — the `AccessToken` + the authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    /// The bearer token to put in the `Authorization` header of every
    /// subsequent request (see [`ServerAuth`](crate::ServerAuth)).
    pub access_token: String,
    /// The server GUID, if reported.
    #[serde(default)]
    pub server_id: Option<String>,
    /// The authenticated user (its [`PublicUser::id`] is the saved `UserId`).
    #[serde(default)]
    pub user: PublicUser,
}

/// The Quick Connect state — the response of both `/QuickConnect/Initiate` and
/// each `/QuickConnect/Connect` poll.
///
/// On initiate, [`authenticated`](Self::authenticated) is `false` and the
/// client shows the [`code`](Self::code) for the user to approve in an already
/// signed-in session; polling `/QuickConnect/Connect?secret=…` with the
/// [`secret`](Self::secret) flips `authenticated` to `true`, after which the
/// [`secret`](Self::secret) is exchanged for an [`AuthenticationResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnectState {
    /// Whether the user has approved this request yet.
    #[serde(default)]
    pub authenticated: bool,
    /// The opaque secret the client polls with and finally exchanges.
    #[serde(default)]
    pub secret: String,
    /// The short human code the user types into an authorized session.
    #[serde(default)]
    pub code: String,
    /// The device GUID the request is bound to, if reported.
    #[serde(default)]
    pub device_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_item_maps_pascalcase_and_type_alias() {
        let json = r#"{
            "Id": "abc",
            "Name": "The Show",
            "Type": "Series",
            "ProductionYear": 2019,
            "Genres": ["Drama","Sci-Fi"],
            "ImageTags": { "Primary": "tagp", "Logo": "tagl" },
            "UserData": { "PlaybackPositionTicks": 42, "Played": false, "PlayCount": 3 }
        }"#;
        let item: BaseItemDto = serde_json::from_str(json).expect("parse base item");
        assert_eq!(item.id, "abc");
        assert_eq!(item.name.as_deref(), Some("The Show"));
        assert_eq!(item.item_type.as_deref(), Some("Series"));
        assert_eq!(item.production_year, Some(2019));
        assert_eq!(item.genres, vec!["Drama", "Sci-Fi"]);
        assert_eq!(
            item.image_tags.get("Primary").map(String::as_str),
            Some("tagp")
        );
        let ud = item.user_data.expect("user data present");
        assert_eq!(ud.playback_position_ticks, 42);
        assert_eq!(ud.play_count, 3);
        assert!(!ud.played);
    }

    #[test]
    fn sparse_item_deserializes_softly() {
        // Only the required Id — every other field defaults, no error.
        let item: BaseItemDto = serde_json::from_str(r#"{"Id":"x"}"#).expect("sparse item");
        assert_eq!(item.id, "x");
        assert!(item.name.is_none());
        assert!(item.genres.is_empty());
        assert!(item.user_data.is_none());
    }

    #[test]
    fn items_envelope_parses_paging() {
        let json = r#"{"Items":[{"Id":"1"},{"Id":"2"}],"TotalRecordCount":57,"StartIndex":10}"#;
        let resp: ItemsResponse = serde_json::from_str(json).expect("parse envelope");
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.total_record_count, 57);
        assert_eq!(resp.start_index, 10);
    }

    #[test]
    fn empty_envelope_defaults() {
        let resp: ItemsResponse = serde_json::from_str("{}").expect("empty envelope");
        assert!(resp.items.is_empty());
        assert_eq!(resp.total_record_count, 0);
    }

    #[test]
    fn auth_result_projects_token_and_user() {
        let json = r#"{
            "AccessToken": "TOKEN-xyz",
            "ServerId": "srv-1",
            "User": { "Id": "user-9", "Name": "matthew" }
        }"#;
        let res: AuthenticationResult = serde_json::from_str(json).expect("parse auth");
        assert_eq!(res.access_token, "TOKEN-xyz");
        assert_eq!(res.server_id.as_deref(), Some("srv-1"));
        assert_eq!(res.user.id, "user-9");
        assert_eq!(res.user.name, "matthew");
    }

    #[test]
    fn quick_connect_state_flips_authenticated() {
        let pending: QuickConnectState =
            serde_json::from_str(r#"{"Authenticated":false,"Secret":"s","Code":"123456"}"#)
                .expect("parse pending");
        assert!(!pending.authenticated);
        assert_eq!(pending.code, "123456");

        let done: QuickConnectState =
            serde_json::from_str(r#"{"Authenticated":true,"Secret":"s","Code":"123456"}"#)
                .expect("parse done");
        assert!(done.authenticated);
    }
}
