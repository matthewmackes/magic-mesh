//! Seat-wide Clock ducking through one bounded PipeWire/WirePlumber authority.
//!
//! The Clock renderer and Music queue remain daemon-owned.  This authority
//! touches only admitted playback streams owned by other seat processes.  It
//! snapshots their exact scalar PipeWire levels, applies a quarter-gain lease,
//! and retains enough state to restore each stream exactly after partial I/O
//! failure.  Product code uses [`WirePlumberCli`]; tests inject [`PipeWireIo`].

use std::collections::{BTreeMap, BTreeSet};

const MAX_ADMITTED_STREAMS: usize = 64;
const DUCK_FACTOR: f64 = 0.25;
const MAX_ADMITTED_GAIN: f64 = 4.0;
const CLASS_STREAM_OUTPUT: &str = "Stream/Output/Audio";

/// One occurrence generation allowed to own the seat duck lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatDuckGeneration {
    /// Stable occurrence identity.
    pub occurrence_id: String,
    /// Globally converged Clock event identity.
    pub global_event_id: String,
    /// Monotonic occurrence generation.
    pub generation: u64,
}

/// Honest failures at the seat-audio authority boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatAudioError {
    /// PipeWire/WirePlumber is absent or unreachable.
    Unavailable,
    /// The graph cannot be represented without losing an exact level.
    InvalidGraph,
    /// More playback streams exist than the bounded authority admits.
    StreamLimit,
    /// Another occurrence generation still owns the lease.
    GenerationConflict,
    /// A volume write failed after the authority was reached.
    ControlFailed,
}

impl SeatAudioError {
    /// Stable Clock status reason code.
    #[must_use]
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::Unavailable => "seat_audio_authority_unavailable",
            Self::InvalidGraph => "seat_audio_graph_invalid",
            Self::StreamLimit => "seat_audio_stream_limit",
            Self::GenerationConflict => "seat_audio_generation_conflict",
            Self::ControlFailed => "seat_audio_control_failed",
        }
    }
}

/// PipeWire graph/control I/O. No renderer or UI code shells out.
pub trait PipeWireIo: Send {
    /// Read one current PipeWire graph snapshot.
    fn dump(&self) -> Result<serde_json::Value, SeatAudioError>;
    /// Set one node to the supplied exact scalar level.
    fn set_volume(&self, node_id: u32, exact_level: f64) -> Result<(), SeatAudioError>;
}

/// Production `pw-dump` + WirePlumber `wpctl` binding.
#[derive(Debug, Clone, Copy, Default)]
pub struct WirePlumberCli;

impl PipeWireIo for WirePlumberCli {
    fn dump(&self) -> Result<serde_json::Value, SeatAudioError> {
        let output = std::process::Command::new("pw-dump")
            .output()
            .map_err(|_| SeatAudioError::Unavailable)?;
        if !output.status.success() {
            return Err(SeatAudioError::Unavailable);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| SeatAudioError::InvalidGraph)
    }

