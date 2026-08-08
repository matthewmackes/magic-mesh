//! The background worker (E12-5): a dedicated thread that owns the Tokio runtime,
//! the Airsonic [`Client`], and — lazily, on first play — the native playback
//! [`Engine`], so the egui UI thread never blocks on the network or the audio
//! device. The UI sends [`Command`]s in; the worker sends [`Update`]s back and
//! wakes the UI with [`Context::request_repaint`].
//!
//! The engine is constructed *inside* this thread and never crosses a thread
//! boundary (its `cpal::Stream` is not `Send`); the airsonic `Client` is `Send`
//! and is moved in once. A current-thread runtime drives the async library calls
//! via `block_on`; playback control (`play`/`pause`/`stop`) is synchronous and the
//! engine spawns its own decode thread, so no Tokio runtime is ever nested.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use mde_egui::egui::Context;
use mde_musicd::airsonic::{Client, Song};
use mde_musicd::engine::Engine;

use crate::model::{
    select_default_server, track_for_engine, Command, FailoverRequest, SeatServer, Update,
};

/// Albums fetched per library listing. Subsonic's `getAlbumList2` caps `size` at
/// 500; one page covers the first-slice listing.
const LIBRARY_PAGE: u32 = 500;

/// The `getAlbumList2` ordering used for the library listing.
const LIBRARY_ORDER: &str = "alphabeticalByName";

/// How often the worker polls the engine's live playhead while a track is loaded,
/// pushing an [`Update::Progress`] and detecting a track that finished on its own.
/// Fast enough for a smooth seconds readout, slow enough to stay off the UI.
const PROGRESS_TICK: Duration = Duration::from_millis(500);

/// Bound UI intents so a stalled daemon/audio worker cannot turn repeated
/// clicks into an unbounded heap allocation. The UI remains responsive while
/// the worker drains network and engine work in order.
pub(crate) const COMMAND_QUEUE_CAPACITY: usize = 64;

/// Bound worker updates so a stalled egui frame cannot accumulate an unbounded
/// result backlog. The worker applies backpressure at this boundary.
pub(crate) const UPDATE_QUEUE_CAPACITY: usize = 256;

/// Spawn the worker thread around `client`, returning the [`Command`] sender the
/// UI drives it with. `ctx` is repainted after every [`Update`]; `updates`
/// carries results back. If the thread cannot be spawned, an [`Update::Error`] is
/// sent so the UI surfaces it rather than silently doing nothing.
pub fn spawn(
    connections: Vec<(SeatServer, Client)>,
    ctx: Context,
    updates: SyncSender<Update>,
) -> SyncSender<Command> {
    let (tx, rx) = mpsc::sync_channel::<Command>(COMMAND_QUEUE_CAPACITY);
    let err_tx = updates.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("mde-music-egui-worker".to_string())
        .spawn(move || run(connections, &ctx, &updates, &rx))
    {
        let _ = err_tx.send(Update::Error(format!("could not start music worker: {e}")));
    }
    tx
}

/// The worker loop: build the runtime, then service commands until the UI hangs
/// up (its command sender drops, ending `recv`).
struct Connection {
    server: SeatServer,
    client: Client,
}

