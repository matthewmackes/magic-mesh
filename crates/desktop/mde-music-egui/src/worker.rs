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

use std::sync::mpsc::{self, Receiver, Sender};

use mde_egui::egui::Context;
use mde_musicd::airsonic::{Client, Song};
use mde_musicd::engine::Engine;

use crate::model::{track_for_engine, Command, Update};

/// Albums fetched per library listing. Subsonic's `getAlbumList2` caps `size` at
/// 500; one page covers the first-slice listing.
const LIBRARY_PAGE: u32 = 500;

/// The `getAlbumList2` ordering used for the library listing.
const LIBRARY_ORDER: &str = "alphabeticalByName";

/// Spawn the worker thread around `client`, returning the [`Command`] sender the
/// UI drives it with. `ctx` is repainted after every [`Update`]; `updates`
/// carries results back. If the thread cannot be spawned, an [`Update::Error`] is
/// sent so the UI surfaces it rather than silently doing nothing.
pub fn spawn(client: Client, ctx: Context, updates: Sender<Update>) -> Sender<Command> {
    let (tx, rx) = mpsc::channel::<Command>();
    let err_tx = updates.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("mde-music-egui-worker".to_string())
        .spawn(move || run(&client, &ctx, &updates, &rx))
    {
        let _ = err_tx.send(Update::Error(format!("could not start music worker: {e}")));
    }
    tx
}

/// The worker loop: build the runtime, then service commands until the UI hangs
/// up (its command sender drops, ending `recv`).
fn run(client: &Client, ctx: &Context, updates: &Sender<Update>, rx: &Receiver<Command>) {
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
    // Opened on first play; a headless host with no sound card surfaces the
    // failure once, on the play attempt, instead of failing the whole surface.
    let mut engine: Option<Engine> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::LoadLibrary => {
                let result = rt
                    .block_on(client.get_album_list2(LIBRARY_ORDER, LIBRARY_PAGE))
                    .map_err(|e| e.to_string());
                let _ = updates.send(Update::Library(result));
            }
            Command::LoadAlbum(id) => {
                let result = rt
                    .block_on(client.get_album(&id))
                    .map(|detail| detail.songs)
                    .map_err(|e| e.to_string());
                let _ = updates.send(Update::Tracks {
                    album_id: id,
                    result,
                });
            }
            Command::Play(song) => play(client, &mut engine, updates, song),
            Command::Pause => {
                if let Some(eng) = engine.as_ref() {
                    eng.pause();
                }
                let _ = updates.send(Update::Playing(false));
            }
            Command::Resume => {
                if let Some(eng) = engine.as_ref() {
                    eng.resume();
                }
                let _ = updates.send(Update::Playing(true));
            }
            Command::Stop => {
                if let Some(eng) = engine.as_ref() {
                    eng.stop();
                }
                let _ = updates.send(Update::Stopped);
            }
        }
        // Wake the UI to drain the update we just sent.
        ctx.request_repaint();
    }
}

/// Lazily open the audio engine (first play only). Returns a borrow of the live
/// engine, or `None` after surfacing an [`Update::Error`] when no output device
/// is available.
fn ensure_engine<'a>(
    engine: &'a mut Option<Engine>,
    updates: &Sender<Update>,
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
/// engine, replacing any current playback. Confirms with [`Update::Started`].
fn play(client: &Client, engine: &mut Option<Engine>, updates: &Sender<Update>, song: Song) {
    if let Some(eng) = ensure_engine(engine, updates) {
        let (url, codec) = track_for_engine(client, &song);
        eng.play(vec![(url, codec)]);
        let _ = updates.send(Update::Started(song));
    }
}