    fn set_volume(&self, node_id: u32, exact_level: f64) -> Result<(), SeatAudioError> {
        let node_id = node_id.to_string();
        // The shortest round-tripping decimal preserves the retained f64.
        let exact_level = exact_level.to_string();
        let output = std::process::Command::new("wpctl")
            .args(["set-volume", &node_id, &exact_level])
            .output()
            .map_err(|_| SeatAudioError::Unavailable)?;
        output
            .status
            .success()
            .then_some(())
            .ok_or(SeatAudioError::ControlFailed)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StreamLevel {
    node_id: u32,
    exact_level: f64,
}

#[derive(Debug, Clone)]
struct DuckLease {
    generation: SeatDuckGeneration,
    /// Streams still requiring restoration. Successful restores are removed so
    /// a retry never overwrites a user's later volume change.
    pending_restore: Vec<StreamLevel>,
}

/// Persistent, generation-idempotent seat ducking authority.
pub struct SeatAudioAuthority {
    io: Box<dyn PipeWireIo>,
    excluded_process_id: u32,
    lease: Option<DuckLease>,
}

impl SeatAudioAuthority {
    /// Bind the authority to the production seat and exclude this daemon PID.
    #[must_use]
    pub fn production() -> Self {
        Self::with_io(Box::new(WirePlumberCli), std::process::id())
    }

    /// Construct the deterministic injected-I/O seam.
    #[must_use]
    pub fn with_io(io: Box<dyn PipeWireIo>, excluded_process_id: u32) -> Self {
        Self {
            io,
            excluded_process_id,
            lease: None,
        }
    }

    /// Snapshot and quarter every admitted external playback stream. Repeating
    /// the same generation is a no-op; a different live generation is refused.
    pub fn duck(&mut self, generation: SeatDuckGeneration) -> Result<(), SeatAudioError> {
        if let Some(lease) = &self.lease {
            return if lease.generation == generation {
                Ok(())
            } else {
                Err(SeatAudioError::GenerationConflict)
            };
        }

        let snapshot = admitted_streams(&self.io.dump()?, self.excluded_process_id)?;
        let mut changed = Vec::with_capacity(snapshot.len());
        for stream in &snapshot {
            if let Err(error) = self
                .io
                .set_volume(stream.node_id, stream.exact_level * DUCK_FACTOR)
            {
                // Retain only writes that happened, then make a best-effort
                // rollback. Any failed restore remains leased for a later retry.
                self.lease = Some(DuckLease {
                    generation,
                    pending_restore: changed,
                });
                let _ = self.restore();
                return Err(error);
            }
            changed.push(stream.clone());
        }
        self.lease = Some(DuckLease {
            generation,
            pending_restore: snapshot,
        });
        Ok(())
    }

    /// Restore every retained exact level. The operation is idempotent and
    /// retries only streams whose previous restore failed.
    pub fn restore(&mut self) -> Result<(), SeatAudioError> {
        let Some(mut lease) = self.lease.take() else {
            return Ok(());
        };
        let mut failed = Vec::new();
        let mut first_error = None;
        for stream in lease.pending_restore.drain(..) {
            if let Err(error) = self.io.set_volume(stream.node_id, stream.exact_level) {
                first_error.get_or_insert(error);
                failed.push(stream);
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        lease.pending_restore = failed;
        self.lease = Some(lease);
        Err(first_error.unwrap_or(SeatAudioError::ControlFailed))
    }

    #[cfg(test)]
    fn pending_restore_count(&self) -> usize {
        self.lease
            .as_ref()
            .map_or(0, |lease| lease.pending_restore.len())
    }
}

fn admitted_streams(
    dump: &serde_json::Value,
    excluded_process_id: u32,
) -> Result<Vec<StreamLevel>, SeatAudioError> {
    let nodes = dump.as_array().ok_or(SeatAudioError::InvalidGraph)?;
    let mut client_processes = BTreeMap::new();
    for client in nodes.iter().filter(|entry| {
        entry.get("type").and_then(serde_json::Value::as_str)
            == Some("PipeWire:Interface:Client")
    }) {
        let id = bounded_u32(client.get("id")).ok_or(SeatAudioError::InvalidGraph)?;
        let process_id = process_id(&client["info"]["props"])
            .ok_or(SeatAudioError::InvalidGraph)?;
        if client_processes.insert(id, process_id).is_some() {
            return Err(SeatAudioError::InvalidGraph);
        }
    }
    let mut admitted = Vec::new();
    let mut ids = BTreeSet::new();
    for node in nodes {
        if node.get("type").and_then(serde_json::Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let info = &node["info"];
        let props = &info["props"];
        if props.get("media.class").and_then(serde_json::Value::as_str) != Some(CLASS_STREAM_OUTPUT)
        {
            continue;
        }
        let direct_process_id = process_id(props);
        let client_process_id = bounded_u32(props.get("client.id"))
            .and_then(|client_id| client_processes.get(&client_id).copied());
        let process_id = match (direct_process_id, client_process_id) {
            (Some(direct), Some(client)) if direct != client => {
                return Err(SeatAudioError::InvalidGraph);
            }
            (Some(direct), _) => direct,
            (None, Some(client)) => client,
            (None, None) => return Err(SeatAudioError::InvalidGraph),
        };
        if process_id == u64::from(excluded_process_id) {
            continue;
        }
        let id = node
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or(SeatAudioError::InvalidGraph)?;
        if !ids.insert(id) {
            return Err(SeatAudioError::InvalidGraph);
        }
        let pw_props = info
            .get("params")
            .and_then(|params| params.get("Props"))
            .and_then(serde_json::Value::as_array)
            .and_then(|props| props.first());
        let exact_level = exact_scalar_level(pw_props)?;
        admitted.push(StreamLevel {
            node_id: id,
            exact_level,
        });
        if admitted.len() > MAX_ADMITTED_STREAMS {
            return Err(SeatAudioError::StreamLimit);
        }
    }
    admitted.sort_by_key(|stream| stream.node_id);
    Ok(admitted)
}

fn bounded_u32(value: Option<&serde_json::Value>) -> Option<u32> {
    value
        .and_then(|candidate| {
            candidate
                .as_u64()
                .or_else(|| candidate.as_str().and_then(|raw| raw.parse().ok()))
        })
        .and_then(|candidate| u32::try_from(candidate).ok())
}

fn process_id(props: &serde_json::Value) -> Option<u64> {
    props.get("application.process.id").and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
    })
}

fn exact_scalar_level(props: Option<&serde_json::Value>) -> Result<f64, SeatAudioError> {
    let props = props.ok_or(SeatAudioError::InvalidGraph)?;
    let level = if let Some(channels) = props
        .get("channelVolumes")
        .and_then(serde_json::Value::as_array)
    {
        if channels.is_empty() {
            return Err(SeatAudioError::InvalidGraph);
        }
        let levels = channels
            .iter()
            .map(serde_json::Value::as_f64)
            .collect::<Option<Vec<_>>>()
            .ok_or(SeatAudioError::InvalidGraph)?;
        // wpctl's scalar control cannot round-trip channel balance. Admit only
        // streams with one exact scalar level and refuse the graph otherwise.
        let first = levels[0];
        if levels.iter().any(|candidate| *candidate != first) {
            return Err(SeatAudioError::InvalidGraph);
        }
        first
    } else if let Some(level) = props.get("volume").and_then(serde_json::Value::as_f64) {
        level
    } else {
        return Err(SeatAudioError::InvalidGraph);
    };
    (level.is_finite() && (0.0..=MAX_ADMITTED_GAIN).contains(&level))
        .then_some(level)
        .ok_or(SeatAudioError::InvalidGraph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FixtureIo {
        dump: serde_json::Value,
        writes: Arc<Mutex<Vec<(u32, f64)>>>,
        fail_node_once: Arc<Mutex<Option<u32>>>,
    }

    impl PipeWireIo for FixtureIo {
        fn dump(&self) -> Result<serde_json::Value, SeatAudioError> {
            Ok(self.dump.clone())
        }

        fn set_volume(&self, node_id: u32, level: f64) -> Result<(), SeatAudioError> {
            if self
                .fail_node_once
                .lock()
                .expect("failure fixture lock")
                .take_if(|failed| *failed == node_id)
                .is_some()
            {
                return Err(SeatAudioError::ControlFailed);
            }
            self.writes
                .lock()
                .expect("write fixture lock")
                .push((node_id, level));
            Ok(())
        }
    }

    fn node(id: u32, process: u32, volume: f64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "media.class": CLASS_STREAM_OUTPUT,
                    "application.process.id": process.to_string()
                },
                "params": { "Props": [{ "channelVolumes": [volume, volume] }] }
            }
        })
    }

