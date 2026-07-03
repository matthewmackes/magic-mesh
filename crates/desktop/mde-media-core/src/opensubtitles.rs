//! MEDIA-5: `OpenSubtitles` online fetch by movie hash (egress, fail-soft).
//!
//! Design lock (`docs/design/mesh-media-player.md`, Q7 + the Risks note): the
//! player can fetch subtitles from **`OpenSubtitles`** by the file's *movie hash* —
//! an egress feature that must "gate cleanly, fail soft offline, and honor any
//! fleet egress policy".
//!
//! The load-bearing, airgap-safe core is **pure** and fully fixture-tested here:
//!
//! - [`hash_reader`] / [`hash_file`] compute the `OpenSubtitles` movie hash — the
//!   `filesize + Σ(first 64 KiB words) + Σ(last 64 KiB words)` 64-bit checksum —
//!   over any `Read + Seek`, and [`format_hash`] renders the 16-hex-digit form the
//!   API expects. This is a pure function, tested against hand-derived vectors.
//! - [`search_url`] + [`request_headers`] build the exact REST request, and
//!   [`parse_search_response`] parses the API's JSON into typed
//!   [`SubtitleSearchResult`]s. Both are pure + fixture-tested.
//!
//! The only part that touches the network is [`OpenSubtitlesClient`] (behind the
//! `opensubtitles` feature): it performs the one blocking HTTPS `GET` and hands the
//! body to [`parse_search_response`]. Like MEDIA-1's real-clip `mpv` path it is
//! **honest-gated** — the airgapped farm carries no egress and no API key, so the
//! live fetch is exercised only where both exist; every byte of request-building
//! and response-parsing around it is tested with no network.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The size of the head + tail regions summed by the `OpenSubtitles` hash (64 KiB).
const CHUNK: u64 = 64 * 1024;

/// Sum every complete little-endian `u64` word in `buf` into `acc` (wrapping).
///
/// The `OpenSubtitles` algorithm treats each 64 KiB region as an array of
/// little-endian `u64`s and wraps on overflow; any trailing bytes shorter than a
/// full word are ignored, matching the reference implementations.
fn add_words(acc: u64, buf: &[u8]) -> u64 {
    buf.chunks_exact(8).fold(acc, |sum, word| {
        // chunks_exact(8) yields 8-byte slices, so the array conversion is total.
        let value = u64::from_le_bytes(word.try_into().expect("chunks_exact(8) is 8 bytes"));
        sum.wrapping_add(value)
    })
}

/// Compute the `OpenSubtitles` movie hash of a `Read + Seek` source.
///
/// The hash is `filesize + Σ(words of the first 64 KiB) + Σ(words of the last
/// 64 KiB)`, all as wrapping `u64` arithmetic. For a file shorter than 64 KiB the
/// two regions overlap (the whole file is summed for each), matching the reference
/// implementations; a valid movie hash conventionally needs ≥ 64 KiB of media.
///
/// # Errors
/// Returns any [`io::Error`] from seeking or reading the source.
pub fn hash_reader<R: Read + Seek>(reader: &mut R) -> io::Result<u64> {
    let size = reader.seek(SeekFrom::End(0))?;
    let mut hash = size;

    // First region: up to 64 KiB from the start.
    let head_len = size.min(CHUNK);
    let mut head = vec![0u8; usize::try_from(head_len).unwrap_or(usize::MAX)];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut head)?;
    hash = add_words(hash, &head);

    // Last region: up to 64 KiB ending at EOF (start clamped to 0 for small files).
    let tail_start = size.saturating_sub(CHUNK);
    let tail_len = size - tail_start;
    let mut tail = vec![0u8; usize::try_from(tail_len).unwrap_or(usize::MAX)];
    reader.seek(SeekFrom::Start(tail_start))?;
    reader.read_exact(&mut tail)?;
    hash = add_words(hash, &tail);

    Ok(hash)
}

/// Compute the `OpenSubtitles` movie hash of a file on disk.
///
/// # Errors
/// Returns any [`io::Error`] from opening, seeking, or reading the file.
pub fn hash_file<P: AsRef<Path>>(path: P) -> io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    hash_reader(&mut file)
}

/// Render a movie hash as the 16-lowercase-hex-digit string the API expects
/// (e.g. `0x20003` → `"0000000000020003"`).
#[must_use]
pub fn format_hash(hash: u64) -> String {
    format!("{hash:016x}")
}

/// The `OpenSubtitles` REST API subtitle-search endpoint.
pub const SEARCH_ENDPOINT: &str = "https://api.opensubtitles.com/api/v1/subtitles";

