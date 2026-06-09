//! `mde-musicd` binary.
//!
//! Subcommands exercise the daemon's modules end-to-end (their §0.12
//! runtime reachability): `ping` (creds + Airsonic reach), `queue` /
//! `state` / `cache` (AIR-2/7/8), `serve` (the Bus control responder),
//! and `play` (the AIR-5 engine: Symphonia decode → cpal output).
//! The remaining `D-Bus` + MPRIS surfaces land in AIR-2.c/6.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use mde_musicd::airsonic::Client;
use mde_musicd::engine::{Engine, SourceCodec};
use mde_musicd::{bus_responder, cache, creds, queue, reconnect, state};

#[derive(Parser)]
#[command(name = "mde-musicd", about = "MDE native Airsonic music daemon.")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Load the mesh-shared creds + reach the Airsonic server, printing
    /// its reported API version. Exits non-zero when creds are missing
    /// or the server is unreachable.
    Ping {
        /// Retry on failure with exponential backoff (AIR-9 schedule),
        /// up to this many extra attempts (0 = single try).
        #[arg(long, default_value_t = 0)]
        retry: u32,
    },
    /// Inspect or trim the mesh-shared audio cache (AIR-7).
    Cache {
        #[command(subcommand)]
        op: CacheOp,
    },
    /// Inspect mesh playback state or request a take-over (AIR-8).
    State {
        #[command(subcommand)]
        op: StateOp,
    },
    /// Inspect or mutate the playback queue (AIR-2).
    Queue {
        #[command(subcommand)]
        op: QueueOp,
    },
    /// Run the daemon's Bus control responder (`action/music/<verb>` →
    /// `reply/<ulid>`). Loops until interrupted (AIR-2 control surface;
    /// the play flow + MPRIS are AIR-2.c, gated on the audio engine).
    Serve,
    /// Decode + play one or more songs gaplessly through the native
    /// engine (AIR-5: Symphonia → cpal). Resolves each id's Airsonic
    /// stream URL, then blocks until the last track finishes. This is the
    /// engine's runtime entry point (§0.12); gap-free album playback is a
    /// release HW-bench item (§0.15).
    Play {
        /// Airsonic song ids to play, in order.
        #[arg(required = true)]
        song_ids: Vec<String>,
    },
}

#[derive(Subcommand)]
enum QueueOp {
    /// Append a song-id to the end of the queue.
    Add { song_id: String },
    /// Insert a song-id right after the current track (Play Next).
    AddNext { song_id: String },
    /// Print the queue, marking the current track.
    List,
    /// Advance to the next track + print it.
    Next,
    /// Step back to the previous track + print it.
    Prev,
    /// Empty the queue.
    Clear,
}

#[derive(Subcommand)]
enum StateOp {
    /// Print the authoritative "who is playing what" record.
    Show,
    /// List every peer's last-known playback snapshot.
    ByPeer,
    /// Request that the peer currently playing yields to this host.
    Takeover {
        /// The host to take over from (the current playing peer).
        peer: String,
    },
}