fn run(
    connections: Vec<(SeatServer, Client)>,
    ctx: &Context,
    updates: &SyncSender<Update>,
    rx: &Receiver<Command>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = updates.send(Update::Error(format!("music worker runtime: {e}")));
            return;
        }
    };
    let mut connections: Vec<Connection> = connections
        .into_iter()
        .map(|(server, client)| Connection { server, client })
        .collect();
    if connections.is_empty() {
        let _ = updates.send(Update::Error(
            "no music seat servers configured".to_string(),
        ));
        return;
    }
    for connection in &mut connections {
        let started = Instant::now();
        if rt.block_on(connection.client.ping()).is_ok() {
            connection.server.latency_ms =
                Some(u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX));
        }
    }
    let mut active = select_default_server(
        &connections
            .iter()
            .map(|c| c.server.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap_or(0);
    let _ = updates.send(Update::ServerSelected(connections[active].server.clone()));
    let mut pending_failover: Option<usize> = None;
    // Opened on first play; a headless host with no sound card surfaces the
    // failure once, on the play attempt, instead of failing the whole surface.
    let mut engine: Option<Engine> = None;
    // Whether a track is loaded in the engine (playing OR paused). While it is,
    // we wait on a timeout so the live playhead and a natural end reach the UI
    // without a user command; while it isn't, we block until the next command.
    let mut track_loaded = false;

    loop {
        let cmd = if track_loaded {
            match rx.recv_timeout(PROGRESS_TICK) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            }
        };

        if let Some(cmd) = cmd {
            match cmd {
                Command::LoadLibrary => {
                    let result = rt
                        .block_on(
                            connections[active]
                                .client
                                .get_album_list2(LIBRARY_ORDER, LIBRARY_PAGE),
                        )
                        .map_err(|e| e.to_string());
                    if result.is_err() {
                        propose_failover(
                            &connections,
                            active,
                            updates,
                            &mut pending_failover,
                            "library server unavailable",
                        );
                    }
                    let _ = updates.send(Update::Library(result));
                }
                Command::LoadStarred => {
                    let result = rt
                        .block_on(connections[active].client.get_starred2())
                        .map_err(|e| e.to_string());
                    let _ = updates.send(Update::Starred(result));
                }
                Command::Search { generation, query } => {
                    let result = rt
                        .block_on(connections[active].client.search3(&query))
                        .map_err(|e| e.to_string());
                    let _ = updates.send(Update::Search {
                        generation,
                        query,
                        result,
                    });
                }
                Command::LoadAlbum(id) => {
                    let result = rt
                        .block_on(connections[active].client.get_album(&id))
                        .map(|detail| detail.songs)
                        .map_err(|e| e.to_string());
                    if result.is_err() {
                        propose_failover(
                            &connections,
                            active,
                            updates,
                            &mut pending_failover,
                            "album server unavailable",
                        );
                    }
                    let _ = updates.send(Update::Tracks {
                        album_id: id,
                        result,
                    });
                }
                Command::Play(song) => {
                    track_loaded = play(&connections[active].client, &mut engine, updates, song)
                }
                Command::Pause => {
                    let owns_active_track = engine.is_some() && track_loaded;
                    if owns_active_track {
                        if let Some(eng) = engine.as_ref() {
                            eng.pause();
                        }
                    }
                    if let Some(update) = transport_update(false, engine.is_some(), track_loaded) {
                        let _ = updates.send(update);
                    }
                }
                Command::Resume => {
                    let owns_active_track = engine.is_some() && track_loaded;
                    if owns_active_track {
                        if let Some(eng) = engine.as_ref() {
                            eng.resume();
                        }
                    }
                    if let Some(update) = transport_update(true, engine.is_some(), track_loaded) {
                        let _ = updates.send(update);
                    }
                }
                Command::Stop => {
                    if let Some(eng) = engine.as_ref() {
                        eng.stop();
                    }
                    track_loaded = false;
                    let _ = updates.send(Update::Stopped);
                }
                Command::SelectServer(seat) => {
                    if let Some(index) = connections.iter().position(|c| c.server.seat == seat) {
                        active = index;
                        pending_failover = None;
                        let _ = updates
                            .send(Update::ServerSelected(connections[active].server.clone()));
                    }
                }
                Command::ApproveFailover => {
                    if let Some(index) = pending_failover.take() {
                        active = index;
                        let _ = updates
                            .send(Update::ServerSelected(connections[active].server.clone()));
                    }
                }
                Command::RejectFailover => pending_failover = None,
                Command::Seek(target_ms) => {
                    if let Some(eng) = engine.as_ref() {
                        if !eng.seek(target_ms) {
                            let _ = updates
                                .send(Update::Error("This stream cannot be scrubbed".to_string()));
                        } else {
                            let _ = updates.send(Update::Progress(target_ms));
                        }
                    }
                }
                Command::SetVolume(volume) => {
                    if let Some(eng) = engine.as_ref() {
                        eng.set_volume(volume);
                    }
                }
            }
            // Wake the UI to drain the update we just sent.
            ctx.request_repaint();
        }

        // Poll the live engine while a track is loaded and actually playing: report
        // the playhead, or — once decode has finished and the ring has drained —
        // report the natural end so the transport clears instead of freezing on
        // the last track. A paused engine reports neither (it is not playing).
        if track_loaded {
            if let Some(eng) = engine.as_ref() {
                if eng.is_playing() {
                    if eng.is_active() {
                        let _ = updates.send(Update::Progress(eng.position_ms()));
                    } else {
                        track_loaded = false;
                        let _ = updates.send(Update::Ended);
                    }
                    ctx.request_repaint();
                }
            }
        }
    }
}