/// Build the search URL that queries `OpenSubtitles` by movie hash.
///
/// Pure + testable: the live [`OpenSubtitlesClient`] simply `GET`s this URL. The
/// hash is rendered via [`format_hash`].
#[must_use]
pub fn search_url(hash: u64) -> String {
    format!("{SEARCH_ENDPOINT}?moviehash={}", format_hash(hash))
}

/// The HTTP headers an `OpenSubtitles` REST request must carry: the caller's
/// `Api-Key`, a `User-Agent`, and the JSON `Accept`/`Content-Type`.
///
/// Returned as ordered `(name, value)` pairs so both the live client and the tests
/// build the identical header set. The API key is an operator secret supplied at
/// call time — never embedded.
#[must_use]
pub fn request_headers(api_key: &str, user_agent: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Api-Key", api_key.to_owned()),
        ("User-Agent", user_agent.to_owned()),
        ("Accept", "application/json".to_owned()),
    ]
}

/// One subtitle result parsed from an `OpenSubtitles` search response.
///
/// The typed projection of the API's nested JSON that the surface needs to list a
/// candidate + fetch it: the language label, the release name, the downloadable
/// file id + name, the format, and whether it matched by movie hash (the exact
/// match this feature is about).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubtitleSearchResult {
    /// The subtitle language tag (`"en"`, `"ja"`, …), if reported.
    pub language: Option<String>,
    /// The release / movie name the subtitle is for, if reported.
    pub release: Option<String>,
    /// The downloadable file id (handed to the download endpoint), if present.
    pub file_id: Option<i64>,
    /// The subtitle file name, if present.
    pub file_name: Option<String>,
    /// The subtitle format (`"srt"`, `"ass"`, …), if reported.
    pub format: Option<String>,
    /// The reported download count (a rough quality signal), if present.
    pub download_count: Option<i64>,
    /// Whether this result matched the query's movie hash exactly.
    pub moviehash_match: bool,
}

// ── the API's on-the-wire JSON shape (REST v1), parsed then projected ──────────

#[derive(Deserialize)]
struct RawResponse {
    #[serde(default)]
    data: Vec<RawDatum>,
}

#[derive(Deserialize)]
struct RawDatum {
    #[serde(default)]
    attributes: RawAttributes,
}

#[derive(Deserialize, Default)]
struct RawAttributes {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    download_count: Option<i64>,
    #[serde(default)]
    moviehash_match: bool,
    #[serde(default)]
    files: Vec<RawFile>,
}

#[derive(Deserialize, Default)]
struct RawFile {
    #[serde(default)]
    file_id: Option<i64>,
    #[serde(default)]
    file_name: Option<String>,
}

/// Parse an `OpenSubtitles` REST search-response body into typed results.
///
/// Tolerant of the API's optional fields (any missing field becomes [`None`] /
/// `false`) and flattens the first downloadable `files[]` entry of each datum into
/// the result. Pure — the live client feeds it the fetched body.
///
/// # Errors
/// Returns the [`serde_json::Error`] if the body is not the expected JSON shape.
pub fn parse_search_response(body: &str) -> Result<Vec<SubtitleSearchResult>, serde_json::Error> {
    let raw: RawResponse = serde_json::from_str(body)?;
    Ok(raw
        .data
        .into_iter()
        .map(|datum| {
            let attrs = datum.attributes;
            let first_file = attrs.files.into_iter().next().unwrap_or_default();
            SubtitleSearchResult {
                language: attrs.language,
                release: attrs.release,
                file_id: first_file.file_id,
                file_name: first_file.file_name,
                format: attrs.format,
                download_count: attrs.download_count,
                moviehash_match: attrs.moviehash_match,
            }
        })
        .collect())
}

/// A soft failure from the live `OpenSubtitles` fetch.
///
/// Every variant is recoverable: the caller treats any error as "no online
/// subtitles right now" and carries on with embedded/external subs — the fail-soft
/// contract of an egress feature.
#[derive(Debug, thiserror::Error)]
pub enum OpenSubtitlesError {
    /// The request could not be sent / no response (offline, DNS, TLS, timeout).
    #[error("opensubtitles request failed: {0}")]
    Request(String),
    /// The server answered with a non-success HTTP status.
    #[error("opensubtitles returned HTTP {0}")]
    Status(u16),
    /// The response body was not the expected JSON shape.
    #[error("opensubtitles response parse error: {0}")]
    Parse(String),
}

