//! Integration test: drive the whole `JellyfinClient` through a fixture
//! transport that replays recorded Jellyfin JSON — no live network. This is the
//! MEDIA-9 acceptance in one place: multi-server auth (both flows), the full
//! browse surface, artwork URLs, and the show-tree fold, all against fixtures.

use mde_jellyfin::{
    build_show_tree, group_by_type, image_url, BaseItemDto, ClientInfo, HttpRequest, HttpResponse,
    HttpTransport, ImageQuery, ImageType, ItemsQuery, JellyfinClient, JellyfinError, ServerAuth,
    ServerConfig, ServerStore, SortOrder, TransportError,
};

// ── recorded Jellyfin responses ───────────────────────────────────────────────
const AUTH_RESULT: &str = include_str!("fixtures/auth_result.json");
const QC_INITIATE: &str = include_str!("fixtures/quickconnect_initiate.json");
const QC_AUTHED: &str = include_str!("fixtures/quickconnect_authed.json");
const ITEMS_MOVIES: &str = include_str!("fixtures/items_movies.json");
const MIXED_LIBRARY: &str = include_str!("fixtures/mixed_library.json");
const SERIES_SEASONS: &str = include_str!("fixtures/series_seasons.json");
const SEASON_EPISODES: &str = include_str!("fixtures/season_episodes.json");
const NEXTUP: &str = include_str!("fixtures/nextup.json");
const RESUME: &str = include_str!("fixtures/resume.json");
const GENRES: &str = include_str!("fixtures/genres.json");

/// A transport that routes a request URL to the matching recorded fixture, so
/// the identical client code path runs with no network.
struct FixtureTransport {
    /// When set, the movies fixture is served with this alternate body (used to
    /// exercise the mixed music/collection library through the same route).
    items_body: &'static str,
}

impl FixtureTransport {
    const fn movies() -> Self {
        Self {
            items_body: ITEMS_MOVIES,
        }
    }

    const fn mixed() -> Self {
        Self {
            items_body: MIXED_LIBRARY,
        }
    }

    fn route(&self, url: &str) -> Option<&'static str> {
        // Most-specific paths first so `/Items/Resume` is not shadowed by
        // `/Items`, and the auth endpoints are matched before browse.
        if url.contains("/QuickConnect/Initiate") {
            Some(QC_INITIATE)
        } else if url.contains("/QuickConnect/Connect") {
            Some(QC_AUTHED)
        } else if url.contains("/Users/AuthenticateWithQuickConnect")
            || url.contains("/Users/AuthenticateByName")
        {
            Some(AUTH_RESULT)
        } else if url.contains("/Shows/NextUp") {
            Some(NEXTUP)
        } else if url.contains("/Seasons") {
            Some(SERIES_SEASONS)
        } else if url.contains("/Episodes") {
            Some(SEASON_EPISODES)
        } else if url.contains("/Items/Resume") {
            Some(RESUME)
        } else if url.contains("/Genres") {
            Some(GENRES)
        } else if url.contains("/Items") {
            Some(self.items_body)
        } else {
            None
        }
    }
}

impl HttpTransport for FixtureTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        Ok(self.route(&request.url).map_or_else(
            || HttpResponse {
                status: 404,
                body: b"{}".to_vec(),
            },
            |body| HttpResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
            },
        ))
    }
}

/// A transport that always fails at the wire level (connect/TLS/timeout).
struct DeadTransport;
impl HttpTransport for DeadTransport {
    fn execute(&self, _request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        Err(TransportError("connection refused".to_string()))
    }
}

/// A transport that always returns a given HTTP status with an empty body.
struct StatusTransport(u16);
impl HttpTransport for StatusTransport {
    fn execute(&self, _request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: self.0,
            body: b"{}".to_vec(),
        })
    }
}

fn device() -> ClientInfo {
    ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0")
}

/// A movies-serving client already signed in as the fixture user.
fn authed_client() -> JellyfinClient<FixtureTransport> {
    JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        FixtureTransport::movies(),
    )
    .with_auth("eyJhbGci-ACCESS-TOKEN", "user-9f3a")
}

