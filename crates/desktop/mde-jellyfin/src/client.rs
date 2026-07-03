//! The typed Jellyfin client — request builders + the transport-driving methods.
//!
//! Every endpoint is a pure builder that forms a complete [`HttpRequest`] (URL,
//! `Authorization` line, JSON body) and a serde parse of the response, so the
//! request shape is unit-testable without a network. [`JellyfinClient`] threads
//! those through an injected [`HttpTransport`]; a fixture transport exercises the
//! same code path in tests.

use serde::de::DeserializeOwned;

use crate::models::{AuthenticationResult, ItemsResponse, QuickConnectState};
use crate::net::{encode_query_component, HttpRequest, HttpResponse, HttpTransport};

/// The calling app's identity, sent in Jellyfin's `Authorization` header on
/// every request (Jellyfin binds the `AccessToken` to this device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// The application name (e.g. `"mde-media"`).
    pub client: String,
    /// A human device name (e.g. the workstation hostname).
    pub device: String,
    /// A stable per-install device id — the token is bound to it.
    pub device_id: String,
    /// The application version string.
    pub version: String,
}

impl ClientInfo {
    /// Build a device identity from its four parts.
    pub fn new(
        client: impl Into<String>,
        device: impl Into<String>,
        device_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            client: client.into(),
            device: device.into(),
            device_id: device_id.into(),
            version: version.into(),
        }
    }
}

/// Build Jellyfin's `Authorization` header value: the `MediaBrowser` scheme
/// with the client identity and, once logged in, the bearer `Token`.
///
/// Pre-auth requests (login, Quick Connect initiate/poll) pass `token = None`.
#[must_use]
pub fn authorization_header(device: &ClientInfo, token: Option<&str>) -> String {
    // Quotes are stripped from the identity fields so a stray quote can't break
    // the header grammar (the values are app-controlled, but be defensive).
    let sanitize = |s: &str| s.replace('"', "");
    let mut header = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        sanitize(&device.client),
        sanitize(&device.device),
        sanitize(&device.device_id),
        sanitize(&device.version),
    );
    if let Some(token) = token {
        header.push_str(", Token=\"");
        header.push_str(&sanitize(token));
        header.push('"');
    }
    header
}

/// Trim exactly one trailing `/` from a base URL so path joins never double it.
fn trim_base(base_url: &str) -> &str {
    base_url.strip_suffix('/').unwrap_or(base_url)
}

/// The standard JSON headers for a request, with the `Authorization` line.
fn json_headers(device: &ClientInfo, token: Option<&str>, post: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Authorization".to_string(),
            authorization_header(device, token),
        ),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if post {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    headers
}

/// Render `pairs` (skipping empty values) as a `?k=v&…` suffix with each value
/// percent-encoded. Returns an empty string when nothing is set.
fn render_query(pairs: &[(&str, String)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", encode_query_component(v)))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// The sort direction for [`ItemsQuery::sort_order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Ascending (A→Z, oldest first).
    Ascending,
    /// Descending (Z→A, newest first).
    Descending,
}

impl SortOrder {
    /// The Jellyfin wire token for this direction.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "Ascending",
            Self::Descending => "Descending",
        }
    }
}

/// A typed query for the `/Users/{id}/Items` browse endpoint.
///
/// Covers the whole browse surface: movies (`IncludeItemTypes=Movie`), music
/// (`MusicAlbum` / `MusicArtist` / `Audio`), collections (`BoxSet`),
/// genre-filtered listings (`genre_ids`), and shows→seasons→episodes navigation
/// (`parent_id`). Unset fields are omitted from the query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemsQuery {
    /// Restrict to children of this item (a library, a series, a season).
    pub parent_id: Option<String>,
    /// Restrict to these item kinds (`"Movie"`, `"Series"`, `"BoxSet"`, …).
    pub include_item_types: Vec<String>,
    /// Recurse into descendants rather than only direct children.
    pub recursive: bool,
    /// Sort keys (`"SortName"`, `"DatePlayed"`, `"ProductionYear"`, …).
    pub sort_by: Vec<String>,
    /// Sort direction.
    pub sort_order: Option<SortOrder>,
    /// Restrict to items in these genre GUIDs.
    pub genre_ids: Vec<String>,
    /// A free-text search term.
    pub search_term: Option<String>,
    /// Extra fields to hydrate (`"Overview"`, `"Genres"`, …).
    pub fields: Vec<String>,
    /// Page offset.
    pub start_index: Option<i64>,
    /// Page size.
    pub limit: Option<i64>,
}

