//! PC-3 — peer-join handler.
//!
//! On `peer_joined { id }` from the mesh layer, this module:
//!   1. Writes the peer's [`PeerProbe`] to
//!      `~/.cache/mde/peers/<peer-id>/probe.json`
//!   2. Spawns `mde-peer-card --peer <id>` as a detached child
//!      (the modal lives in its own process so a crash there
//!      doesn't ripple back into mded).
//!   3. Debounces re-spawn within a 30 s window per peer-id so
//!      flapping links don't flood the user with modals.
//!
//! Event-source integration (the actual `peer_joined` event
//! emission point in mackesd's mesh/topology layer) is a
//! follow-up — PC-3.a — that wires this handler into mackesd's
//! enrollment + reconcile loops. The handler itself is
//! stand-alone + testable without that integration.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mackes_mesh_types::PeerProbe;

/// PC-3 spec — minimum gap between two peer-card spawns for
/// the same peer-id. 30 s is short enough that a deliberate
/// re-test of a peer (unplug + replug) re-spawns; long enough
/// that a flapping link doesn't carpet-bomb the user.
pub const DEBOUNCE_WINDOW: Duration = Duration::from_secs(30);

/// Default binary name to spawn. Made `pub` so an out-of-band
/// installer can probe + an integration test can verify it.
pub const PEER_CARD_BIN: &str = "mde-peer-card";

/// Process-wide debounce map: peer-id → last spawn instant.
fn debounce_map() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build the probe cache path for a peer. Returns `None` when
/// no `HOME` is set (headless CI containers can override via
/// `XDG_CACHE_HOME`).
#[must_use]
pub fn probe_cache_path(peer_id: &str) -> Option<PathBuf> {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(
        base.join("mde")
            .join("peers")
            .join(peer_id)
            .join("probe.json"),
    )
}

/// Should the peer-card spawn for `peer_id` now? Updates the
/// debounce map as a side effect: returns `true` exactly when
/// the previous spawn for this peer was longer ago than
/// [`DEBOUNCE_WINDOW`] (or never happened).
pub fn debounce_allows(peer_id: &str, now: Instant) -> bool {
    let map = debounce_map();
    let mut guard = map.lock().expect("debounce-map mutex poisoned");
    match guard.get(peer_id) {
        Some(last) if now.duration_since(*last) < DEBOUNCE_WINDOW => false,
        _ => {
            guard.insert(peer_id.to_owned(), now);
            true
        }
    }
}

/// Reset the debounce state for a specific peer. Used in tests
/// + by a future `mded peer reset <peer-id>` admin verb if
/// operators want to force a re-spawn.
pub fn debounce_reset(peer_id: &str) {
    let map = debounce_map();
    let mut guard = map.lock().expect("debounce-map mutex poisoned");
    guard.remove(peer_id);
}

/// Write the probe blob to disk at the per-peer cache path.
/// Creates parent directories as needed.
pub fn write_probe(probe: &PeerProbe) -> io::Result<PathBuf> {
    let Some(path) = probe_cache_path(&probe.peer_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no $HOME or $XDG_CACHE_HOME set; cannot resolve probe cache path",
        ));
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(probe)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, body)?;
    Ok(path)
}

/// Spawn `mde-peer-card --peer <id>` as a detached child.
/// Returns the child PID on success. The child's stdout / stderr
/// inherit mded's so journald logging captures both.
pub fn spawn_peer_card(peer_id: &str) -> io::Result<u32> {
    let child = Command::new(PEER_CARD_BIN)
        .arg("--peer")
        .arg(peer_id)
        .spawn()?;
    Ok(child.id())
}

/// Top-level handler called from the mackesd event loop (PC-3.a
/// integration target). Writes the probe + spawns the modal,
/// respecting the per-peer debounce window. Returns `Ok(Some(pid))`
/// when a card spawned, `Ok(None)` when the spawn was debounced.
pub fn handle_peer_joined(probe: &PeerProbe) -> io::Result<Option<u32>> {
    write_probe(probe)?;
    if !debounce_allows(&probe.peer_id, Instant::now()) {
        return Ok(None);
    }
    let pid = spawn_peer_card(&probe.peer_id)?;
    Ok(Some(pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Generate a unique peer-id per test so concurrent test
    /// runs don't poison each other's debounce state.
    fn unique_peer_id(tag: &str) -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        format!("test-{tag}-{}", N.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn debounce_allows_first_spawn() {
        let pid = unique_peer_id("first");
        let now = Instant::now();
        assert!(debounce_allows(&pid, now));
    }

    #[test]
    fn debounce_blocks_within_window() {
        let pid = unique_peer_id("blocks");
        let t0 = Instant::now();
        assert!(debounce_allows(&pid, t0));
        // 5 s later — well inside the 30 s window.
        let t1 = t0 + Duration::from_secs(5);
        assert!(!debounce_allows(&pid, t1));
    }

    #[test]
    fn debounce_allows_after_window() {
        let pid = unique_peer_id("after");
        let t0 = Instant::now();
        assert!(debounce_allows(&pid, t0));
        let t1 = t0 + DEBOUNCE_WINDOW + Duration::from_secs(1);
        assert!(debounce_allows(&pid, t1));
    }

    #[test]
    fn debounce_reset_clears_state() {
        let pid = unique_peer_id("reset");
        let t0 = Instant::now();
        assert!(debounce_allows(&pid, t0));
        debounce_reset(&pid);
        // After reset, the very next spawn at the same instant
        // is allowed again.
        assert!(debounce_allows(&pid, t0));
    }

    #[test]
    fn probe_cache_path_lives_under_cache_home() {
        // Verify the path shape — actual filesystem write is
        // covered by `write_probe_creates_parents` below.
        std::env::set_var("HOME", "/home/testuser");
        std::env::remove_var("XDG_CACHE_HOME");
        let path = probe_cache_path("peer-x").unwrap();
        let s = path.to_string_lossy();
        assert!(s.starts_with("/home/testuser/.cache/mde/peers/peer-x"));
        assert!(s.ends_with("probe.json"));
    }

    #[test]
    fn xdg_cache_home_overrides_dotcache() {
        std::env::set_var("XDG_CACHE_HOME", "/tmp/mde-test-xdg-cache");
        let path = probe_cache_path("peer-y").unwrap();
        assert!(path
            .to_string_lossy()
            .starts_with("/tmp/mde-test-xdg-cache/mde/peers/peer-y"));
        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn write_probe_creates_parents_and_serializes_json() {
        let tmp = tempfile_dir("write_probe");
        std::env::set_var("XDG_CACHE_HOME", &tmp);

        let probe = PeerProbe::fixture();
        let path = write_probe(&probe).expect("write_probe succeeds");
        assert!(path.exists(), "probe.json was created");

        // Round-trip parse to confirm valid JSON serialization.
        let raw = fs::read_to_string(&path).expect("re-read");
        let back: PeerProbe = serde_json::from_str(&raw).expect("parse");
        assert_eq!(back, probe);

        let _ = fs::remove_dir_all(&tmp);
        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn debounce_window_is_thirty_seconds() {
        // PC-3 spec lock — guard against silent drift.
        assert_eq!(DEBOUNCE_WINDOW, Duration::from_secs(30));
    }

    /// Pick a unique tempdir path per test to avoid cross-test
    /// pollution from concurrent fs writes.
    fn tempfile_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("mde-peer-join-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("create tempdir");
        p
    }
}
