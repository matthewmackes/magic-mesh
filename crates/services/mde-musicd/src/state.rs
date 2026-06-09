//! AIR-8 (v6.1) — mesh playback state + exclusive-playback handoff.
//!
//! Music plays on **one** peer at a time across the workgroup. The
//! playing peer writes its authoritative state to
//! `~/.local/share/mde/music-state.json` every 5 s, plus a per-peer
//! activity snapshot at `music-state-by-peer/<host>.json` (the Peers
//! tab, Q26). When another peer wants to take over, it drops a
//! `music-handoff-intent/<ulid>.json`; the current peer reads it,
//! pauses, surfaces an "Operator-Mac took over" notification, and
//! deletes the intent.
//!
//! All the coordination decisions are pure functions
//! (`is_claimed_by_other`, `pending_takeover_for`, `latest_intent`) so
//! the conflict resolution is fully unit-testable; the daemon (AIR-2)
//! drives the 5 s write + the pause/notify side effects, and
//! `mde-musicd state {show,by-peer,takeover}` is the reachable entry
//! point exercising the files.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A music-state record is considered stale (the playing peer went
/// away without clearing it) after 15 s — three missed 5 s writes.
pub const STATE_STALE_MS: u64 = 15_000;

/// Authoritative "who is playing what" record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicState {
    /// Hostname of the peer that owns playback.
    pub peer: String,
    /// Whether that peer is actively playing (vs paused/idle).
    pub playing: bool,
    /// Currently-loaded song id (empty when idle).
    #[serde(default)]
    pub song_id: String,
    /// Playhead position in ms.
    #[serde(default)]
    pub position_ms: u64,
    /// Epoch-ms of this record's last write (freshness / staleness).
    pub updated_ms: u64,
}

/// A take-over request: `from_peer` asks the current owner to yield.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffIntent {
    /// ULID — also the intent file's basename.
    pub intent_id: String,
    /// Peer requesting playback.
    pub from_peer: String,
    /// Current owner being asked to pause (`None` = claim an idle mesh).
    #[serde(default)]
    pub to_peer: Option<String>,
    /// Epoch-ms the intent was issued (conflict tiebreak: latest wins).
    pub issued_ms: u64,
}

// ───────────────────────── pure decisions ─────────────────────────

/// Is the mesh currently claimed by a peer **other than** `my_host`?
/// Returns that peer's hostname when the state is fresh + playing +
/// owned elsewhere; `None` when idle, stale, or owned by us.
#[must_use]
pub fn is_claimed_by_other(
    state: Option<&MusicState>,
    my_host: &str,
    now_ms: u64,
) -> Option<String> {
    let s = state?;
    if !s.playing {
        return None;
    }
    if s.peer == my_host {
        return None;
    }
    if now_ms.saturating_sub(s.updated_ms) > STATE_STALE_MS {
        return None; // owner went away — mesh is effectively free.
    }
    Some(s.peer.clone())
}

/// The handoff intent (if any) that `my_host` must honour by pausing:
/// an intent whose `to_peer` is `my_host` (or unset, a general claim
/// while we own playback). When several target us, the **latest** wins.
#[must_use]
pub fn pending_takeover_for(intents: &[HandoffIntent], my_host: &str) -> Option<HandoffIntent> {
    intents
        .iter()
        .filter(|i| i.to_peer.as_deref() == Some(my_host) && i.from_peer != my_host)
        .max_by_key(|i| i.issued_ms)
        .cloned()
}

/// Of a set of competing intents, the one that wins (latest `issued_ms`;
/// ties broken by `intent_id` for determinism).
#[must_use]
pub fn latest_intent(intents: &[HandoffIntent]) -> Option<HandoffIntent> {
    intents
        .iter()
        .max_by(|a, b| {
            a.issued_ms
                .cmp(&b.issued_ms)
                .then_with(|| a.intent_id.cmp(&b.intent_id))
        })
        .cloned()
}

