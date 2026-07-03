//! `media-smoke` — a runtime-reachable smoke for the MEDIA-1 player core.
//!
//! Default build (no `mpv` feature, airgap-safe): construct a [`Player`] over
//! [`FakeMpv`] and drive the full transport cycle, printing each state
//! transition + event. This proves the player core is reachable and works with
//! **no system libmpv** — it is the CI-runnable smoke.
//!
//! With `--features mpv` and a clip path argument: construct the real mpv engine
//! and actually play the clip, printing live position until it ends. This is the
//! honest-gated *real-clip smoke* (only builds/runs where system libmpv is
//! present).
//!
//! [`Player`]: mde_media_core::Player
//! [`FakeMpv`]: mde_media_core::FakeMpv

use mde_media_core::{
    opensubtitles, AbLoop, AspectRatio, AudioConfig, Crop, Deinterlace, EqBand, ExternalSub,
    FakeMpv, HwDecode, LoudnessNorm, PlaybackControls, Player, PlayerState, Playlist, PlaylistItem,
    RepeatMode, ReplayGainMode, Rotation, ScreenshotMode, SubtitleConfig, Track, TrackKind,
    TrackSelect, TrackSelection, VideoConfig, VideoFilter,
};

fn sample_tracks() -> Vec<Track> {
    vec![
        Track {
            id: 1,
            kind: TrackKind::Video,
            title: Some("Main".into()),
            lang: None,
            codec: Some("h264".into()),
            default: true,
            selected: true,
        },
        Track {
            id: 1,
            kind: TrackKind::Audio,
            title: None,
            lang: Some("eng".into()),
            codec: Some("aac".into()),
            default: true,
            selected: true,
        },
        Track {
            id: 2,
            kind: TrackKind::Audio,
            title: None,
            lang: Some("jpn".into()),
            codec: Some("aac".into()),
            default: false,
            selected: false,
        },
        Track {
            id: 1,
            kind: TrackKind::Subtitle,
            title: None,
            lang: Some("eng".into()),
            codec: Some("ass".into()),
            default: false,
            selected: false,
        },
    ]
}

