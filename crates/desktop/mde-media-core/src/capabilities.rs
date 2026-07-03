//! The container + codec support set the local player can **direct-play** (MEDIA-10).
//!
//! Playback negotiation (choosing direct-play / direct-stream vs a server-side
//! transcode) needs one honest fact from the player: *what can this mpv decode
//! locally?* This module is that fact — a pure [`MpvCapabilities`] set of the
//! containers + video/audio codecs an mpv build (via its bundled ffmpeg) plays
//! back in software on any seat, with no GPU or codec pack assumed.
//!
//! It is a **pure value type** (no mpv linkage): the baseline is the
//! software-decode set ffmpeg carries universally, so it is honest on the
//! airgapped farm and does not need the `mpv` feature. The Jellyfin surface
//! (`mde-jellyfin`'s playback negotiation, consumed by `mde-media-egui`) reads
//! these sets to build the client capability profile it negotiates against —
//! §6 glue, so the negotiation is unit-testable with no libmpv and no network.
//!
//! Hardware decode (VA-API, MEDIA-4) is a *runtime GPU* property and is
//! deliberately **not** modelled here: software decode of this set is universal,
//! so treating it as the floor never over-promises (an unlisted codec falls back
//! to asking the server to transcode, which always works).

use std::collections::BTreeSet;

/// Normalize a container / codec label for case-insensitive matching: trimmed
/// and lowercased (Jellyfin reports `"h264"`, mpv `"H264"`, etc.).
fn normalize(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

/// The containers + codecs the local mpv player can decode without a server-side
/// transcode — the capability floor playback negotiation (MEDIA-10) reads.
///
/// Three sets: the demuxable **containers**, the decodable **video codecs**, and
/// the decodable **audio codecs**. Membership is case-insensitive. Build the
/// realistic default with [`baseline`](Self::baseline), or an exact set for a
/// test / a constrained seat with [`new`](Self::new) + the `with_*` builders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MpvCapabilities {
    containers: BTreeSet<String>,
    video_codecs: BTreeSet<String>,
    audio_codecs: BTreeSet<String>,
}

impl MpvCapabilities {
    /// An empty capability set — supports nothing until populated (every probe
    /// yields "transcode"). The builder base + the honest "unknown seat" state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The realistic mpv/ffmpeg software-decode baseline.
    ///
    /// The containers + codecs a stock ffmpeg (which mpv links) demuxes and
    /// decodes on any host with no GPU or extra codec pack — so it is an honest
    /// floor on the airgapped farm. An item outside this set negotiates to a
    /// server transcode rather than a broken direct-play.
    #[must_use]
    pub fn baseline() -> Self {
        let containers = [
            "mkv", "webm", "mp4", "m4v", "mov", "avi", "ts", "m2ts", "mpegts", "flv", "3gp", "wmv",
            "asf", "mpg", "mpeg", "ogv", "ogg", "mp3", "flac", "wav", "m4a", "aac", "opus", "oga",
            "hls",
        ];
        let video_codecs = [
            "h264",
            "avc",
            "hevc",
            "h265",
            "vp8",
            "vp9",
            "av1",
            "mpeg2video",
            "mpeg4",
            "msmpeg4v3",
            "vc1",
            "wmv3",
            "theora",
            "prores",
            "dvvideo",
        ];
        let audio_codecs = [
            "aac",
            "ac3",
            "eac3",
            "mp3",
            "mp2",
            "flac",
            "alac",
            "opus",
            "vorbis",
            "dts",
            "truehd",
            "pcm_s16le",
            "pcm_s24le",
            "wmav2",
            "wmapro",
            "amr_nb",
        ];
        Self::new()
            .with_containers(containers)
            .with_video_codecs(video_codecs)
            .with_audio_codecs(audio_codecs)
    }

    /// Add decodable containers (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_containers<I, S>(mut self, containers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.containers
            .extend(containers.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Add decodable video codecs (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_video_codecs<I, S>(mut self, codecs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.video_codecs
            .extend(codecs.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Add decodable audio codecs (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_audio_codecs<I, S>(mut self, codecs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.audio_codecs
            .extend(codecs.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Whether the player can demux `container` (case-insensitive).
    #[must_use]
    pub fn supports_container(&self, container: &str) -> bool {
        self.containers.contains(&normalize(container))
    }

    /// Whether the player can decode the video codec `codec` (case-insensitive).
    #[must_use]
    pub fn supports_video_codec(&self, codec: &str) -> bool {
        self.video_codecs.contains(&normalize(codec))
    }

    /// Whether the player can decode the audio codec `codec` (case-insensitive).
    #[must_use]
    pub fn supports_audio_codec(&self, codec: &str) -> bool {
        self.audio_codecs.contains(&normalize(codec))
    }

    /// The decodable containers (the bridge to the Jellyfin capability profile).
    #[must_use]
    pub const fn containers(&self) -> &BTreeSet<String> {
        &self.containers
    }

    /// The decodable video codecs.
    #[must_use]
    pub const fn video_codecs(&self) -> &BTreeSet<String> {
        &self.video_codecs
    }

    /// The decodable audio codecs.
    #[must_use]
    pub const fn audio_codecs(&self) -> &BTreeSet<String> {
        &self.audio_codecs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_supports_nothing() {
        let caps = MpvCapabilities::new();
        assert!(!caps.supports_container("mkv"));
        assert!(!caps.supports_video_codec("h264"));
        assert!(caps.containers().is_empty());
    }

    #[test]
    fn baseline_covers_the_common_direct_play_set() {
        let caps = MpvCapabilities::baseline();
        // The everyday container + codec combination direct-plays.
        assert!(caps.supports_container("mkv"));
        assert!(caps.supports_video_codec("h264"));
        assert!(caps.supports_video_codec("hevc"));
        assert!(caps.supports_audio_codec("aac"));
        assert!(caps.supports_audio_codec("flac"));
    }

    #[test]
    fn membership_is_case_insensitive() {
        let caps = MpvCapabilities::baseline();
        assert!(caps.supports_container("MKV"));
        assert!(caps.supports_video_codec("H264"));
        assert!(caps.supports_audio_codec("AAC"));
    }

    #[test]
    fn an_exotic_codec_is_absent_so_it_negotiates_to_transcode() {
        let caps = MpvCapabilities::baseline();
        // A codec no stock ffmpeg carries → not claimed → the negotiation will
        // (honestly) ask the server to transcode rather than fake direct-play.
        assert!(!caps.supports_video_codec("mpeg1video_exotic"));
        assert!(!caps.supports_audio_codec("some_drm_codec"));
        assert!(!caps.supports_container("bespoke-container"));
    }

    #[test]
    fn builder_adds_exact_sets_normalized() {
        let caps = MpvCapabilities::new()
            .with_containers(["MP4", " mkv "])
            .with_video_codecs(["H264"])
            .with_audio_codecs(["AAC"]);
        assert!(caps.supports_container("mp4"));
        assert!(caps.supports_container("mkv"));
        assert!(caps.supports_video_codec("h264"));
        assert!(caps.supports_audio_codec("aac"));
        // Only what was added.
        assert!(!caps.supports_video_codec("hevc"));
    }
}