    fn client(id: u32, process: u32) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "type": "PipeWire:Interface:Client",
            "info": { "props": { "application.process.id": process } }
        })
    }

    fn generation(number: u64) -> SeatDuckGeneration {
        SeatDuckGeneration {
            occurrence_id: "occurrence-a".into(),
            global_event_id: "global-a".into(),
            generation: number,
        }
    }

    #[test]
    fn exact_levels_are_quartered_and_restored_while_daemon_streams_are_excluded() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let io = FixtureIo {
            dump: serde_json::json!([node(31, 700, 0.73), node(11, 42, 0.81), node(24, 701, 1.2)]),
            writes: writes.clone(),
            fail_node_once: Arc::new(Mutex::new(None)),
        };
        let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);

        authority.duck(generation(9)).unwrap();
        authority.duck(generation(9)).unwrap();
        authority.restore().unwrap();
        authority.restore().unwrap();

        assert_eq!(
            *writes.lock().unwrap(),
            [(24, 0.3), (31, 0.1825), (24, 1.2), (31, 0.73)]
        );
    }

    #[test]
    fn generation_conflict_never_overwrites_the_retained_snapshot() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let io = FixtureIo {
            dump: serde_json::json!([node(7, 700, 0.6)]),
            writes: writes.clone(),
            fail_node_once: Arc::new(Mutex::new(None)),
        };
        let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);
        authority.duck(generation(1)).unwrap();
        assert_eq!(
            authority.duck(generation(2)),
            Err(SeatAudioError::GenerationConflict)
        );
        authority.restore().unwrap();
        assert_eq!(*writes.lock().unwrap(), [(7, 0.15), (7, 0.6)]);
    }

    #[test]
    fn failed_restore_is_retained_and_retried_without_rewriting_successes() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let io = FixtureIo {
            dump: serde_json::json!([node(4, 700, 0.4), node(8, 701, 0.8)]),
            writes: writes.clone(),
            fail_node_once: failure.clone(),
        };
        let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);
        authority.duck(generation(4)).unwrap();
        *failure.lock().unwrap() = Some(8);
        assert_eq!(authority.restore(), Err(SeatAudioError::ControlFailed));
        assert_eq!(authority.pending_restore_count(), 1);
        authority.restore().unwrap();
        assert_eq!(
            *writes.lock().unwrap(),
            [(4, 0.1), (8, 0.2), (4, 0.4), (8, 0.8)]
        );
    }

    #[test]
    fn unrepresentable_channel_balance_fails_before_any_write() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut graph = node(4, 700, 0.4);
        graph["info"]["params"]["Props"][0]["channelVolumes"] = serde_json::json!([0.4, 0.5]);
        let io = FixtureIo {
            dump: serde_json::json!([graph]),
            writes: writes.clone(),
            fail_node_once: Arc::new(Mutex::new(None)),
        };
        let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);
        assert_eq!(
            authority.duck(generation(1)),
            Err(SeatAudioError::InvalidGraph)
        );
        assert!(writes.lock().unwrap().is_empty());
    }

    #[test]
    fn unidentified_or_level_less_streams_fail_before_any_write() {
        for missing_process_identity in [true, false] {
            let writes = Arc::new(Mutex::new(Vec::new()));
            let mut graph = node(4, 700, 0.4);
            if missing_process_identity {
                graph["info"]["props"]
                    .as_object_mut()
                    .unwrap()
                    .remove("application.process.id");
            } else {
                graph["info"]["params"]["Props"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("channelVolumes");
            }
            let io = FixtureIo {
                dump: serde_json::json!([graph]),
                writes: writes.clone(),
                fail_node_once: Arc::new(Mutex::new(None)),
            };
            let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);
            assert_eq!(
                authority.duck(generation(1)),
                Err(SeatAudioError::InvalidGraph)
            );
            assert!(writes.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn client_owned_process_identity_excludes_daemon_streams_and_admits_external_streams() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut daemon = node(41, 42, 1.0);
        daemon["info"]["props"].as_object_mut().unwrap().remove("application.process.id");
        daemon["info"]["props"]["client.id"] = serde_json::json!(40);
        let mut external = node(51, 700, 0.8);
        external["info"]["props"].as_object_mut().unwrap().remove("application.process.id");
        external["info"]["props"]["client.id"] = serde_json::json!(50);
        let io = FixtureIo {
            dump: serde_json::json!([client(40, 42), daemon, client(50, 700), external]),
            writes: writes.clone(),
            fail_node_once: Arc::new(Mutex::new(None)),
        };
        let mut authority = SeatAudioAuthority::with_io(Box::new(io), 42);

        authority.duck(generation(2)).unwrap();
        authority.restore().unwrap();

        assert_eq!(*writes.lock().unwrap(), [(51, 0.2), (51, 0.8)]);
    }
}