impl ItemsQuery {
    /// Restrict to children of `parent_id` (a library / series / season).
    #[must_use]
    pub fn parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Restrict to these item kinds.
    #[must_use]
    pub fn include_item_types<I, S>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.include_item_types = types.into_iter().map(Into::into).collect();
        self
    }

    /// Recurse into all descendants (the usual mode for a flat library list).
    #[must_use]
    pub const fn recursive(mut self) -> Self {
        self.recursive = true;
        self
    }

    /// Sort by these keys.
    #[must_use]
    pub fn sort_by<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sort_by = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Set the sort direction.
    #[must_use]
    pub const fn sort_order(mut self, order: SortOrder) -> Self {
        self.sort_order = Some(order);
        self
    }

    /// Restrict to these genre GUIDs.
    #[must_use]
    pub fn genre_ids<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.genre_ids = ids.into_iter().map(Into::into).collect();
        self
    }

    /// Set a free-text search term.
    #[must_use]
    pub fn search_term(mut self, term: impl Into<String>) -> Self {
        self.search_term = Some(term.into());
        self
    }

    /// Hydrate these extra fields.
    #[must_use]
    pub fn fields<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Set the page offset + size.
    #[must_use]
    pub const fn page(mut self, start_index: i64, limit: i64) -> Self {
        self.start_index = Some(start_index);
        self.limit = Some(limit);
        self
    }

    /// The rendered query pairs for this browse query (order stable for tests).
    fn to_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("ParentId", self.parent_id.clone().unwrap_or_default()),
            ("IncludeItemTypes", self.include_item_types.join(",")),
            (
                "Recursive",
                if self.recursive {
                    "true".to_string()
                } else {
                    String::new()
                },
            ),
            ("SortBy", self.sort_by.join(",")),
            (
                "SortOrder",
                self.sort_order
                    .map(|o| o.as_str().to_string())
                    .unwrap_or_default(),
            ),
            ("GenreIds", self.genre_ids.join(",")),
            ("SearchTerm", self.search_term.clone().unwrap_or_default()),
            ("Fields", self.fields.join(",")),
            (
                "StartIndex",
                self.start_index.map(|n| n.to_string()).unwrap_or_default(),
            ),
            (
                "Limit",
                self.limit.map(|n| n.to_string()).unwrap_or_default(),
            ),
        ]
    }
}

