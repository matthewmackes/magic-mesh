//! L1 "Fixture Decode" validation (`docs/gpu_encoder.md`, BUG-VIDEO-1).
//!
//! Proves a short fixture file decodes through the **real** mpv engine end to
//! end — not `FakeMpv` — closing the exact "shipped player is `FakeMpv`" gap
//! the WORKLIST records.
//!
//! The whole file is gated on the `mpv` feature (mirrors `src/mpv.rs` itself),
//! so it compiles — and runs — only where the caller opted into the real
//! engine, exactly the honest-gated shape every other real-clip path in this
//! crate already uses (`media_smoke`'s real-clip leg, the `opensubtitles` live
//! fetch). `cargo test` (default features) never touches this file; `cargo test
//! --features mpv` on a host with `mpv-libs-devel` does.
//!
//! The fixture (`tests/fixtures/tiny_clip.mkv`, ~14 KB) is a synthetic clip in
//! a Matroska container — 64x64, ~1.5 s, VP8 video and an audible Opus tone,
//! generated with `ffmpeg`'s `testsrc2`/`sine` lavfi sources (a real,
//! colourful, non-uniform test pattern, not a solid fill). VP8/Opus were
//! chosen for maximum decoder portability — unencumbered and present in
//! essentially every ffmpeg/libmpv build — rather than to exercise a specific
//! codec; AV1/H.264/H.265 coverage is the follow-on codec task the WORKLIST
//! already tracks separately.

#![cfg(feature = "mpv")]

use std::time::{Duration, Instant};

use mde_media_core::mpv::MpvEngine;
use mde_media_core::{MediaEngine, Player, PlayerState};

/// How long the test waits for the fixture to reach `Playing` (or finish) and
/// for a first nonblank frame, before failing with a clear diagnostic rather
/// than hanging. Generous for a slow/loaded farm build VM; the fixture itself
/// is only ~1.5 s.
const WAIT_BUDGET: Duration = Duration::from_secs(15);

/// The fixture's absolute path (`CARGO_MANIFEST_DIR`-relative, so it resolves
/// regardless of the farm's build-slot working directory).
fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/tiny_clip.mkv",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Poll `player.pump()` until `until` is true or [`WAIT_BUDGET`] elapses.
/// Returns whether `until` was satisfied (never panics itself — the caller
/// asserts with a real diagnostic on `false`).
fn pump_until(
    player: &mut Player<MpvEngine>,
    mut until: impl FnMut(&Player<MpvEngine>) -> bool,
) -> bool {
    let deadline = Instant::now() + WAIT_BUDGET;
    loop {
        player.pump();
        if until(player) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Explicit, grep-able proof this test drives the real engine — never
/// `FakeMpv` — confirming the doc's L1 gate ("no `FakeMpv` type is used in the
/// release feature configuration") at the type level, not just by convention.
fn assert_is_not_fake_mpv<E: 'static>() {
    assert_ne!(
        std::any::TypeId::of::<E>(),
        std::any::TypeId::of::<mde_media_core::FakeMpv>(),
        "the L1 fixture-decode test must drive the real MpvEngine, never FakeMpv \
         (that would silently defeat the whole point of this gate)"
    );
}

#[test]
fn fixture_decodes_through_real_mpv_with_a_nonblank_frame() {
    assert_is_not_fake_mpv::<MpvEngine>();

    let engine = MpvEngine::new().expect(
        "MpvEngine::new() failed — system libmpv should be present when this file is \
         compiled at all (the `mpv` feature links it); check `mpv-libs-devel` on this host",
    );
    let mut player = Player::new(engine);

    player
        .load(fixture_path())
        .expect("load() should accept the checked-in fixture path");

    // 1. Reach a playable state. `MpvEngine::new` sets `audio-fallback-to-null`,
    //    so this must not depend on a real audio device being present — video
    //    decode alone drives Loading -> Playing once mpv's FileLoaded fires. A
    //    very short/slow clip may already have reached Ended by the time we
    //    first observe it; both are an honest "it played" signal.
    let reached_playable = pump_until(&mut player, |p| {
        matches!(p.state(), PlayerState::Playing | PlayerState::Ended)
    });
    assert!(
        reached_playable,
        "fixture never reached Playing/Ended within {WAIT_BUDGET:?} (state = {:?}) — \
         real mpv decode did not complete; is mpv-libs-devel actually usable here?",
        player.state()
    );

    // 2. Audio state reaches playing, or a typed no-audio-device gate: either a
    //    real ao resolved, or the `audio-fallback-to-null` gate did — mpv's
    //    `current-ao` property is empty only if audio never initialized at
    //    all, which is the one outcome this test treats as a real failure.
    let mut current_ao = String::new();
    let audio_resolved = pump_until(&mut player, |p| {
        current_ao = p
            .engine()
            .raw()
            .get_property("current-ao")
            .unwrap_or_default();
        !current_ao.is_empty()
    });
    assert!(
        audio_resolved,
        "mpv's audio output never resolved to anything, not even the honest \
         `null` fallback (current-ao was empty) — audio init did not run at all"
    );

    // 3. Capture at least one nonblank decoded frame. `latest_frame` throttles
    //    to one real capture per ~150 ms, so poll a few times rather than
    //    once.
    let mut frame = None;
    let deadline = Instant::now() + WAIT_BUDGET;
    while frame.is_none() && Instant::now() < deadline {
        frame = player.engine_mut().latest_frame();
        if frame.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let frame = frame.expect(
        "latest_frame() never produced a frame within the wait budget — the \
         screenshot-to-file capture path (vo=null + MediaEngine::latest_frame) \
         is not producing real decoded pixels",
    );

    assert!(
        frame.width > 0 && frame.height > 0,
        "a real frame has real dimensions"
    );
    assert!(
        !frame.is_blank(),
        "the captured frame is a uniform/degenerate fill, not the real testsrc2 \
         pattern — decode is not actually reaching the frame sink"
    );
    let checksum = frame.checksum();
    assert_ne!(
        checksum, 0,
        "a nonblank frame has a nonzero content checksum"
    );

    // A second capture (past the engine's internal throttle window) must also
    // be real, nonblank content — playback may have advanced or already ended
    // by now, so we only assert internal consistency of whichever frame (if
    // any) comes back, not that it matches the first pixel-for-pixel.
    std::thread::sleep(Duration::from_millis(200));
    if let Some(second) = player.engine_mut().latest_frame() {
        assert!(
            !second.is_blank(),
            "a later capture must also be real content"
        );
    }
}
