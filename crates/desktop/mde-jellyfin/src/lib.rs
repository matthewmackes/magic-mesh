//! `mde-jellyfin` — the reqwest+serde Jellyfin API client core (MEDIA-9).
//!
//! MCNF's media surface (the MEDIA epic) can browse & play from a Jellyfin
//! server. This crate is the load-bearing *client core*: a typed, headless
//! Jellyfin client that the `mde-media-egui` Sources plane consumes. No GUI
//! lives here — it is a clean typed client + models (§6 glue), not a
//! reimplementation of Jellyfin.
//!
//! # The seam (§7 runtime-real, testable without a network)
//!
//! The client is generic over an injectable [`HttpTransport`] — the one seam
//! between the typed endpoints and the wire. Every endpoint is built as a pure
//! [`HttpRequest`] and its response parsed by a pure serde struct, so the whole
//! browse surface is fixture-tested with **no live network**:
//!
//! - [`net::ReqwestTransport`] is the real implementation — a single
//!   `reqwest::blocking` client over rustls (airgap-safe to *compile*; only a
//!   live Jellyfin server, out of scope here, needs egress).
//! - In tests a fixture transport feeds recorded Jellyfin JSON bytes through the
//!   identical [`JellyfinClient`] code path.
//!
//! # Multi-server + saved auth
//!
//! [`ServerStore`] holds N configured [`ServerConfig`]s (each a base URL + an
//! optional saved [`ServerAuth`] = `AccessToken` + `UserId`) and round-trips
//! through a `0600` JSON at the user config dir ([`ServerStore::default_path`]).
//! **No plaintext password is ever persisted** — the username/password login
//! exchanges the password for a token at call time and only the token is stored.
//!
//! # Auth
//!
//! Both Jellyfin login flows are modelled as request builders + response
//! structs:
//!
//! - **Quick Connect**: [`JellyfinClient::quick_connect_initiate`] →
//!   [`JellyfinClient::quick_connect_state`] (poll until
//!   [`QuickConnectState::authenticated`]) →
//!   [`JellyfinClient::authenticate_with_quick_connect`].
//! - **Username / password**:
//!   [`JellyfinClient::authenticate_by_name`].
//!
//! Both yield an [`AuthenticationResult`] whose [`AuthenticationResult::into_auth`]
//! is the saved [`ServerAuth`].
//!
//! # Browse
//!
//! Typed request builders + serde responses cover the Jellyfin browse surface —
//! [`JellyfinClient::items`] (`/Users/{id}/Items`: movies, music, collections,
//! genre-filtered, and shows→seasons→episodes via `ParentId`),
//! [`JellyfinClient::seasons`] / [`JellyfinClient::episodes`],
//! [`JellyfinClient::next_up`] (`/Shows/NextUp`),
//! [`JellyfinClient::resume`] (`/Items/Resume`, Continue-Watching),
//! [`JellyfinClient::genres`] (`/Genres`), and the [`image_url`] artwork-URL
//! builder (`/Items/{id}/Images/{type}`). [`build_show_tree`] folds a flat
//! series+seasons+episodes fetch into the browsable [`ShowTree`].
//!
//! ```
//! use mde_jellyfin::{ClientInfo, ItemsQuery};
//!
//! // The pure request builder needs no transport — the URL + auth header are
//! // fully formed and unit-testable before any byte hits the wire.
//! let device = ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0");
//! let req = mde_jellyfin::build_items_request(
//!     "https://jelly.mesh:8096",
//!     "user-abc",
//!     &ItemsQuery::default().include_item_types(["Movie"]).recursive(),
//!     &device,
//!     Some("TOKEN"),
//! );
//! assert!(req.url.contains("/Users/user-abc/Items"));
//! assert!(req.url.contains("IncludeItemTypes=Movie"));
//! ```

// Pragmatic pedantic allows, matching the mde-media-core / mde-media-egui idiom:
// the type names intentionally echo their module (`HttpTransport` in `net`), and
// the pure accessors are convenience getters rather than a `#[must_use]`-critical
// API surface.
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

pub mod browse;
pub mod client;
pub mod models;
pub mod net;
pub mod store;

pub use browse::{build_show_tree, group_by_type, SeasonNode, ShowTree};
pub use client::{
    build_items_request, image_url, ClientInfo, ImageQuery, ImageType, ItemsQuery, JellyfinClient,
    JellyfinError, SortOrder,
};
pub use models::{
    AuthenticationResult, BaseItemDto, ItemsResponse, PublicUser, QuickConnectState, UserData,
};
pub use net::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TransportError,
};
pub use store::{ServerAuth, ServerConfig, ServerStore, StoreError};