/// Build the `/Users/{user_id}/Items` browse request.
///
/// The workhorse of the browse surface — movies, music, collections, genres, and
/// shows→seasons→episodes navigation all flow through it via [`ItemsQuery`].
#[must_use]
pub fn build_items_request(
    base_url: &str,
    user_id: &str,
    query: &ItemsQuery,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Users/{}/Items{}",
        trim_base(base_url),
        user_id,
        render_query(&query.to_pairs()),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Shows/{series_id}/Seasons` request (a series' seasons).
fn build_seasons_request(
    base_url: &str,
    series_id: &str,
    user_id: &str,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Shows/{}/Seasons{}",
        trim_base(base_url),
        series_id,
        render_query(&[("userId", user_id.to_string())]),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Shows/{series_id}/Episodes` request (a series' — optionally one
/// season's — episodes).
fn build_episodes_request(
    base_url: &str,
    series_id: &str,
    season_id: Option<&str>,
    user_id: &str,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Shows/{}/Episodes{}",
        trim_base(base_url),
        series_id,
        render_query(&[
            ("userId", user_id.to_string()),
            ("seasonId", season_id.unwrap_or_default().to_string()),
        ]),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Shows/NextUp` request (the Next-Up row, optionally for one
/// series).
fn build_next_up_request(
    base_url: &str,
    user_id: &str,
    series_id: Option<&str>,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Shows/NextUp{}",
        trim_base(base_url),
        render_query(&[
            ("userId", user_id.to_string()),
            ("seriesId", series_id.unwrap_or_default().to_string()),
        ]),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Items/Resume` request (the Continue-Watching row).
fn build_resume_request(
    base_url: &str,
    user_id: &str,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Items/Resume{}",
        trim_base(base_url),
        render_query(&[("userId", user_id.to_string())]),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Genres` request (the genre list, optionally under one library).
fn build_genres_request(
    base_url: &str,
    user_id: &str,
    parent_id: Option<&str>,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Genres{}",
        trim_base(base_url),
        render_query(&[
            ("userId", user_id.to_string()),
            ("parentId", parent_id.unwrap_or_default().to_string()),
        ]),
    );
    HttpRequest::get(url, json_headers(device, token, false))
}

/// Build the `/Users/AuthenticateByName` (username/password) login request.
///
/// The password rides only in the request body — it is never persisted (see
/// [`ServerStore`](crate::ServerStore)).
fn build_authenticate_by_name_request(
    base_url: &str,
    username: &str,
    password: &str,
    device: &ClientInfo,
) -> HttpRequest {
    let url = format!("{}/Users/AuthenticateByName", trim_base(base_url));
    let body = serde_json::json!({ "Username": username, "Pw": password });
    HttpRequest::post(
        url,
        json_headers(device, None, true),
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

/// Build the `/QuickConnect/Initiate` request.
fn build_quick_connect_initiate_request(base_url: &str, device: &ClientInfo) -> HttpRequest {
    let url = format!("{}/QuickConnect/Initiate", trim_base(base_url));
    HttpRequest::get(url, json_headers(device, None, false))
}

/// Build the `/QuickConnect/Connect?secret=…` poll request.
fn build_quick_connect_state_request(
    base_url: &str,
    secret: &str,
    device: &ClientInfo,
) -> HttpRequest {
    let url = format!(
        "{}/QuickConnect/Connect{}",
        trim_base(base_url),
        render_query(&[("secret", secret.to_string())]),
    );
    HttpRequest::get(url, json_headers(device, None, false))
}

/// Build the `/Users/AuthenticateWithQuickConnect` exchange request.
fn build_authenticate_with_quick_connect_request(
    base_url: &str,
    secret: &str,
    device: &ClientInfo,
) -> HttpRequest {
    let url = format!("{}/Users/AuthenticateWithQuickConnect", trim_base(base_url));
    let body = serde_json::json!({ "Secret": secret });
    HttpRequest::post(
        url,
        json_headers(device, None, true),
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

/// The kind of artwork to build a URL for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    /// The primary poster / cover.
    Primary,
    /// A wide backdrop.
    Backdrop,
    /// A thumbnail.
    Thumb,
    /// A transparent logo.
    Logo,
    /// A banner.
    Banner,
    /// Fan art.
    Art,
}

impl ImageType {
    /// The Jellyfin path segment for this image kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "Primary",
            Self::Backdrop => "Backdrop",
            Self::Thumb => "Thumb",
            Self::Logo => "Logo",
            Self::Banner => "Banner",
            Self::Art => "Art",
        }
    }
}

/// The sizing / cache-key parameters of an artwork URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageQuery {
    /// The image tag from [`BaseItemDto::image_tags`](crate::BaseItemDto::image_tags)
    /// — makes the URL cache-stable and picks the right version.
    pub tag: Option<String>,
    /// A max width the server downscales to.
    pub max_width: Option<u32>,
    /// A max height the server downscales to.
    pub max_height: Option<u32>,
    /// A JPEG quality `0..=100`.
    pub quality: Option<u8>,
}

/// Build the artwork URL for an item's image
/// (`/Items/{id}/Images/{type}?tag=…&maxWidth=…`).
///
/// A pure URL builder (no request / no auth needed — Jellyfin serves item images
/// unauthenticated) that an `<img>`/texture loader fetches directly.
#[must_use]
pub fn image_url(
    base_url: &str,
    item_id: &str,
    image_type: ImageType,
    query: &ImageQuery,
) -> String {
    let pairs = [
        ("tag", query.tag.clone().unwrap_or_default()),
        (
            "maxWidth",
            query.max_width.map(|n| n.to_string()).unwrap_or_default(),
        ),
        (
            "maxHeight",
            query.max_height.map(|n| n.to_string()).unwrap_or_default(),
        ),
        (
            "quality",
            query.quality.map(|n| n.to_string()).unwrap_or_default(),
        ),
    ];
    format!(
        "{}/Items/{}/Images/{}{}",
        trim_base(base_url),
        item_id,
        image_type.as_str(),
        render_query(&pairs),
    )
}

/// A failure from a Jellyfin client call.
#[derive(Debug, thiserror::Error)]
pub enum JellyfinError {
    /// The transport could not complete the round-trip (connect / TLS / read).
    #[error("jellyfin transport failed: {0}")]
    Transport(String),
    /// The server answered with a non-2xx HTTP status (401 = bad creds/token).
    #[error("jellyfin returned HTTP {status}")]
    Http {
        /// The HTTP status code.
        status: u16,
    },
    /// The response body was not the expected JSON shape.
    #[error("jellyfin response parse error: {0}")]
    Parse(String),
    /// A browse call was made before a `UserId` was set (no saved auth).
    #[error("jellyfin client is not authenticated (no UserId)")]
    NotAuthenticated,
}

/// A typed, headless Jellyfin client over an injected [`HttpTransport`].
///
/// Holds one server's base URL + device identity and, once logged in, the bearer
/// token + `UserId`. Auth-free calls (login, Quick Connect) work before auth;
/// browse calls require a `UserId` and error [`JellyfinError::NotAuthenticated`]
/// otherwise.
#[derive(Debug, Clone)]
pub struct JellyfinClient<T> {
    base_url: String,
    device: ClientInfo,
    token: Option<String>,
    user_id: Option<String>,
    transport: T,
}

impl<T: HttpTransport> JellyfinClient<T> {
    /// A client for `base_url` identifying itself as `device`, not yet
    /// authenticated.
    pub fn new(base_url: impl Into<String>, device: ClientInfo, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            device,
            token: None,
            user_id: None,
            transport,
        }
    }

    /// Attach a saved token + `UserId` (builder form).
    #[must_use]
    pub fn with_auth(mut self, token: impl Into<String>, user_id: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self.user_id = Some(user_id.into());
        self
    }

    /// Attach / replace the token + `UserId` in place (e.g. right after login).
    pub fn set_auth(&mut self, token: impl Into<String>, user_id: impl Into<String>) {
        self.token = Some(token.into());
        self.user_id = Some(user_id.into());
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The authenticated `UserId`, if any.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// The `UserId`, or [`JellyfinError::NotAuthenticated`].
    fn require_user(&self) -> Result<&str, JellyfinError> {
        self.user_id
            .as_deref()
            .ok_or(JellyfinError::NotAuthenticated)
    }

    /// Execute `request` and deserialize a 2xx JSON body into `R`.
    fn send<R: DeserializeOwned>(&self, request: &HttpRequest) -> Result<R, JellyfinError> {
        let response: HttpResponse = self
            .transport
            .execute(request)
            .map_err(|e| JellyfinError::Transport(e.to_string()))?;
        if !response.is_success() {
            return Err(JellyfinError::Http {
                status: response.status,
            });
        }
        serde_json::from_slice(&response.body).map_err(|e| JellyfinError::Parse(e.to_string()))
    }

    // ── auth ────────────────────────────────────────────────────────────────

    /// Log in with a username + password, returning the [`AuthenticationResult`]
    /// (token + user). The password is sent once and never stored.
    ///
    /// # Errors
    /// [`JellyfinError::Http`] on bad credentials (401), or a transport / parse
    /// error.
    pub fn authenticate_by_name(
        &self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticationResult, JellyfinError> {
        let req =
            build_authenticate_by_name_request(&self.base_url, username, password, &self.device);
        self.send(&req)
    }

    /// Initiate a Quick Connect request; the returned [`QuickConnectState`]
    /// carries the [`code`](QuickConnectState::code) to show the user and the
    /// [`secret`](QuickConnectState::secret) to poll with.
    ///
    /// # Errors
    /// [`JellyfinError::Http`] if Quick Connect is disabled (401/403), or a
    /// transport / parse error.
    pub fn quick_connect_initiate(&self) -> Result<QuickConnectState, JellyfinError> {
        let req = build_quick_connect_initiate_request(&self.base_url, &self.device);
        self.send(&req)
    }

    /// Poll a Quick Connect request's state; when
    /// [`authenticated`](QuickConnectState::authenticated) is `true` the
    /// `secret` may be exchanged via
    /// [`authenticate_with_quick_connect`](Self::authenticate_with_quick_connect).
    ///
    /// # Errors
    /// [`JellyfinError::Http`] if the secret is unknown / expired, or a transport
    /// / parse error.
    pub fn quick_connect_state(&self, secret: &str) -> Result<QuickConnectState, JellyfinError> {
        let req = build_quick_connect_state_request(&self.base_url, secret, &self.device);
        self.send(&req)
    }

    /// Exchange an approved Quick Connect `secret` for an
    /// [`AuthenticationResult`] (token + user).
    ///
    /// # Errors
    /// [`JellyfinError::Http`] if the secret is not yet approved / has expired,
    /// or a transport / parse error.
    pub fn authenticate_with_quick_connect(
        &self,
        secret: &str,
    ) -> Result<AuthenticationResult, JellyfinError> {
        let req =
            build_authenticate_with_quick_connect_request(&self.base_url, secret, &self.device);
        self.send(&req)
    }

    // ── browse (require a UserId) ─────────────────────────────────────────────

    /// Browse items via `/Users/{id}/Items` (movies, music, collections,
    /// genre-filtered, or a parent's children).
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn items(&self, query: &ItemsQuery) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_items_request(
            &self.base_url,
            user,
            query,
            &self.device,
            self.token.as_deref(),
        );
        self.send(&req)
    }

    /// A series' seasons via `/Shows/{series_id}/Seasons`.
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn seasons(&self, series_id: &str) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_seasons_request(
            &self.base_url,
            series_id,
            user,
            &self.device,
            self.token.as_deref(),
        );
        self.send(&req)
    }

    /// A series' episodes (optionally scoped to one `season_id`) via
    /// `/Shows/{series_id}/Episodes`.
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn episodes(
        &self,
        series_id: &str,
        season_id: Option<&str>,
    ) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_episodes_request(
            &self.base_url,
            series_id,
            season_id,
            user,
            &self.device,
            self.token.as_deref(),
        );
        self.send(&req)
    }

    /// The Next-Up row via `/Shows/NextUp` (optionally for one series).
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn next_up(&self, series_id: Option<&str>) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_next_up_request(
            &self.base_url,
            user,
            series_id,
            &self.device,
            self.token.as_deref(),
        );
        self.send(&req)
    }

    /// The Continue-Watching row via `/Items/Resume`.
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn resume(&self) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_resume_request(&self.base_url, user, &self.device, self.token.as_deref());
        self.send(&req)
    }

    /// The genre list via `/Genres` (optionally under one library `parent_id`).
    ///
    /// # Errors
    /// [`JellyfinError::NotAuthenticated`] with no `UserId`, else a transport /
    /// HTTP / parse error.
    pub fn genres(&self, parent_id: Option<&str>) -> Result<ItemsResponse, JellyfinError> {
        let user = self.require_user()?;
        let req = build_genres_request(
            &self.base_url,
            user,
            parent_id,
            &self.device,
            self.token.as_deref(),
        );
        self.send(&req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::HttpMethod;

    fn device() -> ClientInfo {
        ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0")
    }

    #[test]
    fn authorization_header_omits_token_when_none() {
        let h = authorization_header(&device(), None);
        assert!(h.starts_with("MediaBrowser "));
        assert!(h.contains("Client=\"mde-media\""));
        assert!(h.contains("DeviceId=\"device-42\""));
        assert!(!h.contains("Token="));
    }

    #[test]
    fn authorization_header_includes_token_when_present() {
        let h = authorization_header(&device(), Some("THE-TOKEN"));
        assert!(h.contains("Token=\"THE-TOKEN\""));
    }

    #[test]
    fn authorization_header_strips_quotes_defensively() {
        let dev = ClientInfo::new("mde\"media", "d", "id", "1");
        let h = authorization_header(&dev, None);
        assert!(h.contains("Client=\"mdemedia\""));
    }

    #[test]
    fn trim_base_strips_one_trailing_slash() {
        assert_eq!(trim_base("https://j.mesh/"), "https://j.mesh");
        assert_eq!(trim_base("https://j.mesh"), "https://j.mesh");
    }

    #[test]
    fn items_request_builds_path_and_query() {
        let q = ItemsQuery::default()
            .include_item_types(["Movie", "Series"])
            .recursive()
            .sort_by(["SortName"])
            .sort_order(SortOrder::Descending)
            .fields(["Overview", "Genres"])
            .page(0, 50);
        let req = build_items_request("https://j.mesh/", "user-1", &q, &device(), Some("T"));
        assert_eq!(req.method, HttpMethod::Get);
        assert!(req.url.starts_with("https://j.mesh/Users/user-1/Items?"));
        assert!(req.url.contains("IncludeItemTypes=Movie%2CSeries"));
        assert!(req.url.contains("Recursive=true"));
        assert!(req.url.contains("SortBy=SortName"));
        assert!(req.url.contains("SortOrder=Descending"));
        assert!(req.url.contains("Fields=Overview%2CGenres"));
        assert!(req.url.contains("StartIndex=0"));
        assert!(req.url.contains("Limit=50"));
        // The Authorization line carries the token.
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v.contains("Token=\"T\"")));
    }

    #[test]
    fn empty_items_query_has_no_query_string() {
        let req = build_items_request(
            "https://j.mesh",
            "u",
            &ItemsQuery::default(),
            &device(),
            None,
        );
        assert_eq!(req.url, "https://j.mesh/Users/u/Items");
    }

    #[test]
    fn search_term_is_percent_encoded() {
        let q = ItemsQuery::default().search_term("the martian");
        let req = build_items_request("https://j.mesh", "u", &q, &device(), None);
        assert!(req.url.contains("SearchTerm=the%20martian"));
    }

    #[test]
    fn seasons_and_episodes_requests() {
        let s = build_seasons_request("https://j.mesh", "series-9", "u", &device(), Some("T"));
        assert_eq!(s.url, "https://j.mesh/Shows/series-9/Seasons?userId=u");

        let e = build_episodes_request(
            "https://j.mesh",
            "series-9",
            Some("season-3"),
            "u",
            &device(),
            Some("T"),
        );
        assert!(e.url.starts_with("https://j.mesh/Shows/series-9/Episodes?"));
        assert!(e.url.contains("userId=u"));
        assert!(e.url.contains("seasonId=season-3"));

        // No season → the seasonId pair drops out.
        let e_all =
            build_episodes_request("https://j.mesh", "series-9", None, "u", &device(), None);
        assert_eq!(e_all.url, "https://j.mesh/Shows/series-9/Episodes?userId=u");
    }

    #[test]
    fn next_up_resume_genres_requests() {
        let n = build_next_up_request("https://j.mesh", "u", Some("series-1"), &device(), None);
        assert!(n.url.contains("/Shows/NextUp?"));
        assert!(n.url.contains("seriesId=series-1"));

        let r = build_resume_request("https://j.mesh", "u", &device(), None);
        assert_eq!(r.url, "https://j.mesh/Items/Resume?userId=u");

        let g = build_genres_request("https://j.mesh", "u", Some("lib-1"), &device(), None);
        assert!(g.url.contains("/Genres?"));
        assert!(g.url.contains("parentId=lib-1"));
    }

    #[test]
    fn auth_by_name_body_has_password_only_in_body() {
        let req =
            build_authenticate_by_name_request("https://j.mesh", "alice", "sesame", &device());
        assert_eq!(req.url, "https://j.mesh/Users/AuthenticateByName");
        let body = req.body.expect("post body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body json");
        assert_eq!(json["Username"], "alice");
        assert_eq!(json["Pw"], "sesame");
        // The password appears nowhere in the URL or headers.
        assert!(!req.url.contains("sesame"));
        assert!(req.headers.iter().all(|(_, v)| !v.contains("sesame")));
    }

    #[test]
    fn quick_connect_requests() {
        let i = build_quick_connect_initiate_request("https://j.mesh", &device());
        assert_eq!(i.url, "https://j.mesh/QuickConnect/Initiate");

        let s = build_quick_connect_state_request("https://j.mesh", "sec-abc", &device());
        assert_eq!(s.url, "https://j.mesh/QuickConnect/Connect?secret=sec-abc");

        let x =
            build_authenticate_with_quick_connect_request("https://j.mesh", "sec-abc", &device());
        assert_eq!(x.url, "https://j.mesh/Users/AuthenticateWithQuickConnect");
        let body: serde_json::Value = serde_json::from_slice(&x.body.expect("body")).expect("json");
        assert_eq!(body["Secret"], "sec-abc");
    }

    #[test]
    fn image_url_builds_primary_with_tag_and_size() {
        let url = image_url(
            "https://j.mesh/",
            "item-7",
            ImageType::Primary,
            &ImageQuery {
                tag: Some("abc123".to_string()),
                max_width: Some(400),
                ..ImageQuery::default()
            },
        );
        assert!(url.starts_with("https://j.mesh/Items/item-7/Images/Primary?"));
        assert!(url.contains("tag=abc123"));
        assert!(url.contains("maxWidth=400"));
        assert!(!url.contains("maxHeight="));
    }

    #[test]
    fn image_url_without_query_is_bare() {
        let url = image_url(
            "https://j.mesh",
            "i",
            ImageType::Backdrop,
            &ImageQuery::default(),
        );
        assert_eq!(url, "https://j.mesh/Items/i/Images/Backdrop");
    }
}