/// Drive the fake-engine transport cycle, printing what the surface would render.
// A linear, top-to-bottom smoke that walks every MEDIA-1/3/4/5 surface in one
// pass; splitting it would only scatter the single narrative it prints.
#[allow(clippy::too_many_lines)]
fn run_fake_smoke() {
    let mut player = Player::new(
        FakeMpv::new()
            .with_duration(90.0)
            .with_tracks(sample_tracks()),
    );

    println!("== mde-media-core smoke (FakeMpv, no system libmpv) ==");
    println!("state: {:?}", player.state());

    player.load("test://big-buck-bunny.mkv").expect("load");
    player.pump();
    report(&mut player, "after load + pump");
    assert_eq!(player.state(), PlayerState::Playing);
    println!(
        "  duration={:?} tracks={}",
        player.duration(),
        player.tracks().len()
    );

    // MEDIA-3: configure the PipeWire audio path + a graphic EQ + EBU R128
    // loudness normalization + per-album ReplayGain, and show the folded mpv
    // af-graph / property set the engine actually receives.
    let audio = AudioConfig {
        eq: EqBand::iso_10_band([3.0, 2.0, 1.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 2.0]),
        loudness: LoudnessNorm::Ebu {
            target_lufs: -16.0,
            true_peak_db: -1.5,
            range_lu: 11.0,
        },
        replaygain: ReplayGainMode::Album,
        gapless: true,
        ..AudioConfig::new()
    };
    player.set_audio_config(audio).expect("apply audio config");
    println!("  audio af-graph: {}", player.audio_config().af_graph());
    println!("  audio props:    {:?}", player.audio_config().properties());
    println!("  engine applied: af={:?}", player.engine().applied_af());

    // MEDIA-4: configure VA-API hardware decode (with software fallback) + the
    // video adjustments — a forced 16:9 aspect, a letterbox crop, a 90° rotation,
    // deinterlacing, and a denoise filter — and show the folded mpv hwdec/video-*
    // property set + `vf` graph the engine actually receives.
    let video = VideoConfig {
        hwdec: HwDecode::VaApi,
        aspect: AspectRatio::SIXTEEN_NINE,
        crop: Some(Crop::new(1920, 800, 0, 140)),
        rotate: Rotation::Cw90,
        deinterlace: Deinterlace::On,
        filters: vec![VideoFilter::bare("hqdn3d".to_owned())],
        ..VideoConfig::new()
    };
    player.set_video_config(video).expect("apply video config");
    println!("  video vf-graph: {}", player.video_config().vf_graph());
    println!("  video props:    {:?}", player.video_config().properties());
    println!("  engine applied: vf={:?}", player.engine().applied_vf());

    // MEDIA-5: select the Japanese audio + English subtitle by language label,
    // load an external .srt, and style/position/delay the subtitles — showing the
    // folded aid/vid/sid + sub-add commands + sub-* properties the engine receives.
    player
        .set_track_selection(TrackSelection {
            audio: TrackSelect::Auto,
            video: TrackSelect::Auto,
            subtitle: TrackSelect::Auto,
        })
        .expect("apply track selection");
    let picked_audio = player
        .select_track_by_language(TrackKind::Audio, "jpn")
        .expect("select audio by language");
    let picked_sub = player
        .select_track_by_language(TrackKind::Subtitle, "eng")
        .expect("select subtitle by language");
    println!("  track pick: jpn audio={picked_audio} eng sub={picked_sub}");
    println!("  track props: {:?}", player.track_selection().properties());
    println!(
        "  engine applied: aid/vid/sid={:?}",
        player.engine().applied_track_properties()
    );

    let subtitles = SubtitleConfig {
        external: vec![ExternalSub {
            path: "/subs/big-buck-bunny.eng.srt".into(),
            load: mde_media_core::SubLoad::Select,
            title: Some("English (external)".into()),
            lang: Some("eng".into()),
        }],
        pos: 95,
        scale: 1.1,
        delay: 0.25,
        ..SubtitleConfig::new()
    };
    player
        .set_subtitle_config(subtitles)
        .expect("apply subtitle config");
    println!(
        "  sub commands: {:?}",
        player.engine().applied_sub_commands()
    );
    println!(
        "  sub props:    {:?}",
        player.subtitle_config().properties()
    );

    // MEDIA-5: the OpenSubtitles movie hash + the query URL it drives (the online
    // fetch itself is honest-gated to a host with egress + an API key). Hash a
    // synthetic 128 KiB clip in memory so the smoke needs no real file.
    let clip = vec![0u8; 128 * 1024];
    let mut cursor = std::io::Cursor::new(clip);
    let hash = opensubtitles::hash_reader(&mut cursor).expect("hash");
    println!(
        "  opensubtitles: hash={} url={}",
        opensubtitles::format_hash(hash),
        opensubtitles::search_url(hash)
    );

    // MEDIA-6: advanced playback controls — 1.25x speed, a small A/V-sync offset,
    // gapless prefetch, and an A-B loop — plus a paused frame-step + snapshot,
    // showing the folded speed/audio-delay/ab-loop property set + one-shot commands
    // the engine receives.
    let controls = PlaybackControls {
        speed: 1.25,
        audio_delay: -0.040,
        gapless: true,
        ab_loop: AbLoop::Range { a: 10.0, b: 40.0 },
    };
    player.set_controls(controls).expect("apply controls");
    println!("  controls props: {:?}", player.controls().properties());
    player.pause().expect("pause for frame-step");
    player.frame_step().expect("frame step");
    player
        .snapshot(ScreenshotMode::Subtitles)
        .expect("snapshot");
    println!(
        "  frame steps: {:?} screenshots: {:?}",
        player.engine().frame_steps(),
        player.engine().screenshots()
    );
    player.play().expect("resume after frame-step");

    player.engine_mut().advance(30.0);
    player.pump();
    report(&mut player, "after 30s");

    player.pause().expect("pause");
    report(&mut player, "pause");
    assert_eq!(player.state(), PlayerState::Paused);

    player.play().expect("play");
    report(&mut player, "play");

    player.seek(75.0).expect("seek");
    report(&mut player, "seek 75s");

    player.engine_mut().reach_eof();
    player.pump();
    report(&mut player, "end of file");
    assert_eq!(player.state(), PlayerState::Ended);

    player.play().expect("replay");
    report(&mut player, "replay");
    assert_eq!(player.state(), PlayerState::Playing);

    player.stop().expect("stop");
    report(&mut player, "stop");
    assert_eq!(player.state(), PlayerState::Stopped);

    run_playlist_smoke();

    println!("== smoke OK: full transport cycle drove the state machine ==");
}