#[test]
fn username_password_login_yields_token_and_user() {
    let client = JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        FixtureTransport::movies(),
    );
    let result = client
        .authenticate_by_name("matthew", "hunter2")
        .expect("login should parse");
    assert_eq!(result.access_token, "eyJhbGci-ACCESS-TOKEN");
    assert_eq!(result.user.id, "user-9f3a");
    assert_eq!(result.user.name, "matthew");

    // The saved projection carries the token + ids, never a password.
    let auth = result.into_auth();
    assert_eq!(auth.user_id, "user-9f3a");
    assert_eq!(auth.user_name.as_deref(), Some("matthew"));
}

#[test]
fn quick_connect_flow_initiate_poll_exchange() {
    let client = JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        FixtureTransport::movies(),
    );
    let initiated = client.quick_connect_initiate().expect("initiate");
    assert!(!initiated.authenticated);
    assert_eq!(initiated.code, "482913");
    assert_eq!(initiated.secret, "QC-SECRET-abc123");

    // Poll: the fixture returns the approved state.
    let state = client
        .quick_connect_state(&initiated.secret)
        .expect("poll state");
    assert!(state.authenticated, "user has approved");

    // Exchange the approved secret for a token.
    let result = client
        .authenticate_with_quick_connect(&state.secret)
        .expect("exchange");
    assert_eq!(result.access_token, "eyJhbGci-ACCESS-TOKEN");
    assert_eq!(result.user.id, "user-9f3a");
}

#[test]
fn browse_movies_deserializes_items_and_artwork() {
    let client = authed_client();
    let resp = client
        .items(
            &ItemsQuery::default()
                .include_item_types(["Movie"])
                .recursive()
                .sort_by(["SortName"])
                .sort_order(SortOrder::Ascending),
        )
        .expect("items");
    assert_eq!(resp.total_record_count, 2);
    assert_eq!(resp.items.len(), 2);

    let bbb = &resp.items[0];
    assert_eq!(bbb.name.as_deref(), Some("Big Buck Bunny"));
    assert_eq!(bbb.item_type.as_deref(), Some("Movie"));
    assert_eq!(bbb.production_year, Some(2008));
    assert_eq!(bbb.genres, vec!["Animation", "Short", "Comedy"]);
    let ud = bbb.user_data.as_ref().expect("user data");
    assert!(ud.played);
    assert_eq!(ud.play_count, 2);

    // Artwork URL construction from the item's Primary image tag.
    let tag = bbb.image_tags.get("Primary").expect("primary tag").clone();
    let url = image_url(
        client.base_url(),
        &bbb.id,
        ImageType::Primary,
        &ImageQuery {
            tag: Some(tag),
            max_width: Some(300),
            ..ImageQuery::default()
        },
    );
    assert_eq!(
        url,
        "https://jelly.mesh:8096/Items/movie-bbb/Images/Primary?tag=prim-bbb-tag&maxWidth=300"
    );
}

#[test]
fn browse_mixed_library_groups_music_and_collections() {
    let client = JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        FixtureTransport::mixed(),
    )
    .with_auth("T", "user-9f3a");
    let resp = client.items(&ItemsQuery::default()).expect("mixed items");
    let grouped = group_by_type(&resp.items);
    assert_eq!(grouped["MusicAlbum"].len(), 1);
    assert_eq!(grouped["MusicArtist"].len(), 1);
    assert_eq!(grouped["BoxSet"].len(), 1);
    assert_eq!(grouped["Movie"].len(), 1);
    // The collection reports its child count.
    assert_eq!(grouped["BoxSet"][0].child_count, Some(3));
}