#[derive(Subcommand)]
enum CacheOp {
    /// Print the cache size, track count, and cap.
    Status {
        /// Cap in GiB (default 10).
        #[arg(long, default_value_t = 10)]
        cap_gb: u64,
    },
    /// Evict least-recently-played non-starred tracks to fit the cap.
    Gc {
        /// Cap in GiB (default 10).
        #[arg(long, default_value_t = 10)]
        cap_gb: u64,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.cmd {
        Cmd::Ping { retry } => ping(retry),
        Cmd::Cache { op } => cache_cmd(&op),
        Cmd::State { op } => state_cmd(&op),
        Cmd::Queue { op } => queue_cmd(&op),
        Cmd::Serve => serve(),
        Cmd::Play { song_ids } => play_cmd(&song_ids),
    }
}

fn play_cmd(song_ids: &[String]) -> ExitCode {
    let creds = match creds::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let client = Client::new(&creds.server_url, &creds.username, &creds.password);
    // The stream endpoint serves the decodable bytes regardless of
    // suffix; Symphonia probes the container from content, so an
    // id-only CLI play hands the engine an Unknown hint.
    let tracks: Vec<(String, SourceCodec)> = song_ids
        .iter()
        .map(|id| (client.stream_url(id), SourceCodec::Unknown))
        .collect();
    let engine = match Engine::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("mde-musicd: audio engine unavailable: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("mde-musicd: playing {} track(s)", tracks.len());
    engine.play(tracks);
    // Block until the decode thread drains + the ring empties.
    while engine.is_active() {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    ExitCode::SUCCESS
}

fn serve() -> ExitCode {
    let Some(bus_root) = mde_bus::default_data_dir() else {
        eprintln!("mde-musicd: no Bus data dir (XDG) — cannot serve");
        return ExitCode::FAILURE;
    };
    let persist = match mde_bus::persist::Persist::open(bus_root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mde-musicd: opening Bus store: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("mde-musicd: serving action/music/* on the Bus");
    // Runs until SIGTERM (the AIR-1 systemd unit manages the lifecycle);
    // `serve`'s stop predicate is exercised by the unit tests' one-shot
    // poll path.
    bus_responder::serve(&persist, &queue::queue_path(), || false);
    ExitCode::SUCCESS
}

fn queue_cmd(op: &QueueOp) -> ExitCode {
    let path = queue::queue_path();
    let mut q = queue::read_from(&path);
    let mut mutated = true;
    match op {
        QueueOp::Add { song_id } => q.enqueue(song_id.clone()),
        QueueOp::AddNext { song_id } => q.enqueue_after_current(song_id.clone()),
        QueueOp::Clear => q.clear(),
        QueueOp::Next => match q.next() {
            Some(s) => println!("now playing: {s}"),
            None => println!("end of queue"),
        },
        QueueOp::Prev => match q.prev() {
            Some(s) => println!("now playing: {s}"),
            None => println!("start of queue"),
        },
        QueueOp::List => {
            mutated = false;
            if q.is_empty() {
                println!("queue empty");
            } else {
                let cur = q.current().map(ToString::to_string);
                for (i, s) in q.songs.iter().enumerate() {
                    let marker = if Some(s) == cur.as_ref() { "▶" } else { " " };
                    println!("{marker} {i}: {s}");
                }
            }
        }
    }
    if mutated {
        if let Err(e) = queue::write_to(&path, &q) {
            eprintln!("mde-musicd: writing queue: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn state_cmd(op: &StateOp) -> ExitCode {
    let dir = state::data_dir();
    match op {
        StateOp::Show => {
            match state::read_state(&dir) {
                Some(s) if s.playing => {
                    println!(
                        "playing on {}: song {} @ {}ms",
                        s.peer, s.song_id, s.position_ms
                    );
                }
                Some(s) => println!("idle (last owner: {})", s.peer),
                None => println!("no mesh playback state (nobody is playing)"),
            }
            ExitCode::SUCCESS
        }
        StateOp::ByPeer => {
            let bp_dir = dir.join("music-state-by-peer");
            match std::fs::read_dir(&bp_dir) {
                Ok(rd) => {
                    let mut any = false;
                    for entry in rd.flatten() {
                        if let Some(s) = std::fs::read_to_string(entry.path())
                            .ok()
                            .and_then(|t| serde_json::from_str::<state::MusicState>(&t).ok())
                        {
                            any = true;
                            println!("{}: {}", s.peer, if s.playing { "playing" } else { "idle" });
                        }
                    }
                    if !any {
                        println!("no peer snapshots yet");
                    }
                }
                Err(_) => println!("no peer snapshots yet"),
            }
            ExitCode::SUCCESS
        }
        StateOp::Takeover { peer } => {
            let me = local_hostname();
            match state::post_takeover(&dir, &me, Some(peer.clone()), now_ms()) {
                Ok(i) => {
                    println!(
                        "take-over requested: {} → {} (intent {})",
                        me, peer, i.intent_id
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("mde-musicd: take-over failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn cache_cmd(op: &CacheOp) -> ExitCode {
    let dir = cache::cache_dir();
    match op {
        CacheOp::Status { cap_gb } => {
            let index = cache::read_index(&dir);
            let cap = cap_gb * 1024 * 1024 * 1024;
            println!(
                "music cache: {} across {} track(s) (cap {})",
                cache::human_bytes(index.total_bytes()),
                index.entries.len(),
                cache::human_bytes(cap),
            );
            ExitCode::SUCCESS
        }
        CacheOp::Gc { cap_gb } => {
            let cap = cap_gb * 1024 * 1024 * 1024;
            match cache::run_gc(&dir, cap) {
                Ok(evicted) => {
                    println!("music cache: evicted {} track(s)", evicted.len());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("mde-musicd: cache gc failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn ping(retry: u32) -> ExitCode {
    let creds = match creds::load() {
        Ok(c) => c,
        Err(e) => {
            // The Missing case already carries the first-run hint.
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let client = Client::new(&creds.server_url, &creds.username, &creds.password);
    // Drive the async ping on a small runtime — the daemon proper will
    // host a long-lived runtime (AIR-2).
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mde-musicd: runtime build failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Attempt 0 plus `retry` more, waiting the AIR-9 backoff between
    // failures (1s, 2s, 4s, …, 60s cap).
    for attempt in 0..=retry {
        match rt.block_on(client.ping()) {
            Ok(version) => {
                println!("airsonic {}: reachable (API v{version})", creds.server_url);
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                if attempt == retry {
                    eprintln!("mde-musicd: {e}");
                    return ExitCode::from(3);
                }
                let delay = reconnect::backoff_delay_secs(
                    attempt,
                    reconnect::DEFAULT_BASE_SECS,
                    reconnect::DEFAULT_CAP_SECS,
                );
                eprintln!("mde-musicd: {e} — retrying in {delay}s");
                std::thread::sleep(std::time::Duration::from_secs(delay));
            }
        }
    }
    ExitCode::from(3)
}