/// Publish transport state only while this compatibility worker actually owns
/// an active track. An `Engine` can remain allocated after Stop or natural end,
/// and an idle worker can receive a queued Pause/Resume before first Play; in
/// both cases reporting `Playing` would invent playback authority.
fn transport_update(playing: bool, engine_present: bool, track_loaded: bool) -> Option<Update> {
    (engine_present && track_loaded).then_some(Update::Playing(playing))
}

fn propose_failover(
    connections: &[Connection],
    active: usize,
    updates: &SyncSender<Update>,
    pending: &mut Option<usize>,
    reason: &str,
) {
    if pending.is_some() || connections.len() < 2 {
        return;
    }
    let candidates: Vec<SeatServer> = connections
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != active)
        .map(|(_, connection)| connection.server.clone())
        .collect();
    let Some(candidate) = select_default_server(&candidates) else {
        return;
    };
    let target = connections
        .iter()
        .position(|connection| connection.server == candidates[candidate])
        .unwrap_or(active);
    *pending = Some(target);
    let _ = updates.send(Update::FailoverPending(FailoverRequest {
        from: connections[active].server.seat.clone(),
        to: connections[target].server.seat.clone(),
        reason: reason.to_string(),
    }));
}

/// Lazily open the audio engine (first play only). Returns a borrow of the live
/// engine, or `None` after surfacing an [`Update::Error`] when no output device
/// is available.
fn ensure_engine<'a>(
    engine: &'a mut Option<Engine>,
    updates: &SyncSender<Update>,
) -> Option<&'a Engine> {
    if engine.is_none() {
        match Engine::new() {
            Ok(e) => *engine = Some(e),
            Err(e) => {
                let _ = updates.send(Update::Error(format!("audio output unavailable: {e}")));
                return None;
            }
        }
    }
    engine.as_ref()
}

/// Resolve the track's authenticated stream URL + codec and start it on the
/// engine, replacing any current playback. Confirms with [`Update::Started`] and
/// returns `true` when a track is now loaded, so the caller begins polling the
/// playhead; returns `false` (having surfaced an [`Update::Error`]) when no audio
/// device is available.
fn play(
    client: &Client,
    engine: &mut Option<Engine>,
    updates: &SyncSender<Update>,
    song: Song,
) -> bool {
    if let Some(eng) = ensure_engine(engine, updates) {
        let (url, codec) = track_for_engine(client, &song);
        eng.play(vec![(url, codec)]);
        let _ = updates.send(Update::Started(song));
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TrySendError;

    #[test]
    fn music_worker_command_queue_is_bounded() {
        let (tx, rx) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        for _ in 0..COMMAND_QUEUE_CAPACITY {
            tx.try_send(Command::LoadLibrary)
                .expect("capacity should admit the configured command page");
        }
        assert!(matches!(
            tx.try_send(Command::LoadLibrary),
            Err(TrySendError::Full(Command::LoadLibrary))
        ));
        drop(rx);
    }

    #[test]
    fn music_worker_update_queue_is_bounded() {
        let (tx, rx) = mpsc::sync_channel(UPDATE_QUEUE_CAPACITY);
        for _ in 0..UPDATE_QUEUE_CAPACITY {
            tx.try_send(Update::Progress(0))
                .expect("capacity should admit the configured update page");
        }
        assert!(matches!(
            tx.try_send(Update::Progress(0)),
            Err(TrySendError::Full(Update::Progress(0)))
        ));
        drop(rx);
    }

    #[test]
    fn idle_worker_does_not_publish_transport_authority() {
        for (engine_present, track_loaded) in [(false, false), (true, false), (false, true)] {
            assert!(transport_update(true, engine_present, track_loaded).is_none());
            assert!(transport_update(false, engine_present, track_loaded).is_none());
        }
        assert!(matches!(
            transport_update(true, true, true),
            Some(Update::Playing(true))
        ));
        assert!(matches!(
            transport_update(false, true, true),
            Some(Update::Playing(false))
        ));
    }
}
