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

use mde_media_core::{FakeMpv, Player, PlayerState, Track, TrackKind};

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
    ]
}

/// Drive the fake-engine transport cycle, printing what the surface would render.
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

    println!("== smoke OK: full transport cycle drove the state machine ==");
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