/// MEDIA-6: drive the playlist / queue model — a shuffled, repeat-all 3-item queue
/// that auto-advances on end-of-file — proving the queue drives real playback.
fn run_playlist_smoke() {
    println!("-- MEDIA-6 playlist / queue (auto-advance on EOF) --");
    let mut queue = Player::new(FakeMpv::new().with_duration(3.0));
    let mut playlist = Playlist::from_items(vec![
        PlaylistItem::titled("test://track-1.flac", "Track 1"),
        PlaylistItem::titled("test://track-2.flac", "Track 2"),
        PlaylistItem::titled("test://track-3.flac", "Track 3"),
    ]);
    playlist.set_repeat(RepeatMode::All);
    playlist.shuffle(0x00C0_FFEE);
    println!(
        "  queue: {} items, repeat={:?}, shuffled={}",
        playlist.len(),
        playlist.repeat(),
        playlist.is_shuffled()
    );
    queue.set_playlist(playlist);

    // Start on the queue head, then let end-of-file walk the (shuffled) order.
    if let Some(head) = queue.playlist().current().map(|item| item.url.clone()) {
        queue.load(head).expect("load head");
        queue.pump();
        println!("  now playing: {:?}", queue.media());
        for _ in 0..4 {
            queue.engine_mut().reach_eof();
            queue.pump(); // EOF auto-advances the queue (a fresh load)
            queue.pump(); // FileLoaded → Playing
            println!(
                "  auto-advanced to: {:?} (index {:?})",
                queue.media(),
                queue.playlist().current_index()
            );
        }
        assert_eq!(queue.state(), PlayerState::Playing);
    }
    println!("== playlist smoke OK: the queue drove auto-advance ==");
}

fn report(player: &mut Player<FakeMpv>, label: &str) {
    println!(
        "[{label}] state={:?} pos={:.1}s events={:?}",
        player.state(),
        player.position(),
        player.drain_events()
    );
}

#[cfg(feature = "mpv")]
fn run_real_smoke(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use mde_media_core::mpv::MpvEngine;
    use std::time::{Duration, Instant};

    println!("== mde-media-core real-clip smoke (libmpv): {path} ==");
    let mut player = Player::new(MpvEngine::new()?);
    player.load(path)?;

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        player.pump();
        for ev in player.drain_events() {
            println!("  event: {ev:?}");
        }
        match player.state() {
            PlayerState::Stopped | PlayerState::Ended => break,
            _ => {
                println!(
                    "  state={:?} pos={:.1}s / {:?}",
                    player.state(),
                    player.position(),
                    player.duration()
                );
            }
        }
        if Instant::now() > deadline {
            println!("  (30s smoke window elapsed — stopping)");
            player.stop()?;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("== real-clip smoke OK ==");
    Ok(())
}

fn main() {
    #[cfg(feature = "mpv")]
    {
        if let Some(path) = std::env::args().nth(1) {
            if let Err(e) = run_real_smoke(&path) {
                eprintln!("real-clip smoke failed: {e}");
                std::process::exit(1);
            }
            return;
        }
    }
    run_fake_smoke();
}
