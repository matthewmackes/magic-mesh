//! Hostile Jellyfin outage fixtures for the client/cache boundary.
//!
//! These tests deliberately keep the last complete offline copy while the
//! provider is unavailable, and ensure neither an empty response nor a damaged
//! cache file is presented as playable media.

use mde_jellyfin::{
    CacheError, CacheRequest, ClientInfo, HttpRequest, HttpResponse, HttpTransport, JellyfinClient,
    JellyfinError, OfflineCache, TransportError,
};

const KNOWN_GOOD: &[u8] = b"complete-media-fixture";

#[derive(Clone, Copy)]
enum ProviderReply {
    Http(u16),
    Transport,
    Empty,
}

struct OutageTransport(ProviderReply);

impl HttpTransport for OutageTransport {
    fn execute(&self, _request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        match self.0 {
            ProviderReply::Http(status) => Ok(HttpResponse {
                status,
                body: b"partial-provider-body".to_vec(),
            }),
            ProviderReply::Transport => Err(TransportError("provider connection reset".into())),
            ProviderReply::Empty => Ok(HttpResponse {
                status: 200,
                body: Vec::new(),
            }),
        }
    }
}

fn client(reply: ProviderReply) -> JellyfinClient<OutageTransport> {
    JellyfinClient::new(
        "https://jelly.mesh:8096",
        ClientInfo::new("mde-media", "workstation", "device-42", "fixture"),
        OutageTransport(reply),
    )
    .with_auth("TOKEN", "user-9f3a")
}

fn cache_request() -> CacheRequest {
    CacheRequest {
        item_id: "movie-1".into(),
        server_id: "jelly-home".into(),
        source_id: Some("source-1".into()),
        title: "Movie One".into(),
        container: "mkv".into(),
    }
}

#[test]
fn outage_keeps_last_complete_copy_and_refuses_invalid_replacements() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cache = OfflineCache::with_root(dir.path());
    let request = cache_request();
    cache.store(&request, KNOWN_GOOD, 100).expect("seed cache");

    for reply in [
        ProviderReply::Http(503),
        ProviderReply::Transport,
        ProviderReply::Empty,
    ] {
        let error = client(reply)
            .download("https://jelly.mesh:8096/Videos/movie-1/stream")
            .expect_err("outage must fail closed");
        match reply {
            ProviderReply::Http(503) => {
                assert!(matches!(error, JellyfinError::Http { status: 503 }))
            }
            ProviderReply::Transport => assert!(matches!(error, JellyfinError::Transport(_))),
            ProviderReply::Empty => assert!(matches!(error, JellyfinError::EmptyMedia)),
            ProviderReply::Http(_) => unreachable!("only 503 is used above"),
        }

        assert!(
            cache.contains("movie-1"),
            "outage must preserve cache entry"
        );
        let path = cache.local_path("movie-1").expect("known-good path");
        assert_eq!(std::fs::read(path).expect("known-good bytes"), KNOWN_GOOD);
    }

    // A later filesystem truncation is also not a playable fallback: the
    // manifest row remains evidence only until its file is complete again.
    let path = cache
        .local_path("movie-1")
        .expect("path before hostile write");
    std::fs::write(&path, b"short").expect("truncate fixture");
    assert!(!cache.contains("movie-1"));
    assert!(cache.local_path("movie-1").is_none());

    // The cache itself is a second guard if a future caller bypasses download().
    let error = cache
        .store(&request, &[], 200)
        .expect_err("empty replacement must not be admitted");
    assert!(matches!(error, CacheError::EmptyMedia));
    assert_eq!(cache.entries().len(), 1);
}

#[test]
fn outage_rejects_a_zero_byte_manifest_copy_after_cache_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cache = OfflineCache::with_root(dir.path());
    let request = cache_request();
    cache.store(&request, KNOWN_GOOD, 100).expect("seed cache");

    // Simulate a provider/cache interruption that leaves an empty media file
    // and a self-consistent, but hostile, zero-byte manifest row.  The reload
    // path must not promote that row to playable offline media.
    let path = cache.local_path("movie-1").expect("known-good path");
    std::fs::write(&path, []).expect("write empty replacement");
    let manifest_path = cache.manifest_path();
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("valid manifest");
    manifest["entries"][0]["byte_len"] = serde_json::Value::from(0_u64);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("manifest json"),
    )
    .expect("write hostile manifest");

    let reloaded = OfflineCache::load_from(dir.path()).expect("reload cache");
    assert!(!reloaded.contains("movie-1"));
    assert!(reloaded.local_path("movie-1").is_none());
}