// ───────────────────────── file layout ─────────────────────────

/// `$HOME/.local/share/mde/` — the mesh-shared music data root.
#[must_use]
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(".local/share/mde")
}

/// This peer's hostname — the `peer` field on every [`MusicState`] this
/// host writes.
///
/// Falls back to `localhost` when the `hostname` command is unavailable.
#[must_use]
pub fn local_host() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Epoch-ms now (the `updated_ms` / `issued_ms` timestamp source).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Path of the authoritative `music-state.json` within `dir`.
#[must_use]
pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("music-state.json")
}

/// Per-peer snapshot path: `music-state-by-peer/<host>.json`.
#[must_use]
pub fn by_peer_path(dir: &Path, host: &str) -> PathBuf {
    dir.join("music-state-by-peer").join(format!("{host}.json"))
}

/// The handoff-intent directory within `dir`.
#[must_use]
pub fn intents_dir(dir: &Path) -> PathBuf {
    dir.join("music-handoff-intent")
}

/// Read `music-state.json` (None when absent/malformed).
#[must_use]
pub fn read_state(dir: &Path) -> Option<MusicState> {
    std::fs::read_to_string(state_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Write `music-state.json` (+ this peer's by-peer snapshot).
///
/// # Errors
/// IO / serialization failures.
pub fn write_state(dir: &Path, state: &MusicState) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(state_path(dir), &json)?;
    let bp = by_peer_path(dir, &state.peer);
    if let Some(parent) = bp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(bp, json)
}

/// Read every handoff intent in the intents dir (skips malformed).
#[must_use]
pub fn read_intents(dir: &Path) -> Vec<HandoffIntent> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(intents_dir(dir)) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(i) = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                {
                    out.push(i);
                }
            }
        }
    }
    out
}

/// Read every peer's last activity snapshot from `music-state-by-peer/`
/// (the AIR-15.b.5 Peers-tab roster) — the dir [`write_state`] heartbeats
/// each peer's snapshot into. Skips malformed files; sorted by host.
#[must_use]
pub fn read_all_peer_states(dir: &Path) -> Vec<MusicState> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("music-state-by-peer")) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(st) = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| serde_json::from_str::<MusicState>(&s).ok())
                {
                    out.push(st);
                }
            }
        }
    }
    out.sort_by(|a, b| a.peer.cmp(&b.peer));
    out
}

/// Drop a take-over intent from `from_peer` targeting `to_peer`. Returns
/// the written intent (its `intent_id` is the file basename).
///
/// # Errors
/// IO / serialization failures.
pub fn post_takeover(
    dir: &Path,
    from_peer: &str,
    to_peer: Option<String>,
    now_ms: u64,
) -> std::io::Result<HandoffIntent> {
    let id = ulid::Ulid::new().to_string();
    let intent = HandoffIntent {
        intent_id: id.clone(),
        from_peer: from_peer.to_string(),
        to_peer,
        issued_ms: now_ms,
    };
    let d = intents_dir(dir);
    std::fs::create_dir_all(&d)?;
    let json = serde_json::to_string_pretty(&intent)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(d.join(format!("{id}.json")), json)?;
    Ok(intent)
}

