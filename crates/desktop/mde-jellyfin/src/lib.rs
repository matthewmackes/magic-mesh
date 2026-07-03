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
//!
//! # Playback + sync (MEDIA-10)
//!
//! [`playback`] negotiates how to play a title: [`decide_method`] chooses
//! direct-play / direct-stream / transcode from a [`ClientCapabilities`] set (the
//! player's decode profile, built in the app from `mde-media-core`'s
//! `MpvCapabilities`) and an item's [`MediaSourceInfo`], and
//! [`build_playback_decision`] forms the stream URL — pure, fixture-tested folds.
//! [`sync`] is the progress loop: [`PlaybackReport`] +
//! [`build_report_start_request`] / [`build_report_progress_request`] /
//! [`build_report_stopped_request`] (`/Sessions/Playing*`), cross-device resume
//! via [`resume_position_secs`], and mark-played. Live-TV / DVR
//! ([`JellyfinClient::live_tv_channels`] / [`live_tv_guide`](JellyfinClient::live_tv_guide)
//! / [`recordings`](JellyfinClient::recordings)) + music playback ride the same
//! client patterns; a real server round-trip is honest-gated, the builders + the
//! negotiation are tested.
//!
//! # Offline + profiles (MEDIA-11)
//!
//! [`cache`] downloads a title's untouched direct-play bytes through the same
//! [`HttpTransport`] seam ([`JellyfinClient::download`]) into a managed local
//! [`OfflineCache`] — a cache root + JSON manifest with a real lifecycle (add /
//! evict / size-budget / staleness), so a title plays from a local
//! [`local_path`](OfflineCache::local_path) with no server. And a
//! [`ServerConfig`](store::ServerConfig) now holds **N user profiles** (each its
//! own [`ServerAuth`]), one active at a time
//! ([`switch_profile`](store::ServerConfig::switch_profile)) — per-profile token
//! isolation, in the same [`ServerStore`]. Both are fixture-tested: the cache with
//! synthetic bytes, the profiles as pure store folds.

// Pragmatic pedantic allows, matching the mde-media-core / mde-media-egui idiom:
// the type names intentionally echo their module (`HttpTransport` in `net`), and
// the pure accessors are convenience getters rather than a `#[must_use]`-critical
// API surface.
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

pub mod browse;
pub mod cache;
pub mod client;
pub mod models;
pub mod net;
pub mod playback;
pub mod store;
pub mod sync;

pub use browse::{build_show_tree, group_by_type, SeasonNode, ShowTree};
pub use cache::{CacheEntry, CacheError, CacheRequest, OfflineCache};
pub use client::{
    build_items_request, image_url, ClientInfo, ImageQuery, ImageType, ItemsQuery, JellyfinClient,
    JellyfinError, SortOrder,
};
pub use models::{
    AuthenticationResult, BaseItemDto, ItemsResponse, MediaSourceInfo, MediaStream,
    PlaybackInfoResponse, PublicUser, QuickConnectState, StreamKind, UserData,
};
pub use net::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TransportError,
};
pub use playback::{
    build_playback_decision, build_playback_info_request, decide_method, direct_play_url,
    direct_stream_url, transcode_url, ClientCapabilities, PlaybackDecision, PlaybackMethod,
    StreamMediaType,
};
pub use store::{ServerAuth, ServerConfig, ServerStore, StoreError};
pub use sync::{
    build_mark_played_request, build_mark_unplayed_request, build_report_progress_request,
    build_report_start_request, build_report_stopped_request, resume_position_secs, secs_to_ticks,
    ticks_to_secs, PlaybackReport, TICKS_PER_SECOND,
};