#[test]
fn browse_show_tree_seasons_and_episodes_fold() {
    let client = authed_client();
    let series = BaseItemDto {
        id: "series-cosmos".into(),
        name: Some("Cosmos".into()),
        item_type: Some("Series".into()),
        ..BaseItemDto::default()
    };

    let seasons = client.seasons("series-cosmos").expect("seasons");
    assert_eq!(seasons.items.len(), 2);

    let episodes = client.episodes("series-cosmos", None).expect("episodes");
    assert_eq!(episodes.items.len(), 3);

    let tree = build_show_tree(series, seasons.items, episodes.items);
    assert_eq!(tree.seasons.len(), 2);
    // Season 1 before season 2.
    assert_eq!(tree.seasons[0].season.id, "season-s1");
    assert_eq!(tree.seasons[1].season.id, "season-s2");
    // Season 1 has both its episodes, ordered E1 then E2.
    let s1: Vec<&str> = tree.seasons[0]
        .episodes
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(s1, vec!["ep-s1e1", "ep-s1e2"]);
    // Season 2 has its one episode.
    assert_eq!(tree.seasons[1].episodes.len(), 1);
    assert_eq!(tree.seasons[1].episodes[0].id, "ep-s2e1");
}

#[test]
fn next_up_and_continue_watching() {
    let client = authed_client();

    let next = client.next_up(None).expect("next up");
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].id, "ep-s2e2");

    let resume = client.resume().expect("resume");
    assert_eq!(resume.items.len(), 1);
    let cont = &resume.items[0];
    assert_eq!(cont.id, "movie-sintel");
    let ud = cont.user_data.as_ref().expect("resume user data");
    assert_eq!(ud.playback_position_ticks, 3_000_000_000);
    assert!(!ud.played, "a resumable item is partially watched");
}

#[test]
fn genres_list() {
    let client = authed_client();
    let genres = client.genres(None).expect("genres");
    assert_eq!(genres.items.len(), 2);
    let names: Vec<&str> = genres
        .items
        .iter()
        .filter_map(|g| g.name.as_deref())
        .collect();
    assert_eq!(names, vec!["Animation", "Documentary"]);
}

#[test]
fn browse_without_auth_is_rejected_before_the_wire() {
    // No `with_auth` → no UserId → the browse never leaves the client.
    let client = JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        FixtureTransport::movies(),
    );
    let err = client
        .items(&ItemsQuery::default())
        .expect_err("must require auth");
    assert!(matches!(err, JellyfinError::NotAuthenticated));
}

#[test]
fn transport_failure_surfaces_as_transport_error() {
    let client =
        JellyfinClient::new("https://jelly.mesh:8096", device(), DeadTransport).with_auth("T", "u");
    let err = client.items(&ItemsQuery::default()).expect_err("dead wire");
    assert!(matches!(err, JellyfinError::Transport(_)));
}

#[test]
fn http_401_surfaces_as_http_error() {
    let client = JellyfinClient::new("https://jelly.mesh:8096", device(), StatusTransport(401))
        .with_auth("bad-token", "u");
    let err = client.items(&ItemsQuery::default()).expect_err("401");
    assert!(matches!(err, JellyfinError::Http { status: 401 }));
}

#[test]
fn http_404_surfaces_as_http_error() {
    // A route the fixture transport does not recognize returns 404.
    let client = JellyfinClient::new("https://jelly.mesh:8096", device(), StatusTransport(404))
        .with_auth("T", "u");
    let err = client.resume().expect_err("404");
    assert!(matches!(err, JellyfinError::Http { status: 404 }));
}

#[test]
fn multi_server_store_round_trip_persists_tokens() {
    // Two configured servers, one signed in — persisted + reloaded verbatim.
    let mut store = ServerStore::new();
    store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh:8096"));
    store.upsert(ServerConfig::new("srv-b", "Backup", "https://b.mesh:8096"));
    store.set_auth(
        "srv-a",
        ServerAuth {
            access_token: "eyJhbGci-ACCESS-TOKEN".into(),
            user_id: "user-9f3a".into(),
            user_name: Some("matthew".into()),
            server_id: Some("srv-77".into()),
        },
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("servers.json");
    store.save_to(&path).expect("save");

    let loaded = ServerStore::load_from(&path).expect("load");
    assert_eq!(loaded.servers.len(), 2);
    let a = loaded.get("srv-a").expect("srv-a");
    assert!(a.is_authenticated());
    assert_eq!(
        a.auth.as_ref().expect("auth").access_token,
        "eyJhbGci-ACCESS-TOKEN"
    );
    assert!(!loaded.get("srv-b").expect("srv-b").is_authenticated());
}