/// Delete a handoff intent by id (the yielding peer clears it after
/// pausing). Best-effort.
pub fn clear_intent(dir: &Path, intent_id: &str) {
    let _ = std::fs::remove_file(intents_dir(dir).join(format!("{intent_id}.json")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_all_peer_states_collects_and_sorts_snapshots() {
        let dir = tempfile::TempDir::new().unwrap();
        write_state(
            dir.path(),
            &MusicState {
                peer: "forge".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: 1000,
            },
        )
        .unwrap();
        write_state(
            dir.path(),
            &MusicState {
                peer: "anvil".into(),
                playing: true,
                song_id: "s1".into(),
                position_ms: 0,
                updated_ms: 1000,
            },
        )
        .unwrap();
        let all = read_all_peer_states(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].peer, "anvil");
        assert_eq!(all[1].peer, "forge");
        assert!(read_all_peer_states(tempfile::TempDir::new().unwrap().path()).is_empty());
    }
    use tempfile::tempdir;

    fn state(peer: &str, playing: bool, updated: u64) -> MusicState {
        MusicState {
            peer: peer.into(),
            playing,
            song_id: "s1".into(),
            position_ms: 0,
            updated_ms: updated,
        }
    }

    fn intent(id: &str, from: &str, to: Option<&str>, issued: u64) -> HandoffIntent {
        HandoffIntent {
            intent_id: id.into(),
            from_peer: from.into(),
            to_peer: to.map(ToString::to_string),
            issued_ms: issued,
        }
    }

    #[test]
    fn claimed_by_other_when_fresh_playing_elsewhere() {
        let s = state("anvil", true, 1000);
        assert_eq!(
            is_claimed_by_other(Some(&s), "forge", 2000),
            Some("anvil".into())
        );
    }

    #[test]
    fn not_claimed_when_ours_idle_or_stale() {
        // Owned by us.
        assert_eq!(
            is_claimed_by_other(Some(&state("forge", true, 1000)), "forge", 2000),
            None
        );
        // Not playing.
        assert_eq!(
            is_claimed_by_other(Some(&state("anvil", false, 1000)), "forge", 2000),
            None
        );
        // Stale (owner went away).
        assert_eq!(
            is_claimed_by_other(
                Some(&state("anvil", true, 1000)),
                "forge",
                1000 + STATE_STALE_MS + 1
            ),
            None
        );
        // No state at all.
        assert_eq!(is_claimed_by_other(None, "forge", 2000), None);
    }

    #[test]
    fn pending_takeover_targets_me_latest_wins() {
        let intents = vec![
            intent("a", "forge", Some("anvil"), 10),
            intent("b", "beacon", Some("anvil"), 30), // latest targeting anvil
            intent("c", "forge", Some("other"), 99),  // not for anvil
        ];
        let got = pending_takeover_for(&intents, "anvil").unwrap();
        assert_eq!(got.intent_id, "b");
        assert_eq!(got.from_peer, "beacon");
        // A peer never yields to its own intent.
        let self_only = vec![intent("x", "anvil", Some("anvil"), 5)];
        assert!(pending_takeover_for(&self_only, "anvil").is_none());
    }

    #[test]
    fn latest_intent_breaks_ties_by_id() {
        let intents = vec![
            intent("z", "a", None, 50),
            intent("a", "b", None, 50), // same ts → id tiebreak picks "z" (greater)
            intent("m", "c", None, 10),
        ];
        assert_eq!(latest_intent(&intents).unwrap().intent_id, "z");
        assert!(latest_intent(&[]).is_none());
    }

    #[test]
    fn state_writes_authoritative_plus_by_peer_snapshot() {
        let dir = tempdir().unwrap();
        let s = state("anvil", true, 1234);
        write_state(dir.path(), &s).unwrap();
        assert_eq!(read_state(dir.path()), Some(s.clone()));
        // Per-peer snapshot also written.
        let bp = by_peer_path(dir.path(), "anvil");
        assert!(bp.exists());
        let snap: MusicState = serde_json::from_str(&std::fs::read_to_string(bp).unwrap()).unwrap();
        assert_eq!(snap, s);
    }

    #[test]
    fn post_read_and_clear_intent_round_trip() {
        let dir = tempdir().unwrap();
        let posted = post_takeover(dir.path(), "forge", Some("anvil".into()), 77).unwrap();
        let read = read_intents(dir.path());
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].from_peer, "forge");
        assert_eq!(read[0].issued_ms, 77);
        clear_intent(dir.path(), &posted.intent_id);
        assert!(read_intents(dir.path()).is_empty());
    }

    #[test]
    fn read_state_absent_is_none() {
        let dir = tempdir().unwrap();
        assert_eq!(read_state(dir.path()), None);
    }
}