/// The live `OpenSubtitles` client — the one egress point (feature `opensubtitles`).
///
/// Honest-gated exactly like MEDIA-1's real-clip `mpv` path: it performs a single
/// blocking HTTPS `GET` to [`search_url`] with [`request_headers`] and parses the
/// body via [`parse_search_response`]. The airgapped farm carries neither egress
/// nor an API key, so [`search_by_hash`](Self::search_by_hash) is exercised only
/// where both exist; the request-building + parsing it composes are fully tested
/// with no network. Fail-soft: any transport/status/parse problem becomes an
/// [`OpenSubtitlesError`] the caller can ignore.
#[cfg(feature = "opensubtitles")]
#[derive(Debug, Clone)]
pub struct OpenSubtitlesClient {
    api_key: String,
    user_agent: String,
    http: reqwest::blocking::Client,
}

#[cfg(feature = "opensubtitles")]
impl OpenSubtitlesClient {
    /// A client authenticated with `api_key` (an operator secret) identifying
    /// itself with `user_agent` (`OpenSubtitles` requires a real one).
    ///
    /// # Errors
    /// Returns [`OpenSubtitlesError::Request`] if the HTTP client cannot be built.
    pub fn new(
        api_key: impl Into<String>,
        user_agent: impl Into<String>,
    ) -> Result<Self, OpenSubtitlesError> {
        let http = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| OpenSubtitlesError::Request(e.to_string()))?;
        Ok(Self {
            api_key: api_key.into(),
            user_agent: user_agent.into(),
            http,
        })
    }

    /// Search `OpenSubtitles` for subtitles matching `hash` (a movie hash from
    /// [`hash_file`]). Returns the parsed candidates, hash-matches first.
    ///
    /// # Errors
    /// Returns a soft [`OpenSubtitlesError`] on any transport, HTTP-status, or
    /// parse failure — the caller treats it as "no online subtitles".
    pub fn search_by_hash(
        &self,
        hash: u64,
    ) -> Result<Vec<SubtitleSearchResult>, OpenSubtitlesError> {
        let mut request = self.http.get(search_url(hash));
        for (name, value) in request_headers(&self.api_key, &self.user_agent) {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .map_err(|e| OpenSubtitlesError::Request(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(OpenSubtitlesError::Status(status.as_u16()));
        }
        let body = response
            .text()
            .map_err(|e| OpenSubtitlesError::Request(e.to_string()))?;
        let mut results =
            parse_search_response(&body).map_err(|e| OpenSubtitlesError::Parse(e.to_string()))?;
        // Surface exact hash matches first — the whole point of a hash query.
        results.sort_by(|a, b| b.moviehash_match.cmp(&a.moviehash_match));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The 64 KiB region size as a `usize`, for building fixture buffers.
    const CHUNK_BYTES: usize = 64 * 1024;

    // ── the movie hash (hand-derived vectors) ───────────────────────────────

    #[test]
    fn hash_of_all_zero_128kib_is_just_the_filesize() {
        // A 131_072-byte (2×64 KiB) all-zero file: every head/tail word is 0, so
        // the checksum is exactly the filesize. 131_072 = 0x20000.
        let data = vec![0u8; 2 * CHUNK_BYTES];
        let mut cur = Cursor::new(data);
        let h = hash_reader(&mut cur).expect("hash");
        assert_eq!(h, 0x0002_0000);
        assert_eq!(format_hash(h), "0000000000020000");
    }

    #[test]
    fn hash_sums_head_and_tail_words_plus_filesize() {
        // 131_072 bytes, all zero except byte[0]=1 (head word 0 = 1 LE) and
        // byte[65536]=2 (first word of the last 64 KiB region = 2 LE). The
        // checksum is filesize + 1 + 2 = 131_075 = 0x20003 — independently
        // hand-derived, exercising filesize + LE head word + seek'd tail word.
        let mut data = vec![0u8; 2 * CHUNK_BYTES];
        data[0] = 1;
        data[CHUNK_BYTES] = 2;
        let mut cur = Cursor::new(data);
        let h = hash_reader(&mut cur).expect("hash");
        assert_eq!(h, 0x0002_0003);
        assert_eq!(format_hash(h), "0000000000020003");
    }

    #[test]
    fn hash_wraps_on_overflow() {
        // A single head word of u64::MAX plus a 131_072-byte filesize wraps:
        // MAX + 131_072 (mod 2^64) = 131_071 = 0x1FFFF. (The same word appears in
        // the head only; the tail region starts at byte 65536 and is all zero.)
        let mut data = vec![0u8; 2 * CHUNK_BYTES];
        data[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        let mut cur = Cursor::new(data);
        let h = hash_reader(&mut cur).expect("hash");
        assert_eq!(h, 0x0001_ffff);
    }

    #[test]
    fn hash_file_matches_hash_reader_over_the_same_bytes() {
        // hash_file opens a real file and delegates to hash_reader — write the
        // head/tail vector to a temp file and confirm the on-disk hash matches.
        let mut data = vec![0u8; 2 * CHUNK_BYTES];
        data[0] = 1;
        data[CHUNK_BYTES] = 2;

        let path = std::env::temp_dir().join(format!(
            "mde-media-core-oshash-{}-{}.bin",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, &data).expect("write fixture file");
        let from_file = hash_file(&path).expect("hash file");
        std::fs::remove_file(&path).ok();

        let from_reader = hash_reader(&mut Cursor::new(data)).expect("hash reader");
        assert_eq!(from_file, from_reader);
        assert_eq!(from_file, 0x0002_0003);
    }

    #[test]
    fn format_hash_is_16_lowercase_hex_digits() {
        assert_eq!(format_hash(0), "0000000000000000");
        assert_eq!(format_hash(0x8e24_5d96_79d3_1e12), "8e245d9679d31e12");
        assert_eq!(format_hash(u64::MAX), "ffffffffffffffff");
    }

    // ── request building ────────────────────────────────────────────────────

    #[test]
    fn search_url_embeds_the_formatted_hash() {
        assert_eq!(
            search_url(0x0002_0003),
            "https://api.opensubtitles.com/api/v1/subtitles?moviehash=0000000000020003"
        );
    }

    #[test]
    fn request_headers_carry_api_key_and_user_agent() {
        let headers = request_headers("SECRET-KEY", "mde-media/1.0");
        assert!(headers.contains(&("Api-Key", "SECRET-KEY".to_owned())));
        assert!(headers.contains(&("User-Agent", "mde-media/1.0".to_owned())));
        assert!(headers.contains(&("Accept", "application/json".to_owned())));
    }

    // ── response parsing (fixture) ──────────────────────────────────────────

    #[test]
    fn parses_a_moviehash_search_response() {
        // A trimmed but shape-faithful `OpenSubtitles` REST v1 response with two
        // candidates: an exact hash match (English .srt) and a non-hash Spanish one.
        let body = r#"{
          "total_count": 2,
          "data": [
            {
              "id": "7061050",
              "type": "subtitle",
              "attributes": {
                "language": "en",
                "release": "Big Buck Bunny 2008 1080p",
                "format": "srt",
                "download_count": 512,
                "moviehash_match": true,
                "files": [
                  { "file_id": 7061050, "file_name": "Big.Buck.Bunny.en.srt" }
                ]
              }
            },
            {
              "id": "7061099",
              "type": "subtitle",
              "attributes": {
                "language": "es",
                "release": "Big Buck Bunny",
                "format": "ass",
                "download_count": 40,
                "moviehash_match": false,
                "files": [
                  { "file_id": 7061099, "file_name": "BBB.es.ass" }
                ]
              }
            }
          ]
        }"#;
        let results = parse_search_response(body).expect("parse");
        assert_eq!(results.len(), 2);

        let en = &results[0];
        assert_eq!(en.language.as_deref(), Some("en"));
        assert_eq!(en.release.as_deref(), Some("Big Buck Bunny 2008 1080p"));
        assert_eq!(en.file_id, Some(7_061_050));
        assert_eq!(en.file_name.as_deref(), Some("Big.Buck.Bunny.en.srt"));
        assert_eq!(en.format.as_deref(), Some("srt"));
        assert_eq!(en.download_count, Some(512));
        assert!(en.moviehash_match, "first result is the hash match");

        let es = &results[1];
        assert_eq!(es.language.as_deref(), Some("es"));
        assert!(!es.moviehash_match);
        assert_eq!(es.format.as_deref(), Some("ass"));
    }

    #[test]
    fn parses_empty_and_fieldless_responses_soft() {
        // No results at all.
        assert_eq!(
            parse_search_response(r#"{"data":[]}"#).expect("empty"),
            vec![]
        );
        // A datum with sparse attributes + no files → all-None, no panic.
        let sparse = r#"{"data":[{"attributes":{}}]}"#;
        let results = parse_search_response(sparse).expect("sparse");
        assert_eq!(results, vec![SubtitleSearchResult::default()]);
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_search_response("not json").is_err());
    }
}
